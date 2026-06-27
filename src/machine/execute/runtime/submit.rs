//! The AST-aware dispatch-submission wrappers. Each resolves `(scope, node_scope, chain)` — the Koan
//! name-resolution payload — and forwards to [`KoanRuntime::submit_expression`], so these are the only
//! callers that turn a `KExpression` into scheduler work. Payload construction (`resolve_node_scope`,
//! `ambient_or_detached_chain`, `submit_in_own_scope`, the frame `ensure_run_frame` / tail-reuse
//! mint) lives here on the workload; the scheduler core exposes only the payload-agnostic
//! [`Scheduler::alloc_node`] and the frame-lifecycle accessors it manages.

use std::rc::Rc;

use crate::machine::core::{assemble_body_chain, ErasedScopePtr, KoanRegion, ScopeId};
use crate::machine::model::ast::KExpression;
use crate::machine::{CallFrame, LexicalFrame, NodeId, Scope};

use super::super::dispatch::reattach_node_scope;

/// Pointer equality of two scopes (identity, not structural).
fn scopes_eq(a: &Scope<'_>, b: &Scope<'_>) -> bool {
    std::ptr::eq(
        a as *const Scope<'_> as *const (),
        b as *const Scope<'_> as *const (),
    )
}

/// Whether `target` region is reached by walking `cart_scope`'s lexical `outer` chain — i.e. the
/// scope lives in the cart's own region or a cart ancestor's. The active cart's `FrameStorage.outer` chain
/// pins every such region, so a scope found here is cart-witnessed (a `YokedChild`), not run-lived.
fn cart_chain_reaches_region(cart_scope: &Scope<'_>, target: &KoanRegion) -> bool {
    let target = target as *const KoanRegion as *const ();
    let mut cur = Some(cart_scope);
    while let Some(s) = cur {
        if std::ptr::eq(s.region as *const KoanRegion as *const (), target) {
            return true;
        }
        cur = s.outer();
    }
    false
}

#[cfg(test)]
use super::super::nodes::NodePayload;
use super::super::nodes::{NodeScope, NodeWork};
use super::super::outcome::dep_error_frame;
use super::super::{short_circuit_witnessed, WitnessedDepFinish};
#[cfg(test)]
use super::super::{short_circuit, DepFinish};
use super::{KoanRuntime, KoanWorkload};

/// A bare dep-finish node built for direct submission (not via `apply_outcome`): waits on `deps` (a
/// `park_count`-long park prefix, owned suffix), short-circuits on the first errored dep under the
/// [`dep_error_frame`] label, else hands the resolved values to a value-only `finish`. Used only by
/// the test fixture below; the run path's construction finishes route the witnessed
/// [`awaiting_witnessed`].
#[cfg(test)]
fn awaiting(deps: Vec<NodeId>, park_count: usize, finish: DepFinish<'_>) -> NodeWork<KoanWorkload> {
    NodeWork::new(
        deps,
        park_count,
        short_circuit(Some(dep_error_frame()), finish),
        None,
    )
}

/// Witnessed sibling of [`awaiting`]: builds a dep-finish node whose continuation folds the resolved
/// deps into a witnessed aggregate carrier ([`short_circuit_witnessed`]) instead of handing bare
/// values to a value-only finish.
fn awaiting_witnessed(
    deps: Vec<NodeId>,
    park_count: usize,
    finish: WitnessedDepFinish<'_>,
) -> NodeWork<KoanWorkload> {
    NodeWork::new(
        deps,
        park_count,
        short_circuit_witnessed(Some(dep_error_frame()), finish),
        None,
    )
}

impl<'run> KoanRuntime<'run> {
    /// The explicit chain a submission passes when there is no slot step installed: a detached chain
    /// so the visibility predicate treats every scope as "complete" (test fixtures / top-level),
    /// else `None` to inherit the ambient payload's chain at the submission funnel.
    pub(in crate::machine::execute) fn ambient_or_detached_chain(
        &self,
    ) -> Option<Rc<LexicalFrame>> {
        (!self.has_active_payload()).then(LexicalFrame::detached)
    }

