//! The AST-aware dispatch-submission wrappers. Each resolves `(scope, node_scope, chain)` from
//! scheduler state and forwards to [`KoanRuntime::submit_dispatch`] — so these are the only callers
//! that turn a `KExpression` into scheduler work. The harness owns the sole `&mut Scheduler`; the
//! AST-free submission prims they reach (`ensure_run_frame`, `resolve_node_scope`, `submit_in_own_scope`,
//! `submit_node`) stay on [`Scheduler`](super::super::scheduler::Scheduler).

use std::rc::Rc;

use crate::machine::core::{assemble_body_chain, ScopeId};
use crate::machine::model::ast::KExpression;
use crate::machine::{CallArena, LexicalFrame, NodeId, Scope};

use super::super::nodes::{NodeScope, NodeWork};
use super::super::DepFinish;
use super::KoanRuntime;

impl<'run> KoanRuntime<'run> {
    /// Submit each `statement` as a fresh lexical block over `scope`: mint a frame `(scope_id, i+1)`
    /// per statement (parent = the ambient `active_chain`) and dispatch each against `scope`. The
    /// program / REPL / test entry point for a block of top-level statements.
    pub fn enter_block(
        &mut self,
        scope_id: ScopeId,
        statements: Vec<KExpression<'run>>,
        scope: &'run Scope<'run>,
    ) -> Vec<NodeId> {
        let parent = self.sched.active_chain_clone();
        // Indices start at 1: visibility is strict less-than and builtins sit at idx 0,
        // so a top-level statement at index 1 sees them via `0 < 1`.
        statements
            .into_iter()
            .enumerate()
            .map(|(i, expr)| {
                let chain = LexicalFrame::push(parent.clone(), scope_id, i + 1);
                self.dispatch_in_scope_with_chain(expr, scope, Some(chain))
            })
            .collect()
    }

    /// Submit an unresolved expression for the scheduler to dispatch + execute against `scope`,
    /// inheriting the ambient (or, at top level, a detached) lexical chain. The only public way to
    /// add work.
    pub fn dispatch_in_scope(&mut self, expr: KExpression<'run>, scope: &'run Scope<'run>) -> NodeId {
        let chain = self.sched.ambient_or_detached_chain();
        self.dispatch_in_scope_with_chain(expr, scope, chain)
    }

    /// Submit `expr` against a run-lived `scope`: establish the run frame, decide the slot's
    /// [`NodeScope`] handle against `scope`, then submit. `chain` is the caller's resolved lexical
    /// chain — ambient for [`Self::dispatch_in_scope`], or the per-statement block chain for
    /// [`Self::enter_block`].
    fn dispatch_in_scope_with_chain(
        &mut self,
        expr: KExpression<'run>,
        scope: &'run Scope<'run>,
        chain: Option<Rc<LexicalFrame>>,
    ) -> NodeId {
        self.sched.ensure_run_frame(scope);
        let node_scope = self.sched.resolve_node_scope(scope);
        self.submit_dispatch(expr, scope, node_scope, chain)
    }

    /// Dispatch `expr` as a sub-slot of the currently-active per-call frame, storing the slot's
    /// scope as a `Yoked` handle re-projected from the frame cart rather than a fabricated `&'run`.
    /// The caller must have installed the per-call frame as `active_frame` (the run loop does this
    /// per step; [`Self::dispatch_body`] does it transiently). `chain` is the explicit
    /// lexical chain (`Some` for an `enter_block`-routed body statement; the ambient-inheriting
    /// `ActiveFrame` placement passes [`Scheduler::ambient_or_detached_chain`]).
    ///
    /// [`Scheduler::ambient_or_detached_chain`]: super::super::scheduler::Scheduler::ambient_or_detached_chain
    pub(in crate::machine::execute) fn dispatch_in_active_frame(
        &mut self,
        expr: KExpression<'run>,
        chain: Option<Rc<LexicalFrame>>,
    ) -> NodeId {
        let frame = self
            .sched
            .current_frame()
            .expect("in-frame dispatch requires an active frame");
        // `scope_for_bind` is `Rc`-bounded — not a free `'run`-fabrication. The slot stores `Yoked`
        // and re-projects the scope from the frame cart at the read boundary, so this short borrow
        // only needs to outlive the `submit_dispatch` call.
        let scope = frame.scope_for_bind();
        self.submit_dispatch(expr, scope, NodeScope::Yoked, chain)
    }