    /// Establish the run frame on the first run-lifetime submission (top-level run scope), so every
    /// top-level slot carries a frame cart and `active_frame` is never `None` during a top-level
    /// step. Mints a non-dying `CallFrame` adopting the run scope (no child) — the scope-dependent
    /// construction the scheduler delegates to the workload — and hands it to the scheduler, which
    /// owns the frame's lifecycle. Idempotent (guarded on `has_run_frame`).
    pub(in crate::machine::execute) fn ensure_run_frame<'a>(&mut self, scope: &'a Scope<'a>) {
        if !self.has_run_frame() {
            // The run-root scope records a `Weak` to the run storage as its `region_owner`; adopting
            // it makes the run frame's region the run-root region (so top-level FN owners resolve).
            let run_storage = scope
                .region_owner()
                .upgrade()
                .expect("run-root scope has a live region owner");
            self.set_run_frame(CallFrame::adopting(scope, run_storage));
        }
    }

    /// Decide a run-scope submission's [`NodeScope`] handle — always cart-witnessed, never anchored
    /// at a free `'run`. Three cases, in order:
    ///
    /// - The active cart's *own* scope is `scope` → [`NodeScope::Yoked`] (re-projected from the cart).
    /// - The active cart's outer-chain reaches `scope`'s region → [`NodeScope::YokedChild`]: `scope` is
    ///   a block scope a builtin allocated in a cart *ancestor* region (an `InScope` body), which the
    ///   cart's `FrameStorage.outer` chain pins. Stored as an erased pointer, reattached frame-bounded.
    /// - No active frame but the `run_frame` (which adopts the run root) *is* `scope` → `Yoked`: the
    ///   slot's cart is the `run_frame` (via [`Self::submission_cart`]'s fallback), so the root
    ///   re-projects from it at the slot's step.
    ///
    /// In production every submission falls into one of these (a body always runs inside a slot step,
    /// so a frame is present; the sole frameless submission is the top-level root). The handle is a
    /// Koan name-resolution concept, built here on the workload, not in the scheduler core.
    pub(in crate::machine::execute) fn resolve_node_scope<'a>(
        &self,
        scope: &'a Scope<'a>,
    ) -> NodeScope {
        if let Some(f) = self.active_frame_ref() {
            if scopes_eq(f.scope(), scope) {
                return NodeScope::Yoked;
            }
            if cart_chain_reaches_region(f.scope(), scope.region) {
                return NodeScope::YokedChild(ErasedScopePtr::erase(scope));
            }
            unreachable!("a framed submission's scope is the cart's own or a cart-ancestor child");
        }
        if self
            .run_frame_ref()
            .is_some_and(|rf| scopes_eq(rf.scope(), scope))
        {
            return NodeScope::Yoked;
        }
        unreachable!("a frameless submission targets the run root adopted by the run frame");
    }

    /// Submit `work` against the executing slot's own [`NodeScope`] handle (read back from the
    /// ambient payload): `YokedChild` re-uses the erased cart-ancestor `ErasedScopePtr` the slot already
    /// holds; `Yoked` re-projects from the active frame cart at the read boundary. The chain defaults
    /// to the ambient one (or a detached chain at top level). Backs the `*_here` re-dispatch path.
    pub(in crate::machine::execute) fn submit_in_own_scope(
        &mut self,
        work: NodeWork<KoanWorkload>,
    ) -> NodeId {
        // The body inherits the slot's own handle and chain (a slot step installs the payload before
        // the body submits), so clone it off the ambient before taking `&mut` for the submit.
        let payload = self
            .active_payload()
            .expect("a slot step installs the ambient payload before the body submits")
            .clone();
        let (cart, framed) = self.submission_cart();
        self.sched.alloc_node(work, payload, cart, framed)
    }

    /// Submit each `statement` as a fresh lexical block over `scope`: mint a frame `(scope_id, i+1)`
    /// per statement (parent = the ambient payload's chain) and dispatch each against `scope`. The
    /// program / REPL / test entry point for a block of top-level statements.
    pub fn enter_block<'a>(
        &mut self,
        scope_id: ScopeId,
        statements: Vec<KExpression<'a>>,
        scope: &'a Scope<'a>,
    ) -> Vec<NodeId> {
        let parent = self.active_payload().map(|p| p.chain.clone());
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
    pub fn dispatch_in_scope<'a>(&mut self, expr: KExpression<'a>, scope: &'a Scope<'a>) -> NodeId {
        let chain = self.ambient_or_detached_chain();
        self.dispatch_in_scope_with_chain(expr, scope, chain)
    }

    /// Submit `expr` against a run-lived `scope`: establish the run frame, decide the slot's
    /// [`NodeScope`] handle against `scope`, then submit. `chain` is the caller's resolved lexical
    /// chain — ambient for [`Self::dispatch_in_scope`], or the per-statement block chain for
    /// [`Self::enter_block`].
    fn dispatch_in_scope_with_chain<'a>(
        &mut self,
        expr: KExpression<'a>,
        scope: &'a Scope<'a>,
        chain: Option<Rc<LexicalFrame>>,
    ) -> NodeId {
        self.ensure_run_frame(scope);
        let node_scope = self.resolve_node_scope(scope);
        self.submit_expression(expr, scope, node_scope, chain)
    }

    /// Dispatch `expr` as a sub-slot of the currently-active per-call frame, storing the slot's
    /// scope as a `Yoked` handle re-projected from the frame cart rather than a fabricated `&'run`.
    /// The caller must have installed the per-call frame as `active_frame` (the run loop does this
    /// per step; [`Self::dispatch_body`] does it transiently). `chain` is the explicit
    /// lexical chain (`Some` for an `enter_block`-routed body statement; the ambient-inheriting
    /// `ActiveFrame` placement passes [`Self::ambient_or_detached_chain`]).
    pub(in crate::machine::execute) fn dispatch_in_active_frame<'a>(
        &mut self,
        expr: KExpression<'a>,
        chain: Option<Rc<LexicalFrame>>,
    ) -> NodeId {
        let frame = self
            .current_frame()
            .expect("in-frame dispatch requires an active frame");
        // `scope_for_bind` is `Rc`-bounded — not a free `'run`-fabrication. The slot stores `Yoked`
        // and re-projects the scope from the frame cart at the read boundary, so this short borrow
        // only needs to outlive the `submit_expression` call.
        let scope = frame.scope_for_bind();
        self.submit_expression(expr, scope, NodeScope::Yoked, chain)
    }

    /// Dispatch `expr` against the executing slot's own scope handle — the honest
    /// re-dispatch-against-my-own-scope path (the `OwnScope` dep placement). A `YokedChild` slot
    /// reuses its erased cart-ancestor pointer; a `Yoked` slot routes through
    /// [`Self::dispatch_in_active_frame`] to re-project from the active frame cart. Either way routes
    /// through [`Self::submit_expression`], so a binder spliced here still installs its placeholder.
    pub(in crate::machine::execute) fn dispatch_in_own_scope<'a>(
        &mut self,
        expr: KExpression<'a>,
    ) -> NodeId {
        let node_scope = self
            .active_payload()
            .expect("a slot step installs the ambient payload before the body submits")
            .scope;
        let chain = self.ambient_or_detached_chain();
        match node_scope {
            NodeScope::YokedChild(_) => {
                // Clone the active cart `Rc` to a local so the reattached borrow is witnessed by an
                // owned handle (decoupled from `&mut self` for the `submit_expression` call below).
                // Routes the single audited reattach in `reattach_node_scope` rather than a second
                // open-coded fabrication — `node_scope`'s `YokedChild` pointer is pinned by the cart's
                // `FrameStorage.outer` chain the held `Rc` keeps alive.
                let cart = self.active_frame_ref().cloned();
                let scope: &Scope<'_> = reattach_node_scope(&node_scope, cart.as_ref());
                self.submit_expression(expr, scope, node_scope, chain)
            }
            NodeScope::Yoked => self.dispatch_in_active_frame(expr, chain),
        }
    }

    /// Dispatch a body's non-tail `statements` as sibling sub-slots in `frame`, each positioned at
    /// body-chain index `i + 1` (the params / `it` sit at idx 0) over the frame's body scope, with
    /// the parent chain reconstructed from the call site via [`assemble_body_chain`]. The shared
    /// "execute a block of expressions" primitive: a multi-statement FN body (`KFunction::invoke`),
    /// a deferred return-type dep, and a MATCH/TRY arm body (the action harness) all use it. The
    /// caller tail-replaces into the body's last statement separately. Returns the sub-slots' ids.
    pub(in crate::machine::execute) fn dispatch_body<'a>(
        &mut self,
        frame: &Rc<CallFrame>,
        statements: Vec<KExpression<'a>>,
    ) -> Vec<NodeId> {
        let body_scope = frame.scope_for_bind();
        let body_scope_id = body_scope.id;
        let parent = assemble_body_chain(
            body_scope,
            self.active_payload()
                .map(|p| p.chain.clone())
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
            let prev = self.swap_active_frame(Some(frame.clone()));
            let bid = self.dispatch_in_active_frame(statement, Some(statement_chain));
            self.swap_active_frame(prev);
            ids.push(bid);
        }
        ids
    }

    /// Schedule an `AwaitDeps` against the executing slot's own scope handle whose finish folds the
    /// resolved deps into a witnessed aggregate carrier (the construction inversion), naming every
    /// region the result reaches on its carrier. `owned_subs` are cascade-freed on success;
    /// `park_producers` are existing siblings the combine reads but does not own. The finish sees
    /// results as `[park_producers..., owned_subs...]`.
    pub(in crate::machine::execute) fn submit_dep_finish_witnessed_in_own_scope<'a>(
        &mut self,
        owned_subs: Vec<NodeId>,
        park_producers: Vec<NodeId>,
        finish: WitnessedDepFinish<'a>,
    ) -> NodeId {
        let park_count = park_producers.len();
        let mut deps = park_producers;
        deps.extend(owned_subs);
        self.submit_in_own_scope(awaiting_witnessed(deps, park_count, finish))
    }
}

/// Test-fixture submission prims that build a run-lifetime [`NodePayload`] from a raw `scope` and
/// the ambient chain, so scheduler tests stand up raw `NodeWork` slots through the harness. The run
/// path routes a `Dispatch` through [`KoanRuntime::submit_expression`] (binder-aware) instead.
#[cfg(test)]
impl<'run> KoanRuntime<'run> {
    /// Generic ambient-chain submission for any `NodeWork`. With no slot step installed (test
    /// fixtures) it synthesizes a detached chain so the visibility predicate treats every scope as
    /// "complete".
    pub(in crate::machine::execute) fn add(
        &mut self,
        work: NodeWork<KoanWorkload>,
        scope: &'run Scope<'run>,
    ) -> NodeId {
        let explicit_chain = self.ambient_or_detached_chain();
        self.add_with_chain(work, scope, explicit_chain)
    }

    /// Run-lifetime submission funnel: establish the run frame, decide the slot's [`NodeScope`]
    /// handle against `scope`, default the chain to the ambient one, and submit the assembled
    /// [`NodePayload`]. `explicit_chain` is `Some` for `enter_block`-routed submissions, `None`
    /// otherwise (inherits the ambient payload's chain).
    pub(in crate::machine::execute) fn add_with_chain(
        &mut self,
        work: NodeWork<KoanWorkload>,
        scope: &'run Scope<'run>,
        explicit_chain: Option<Rc<LexicalFrame>>,
    ) -> NodeId {
        self.ensure_run_frame(scope);
        let scope_handle = self.resolve_node_scope(scope);
        let chain = explicit_chain
            .or_else(|| self.active_payload().map(|p| p.chain.clone()))
            .expect("every dispatched node has a chain — submission outside enter_block / ambient payload is a bug");
        let (cart, framed) = self.submission_cart();
        self.sched.alloc_node(
            work,
            NodePayload {
                scope: scope_handle,
                chain,
            },
            cart,
            framed,
        )
    }

    /// Schedule a dep-finish slot against an explicit `scope`. `owned_subs` are sub-Dispatches this
    /// dep-finish allocated (cascade-freed on success); `park_producers` are existing sibling slots
    /// it splices but does not own. The finish closure sees results as `[park_producers..., owned_subs...]`.
    pub(in crate::machine::execute) fn add_dep_finish(
        &mut self,
        owned_subs: Vec<NodeId>,
        park_producers: Vec<NodeId>,
        scope: &'run Scope<'run>,
        finish: DepFinish<'run>,
    ) -> NodeId {
        let park_count = park_producers.len();
        let mut deps = park_producers;
        deps.extend(owned_subs);
        self.add(awaiting(deps, park_count, finish), scope)
    }
}