    /// Dispatch `expr` against the executing slot's own scope handle — the honest
    /// re-dispatch-against-my-own-scope path (the `OwnScope` dep placement). An `Anchored` slot
    /// reuses its genuine run-lived borrow; a `Yoked` slot routes through
    /// [`Self::dispatch_in_active_frame`] to re-project from the active frame cart. Either way routes
    /// through [`Self::submit_dispatch`], so a binder spliced here still installs its placeholder.
    pub(in crate::machine::execute) fn dispatch_in_own_scope(&mut self, expr: KExpression<'run>) -> NodeId {
        let node_scope = self
            .sched
            .current_node_scope()
            .expect("a slot step installs active_node_scope before the body submits");
        let chain = self.sched.ambient_or_detached_chain();
        match node_scope {
            NodeScope::Anchored(scope) => self.submit_dispatch(expr, scope, node_scope, chain),
            NodeScope::Yoked => self.dispatch_in_active_frame(expr, chain),
        }
    }

    /// Dispatch a body's non-tail `statements` as sibling sub-slots in `frame`, each positioned at
    /// body-chain index `i + 1` (the params / `it` sit at idx 0) over the frame's body scope, with
    /// the parent chain reconstructed from the call site via [`assemble_body_chain`]. The shared
    /// "execute a block of expressions" primitive: a multi-statement FN body (`KFunction::invoke`),
    /// a deferred return-type dep, and a MATCH/TRY arm body (the action harness) all use it. The
    /// caller tail-replaces into the body's last statement separately. Returns the sub-slots' ids.
    pub(in crate::machine::execute) fn dispatch_body(
        &mut self,
        frame: &Rc<CallArena>,
        statements: Vec<KExpression<'run>>,
    ) -> Vec<NodeId> {
        let body_scope = frame.scope_for_bind();
        let body_scope_id = body_scope.id;
        let parent = assemble_body_chain(
            body_scope,
            self.sched
                .current_lexical_chain()
                .expect("a body block runs inside an active lexical chain"),
            0,
        )
        .parent
        .clone();
        let mut ids = Vec::with_capacity(statements.len());
        for (i, statement) in statements.into_iter().enumerate() {
            let statement_chain = LexicalFrame::push(parent.clone(), body_scope_id, i + 1);
            // Install `frame` as the ambient cart so `dispatch_in_active_frame` reads it back, then
            // restore the previous — the sub-slot inherits this frame, not the caller's.
            let prev = self.sched.swap_active_frame(Some(frame.clone()));
            let bid = self.dispatch_in_active_frame(statement, Some(statement_chain));
            self.sched.swap_active_frame(prev);
            ids.push(bid);
        }
        ids
    }

    /// Schedule a `Combine` against the executing slot's own scope handle. `owned_subs` are
    /// cascade-freed on success; `park_producers` are existing siblings the combine reads but does
    /// not own. The finish sees results as `[park_producers..., owned_subs...]`.
    pub(in crate::machine::execute) fn submit_dep_finish_in_own_scope(
        &mut self,
        owned_subs: Vec<NodeId>,
        park_producers: Vec<NodeId>,
        finish: DepFinish<'run>,
    ) -> NodeId {
        let park_count = park_producers.len();
        let mut deps = park_producers;
        deps.extend(owned_subs);
        self.sched
            .submit_in_own_scope(NodeWork::awaiting(deps, park_count, finish))
    }
}
