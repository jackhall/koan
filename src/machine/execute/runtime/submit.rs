//! AST-aware dispatch-submission wrappers: each resolves the Koan name-resolution payload
//! `(scope, node_scope, chain)` and forwards to [`KoanRuntime::submit_expression`], the only path
//! that turns a `KExpression` into scheduler work. Payload construction lives here on the workload;
//! the scheduler core exposes only the payload-agnostic [`Scheduler::alloc_node`] and frame-lifecycle
//! accessors.

use std::rc::Rc;

use crate::machine::core::kfunction::action::scope_frame;
use crate::machine::core::{assemble_body_chain, ScopeId, ScopeRefFamily};
use crate::machine::model::ast::KExpression;
use crate::machine::{CallFrame, LexicalFrame, NodeId, Scope};
use crate::witnessed::SealedExtern;

use super::super::dispatch::with_node_scope;

/// Pointer equality of two scopes (identity, not structural).
fn scopes_eq(a: &Scope<'_>, b: &Scope<'_>) -> bool {
    std::ptr::eq(
        a as *const Scope<'_> as *const (),
        b as *const Scope<'_> as *const (),
    )
}

use super::super::nodes::{NodeScope, NodeWork, SlotFrame};
use super::super::outcome::dep_error_frame;
#[cfg(test)]
use super::super::TerminalDepFinish;
use super::super::{seal_witnessed, short_circuit, WitnessedDepFinish};
use super::{KoanRuntime, KoanWorkload};
use crate::scheduler::ResolvedDeps;

/// A bare dep-finish node that waits on the resolved `deps`, short-circuits on the first errored dep
/// under the [`dep_error_frame`] label, else hands the resolved values to a value-only `finish`.
/// Test-only; the run path routes the witnessed [`awaiting_witnessed`].
#[cfg(test)]
fn awaiting(deps: ResolvedDeps, finish: TerminalDepFinish<'_>) -> NodeWork<KoanWorkload> {
    NodeWork::new(deps, short_circuit(Some(dep_error_frame()), finish), None)
}

/// Witnessed sibling of [`awaiting`]: the continuation folds the resolved deps into a witnessed
/// aggregate carrier ([`seal_witnessed`] over [`short_circuit`]) rather than handing out bare values.
fn awaiting_witnessed(
    deps: ResolvedDeps,
    finish: WitnessedDepFinish<'_>,
) -> NodeWork<KoanWorkload> {
    NodeWork::new(
        deps,
        short_circuit(Some(dep_error_frame()), seal_witnessed(finish)),
        None,
    )
}

impl<'run> KoanRuntime<'run> {
    /// The explicit chain when no slot step is installed: a detached chain so the visibility
    /// predicate treats every scope as "complete" (test fixtures / top-level), else `None` to
    /// inherit the ambient payload's chain.
    pub(in crate::machine::execute) fn ambient_or_detached_chain(
        &self,
    ) -> Option<Rc<LexicalFrame>> {
        (!self.has_active_payload()).then(LexicalFrame::detached)
    }

    /// Establish the run frame on the first run-lifetime submission, so every top-level slot carries
    /// a frame cart and `active_frame` is never `None` during a top-level step. Idempotent (guarded
    /// on `has_run_frame`); the scheduler owns the minted frame's lifecycle.
    pub(in crate::machine::execute) fn ensure_run_frame<'a>(&mut self, scope: &'a Scope<'a>) {
        if !self.has_run_frame() {
            // Adopting the run-root scope's `region_owner` storage makes the run frame's region the
            // run-root region, so top-level FN owners resolve.
            let run_storage = scope_frame(scope);
            self.set_run_frame(CallFrame::adopting(scope, run_storage));
        }
    }

    /// Decide a run-scope submission's [`NodeScope`] handle — always cart-witnessed, never anchored
    /// at a free `'run`. Three cases, in order:
    ///
    /// - The active cart's *own* scope is `scope` → [`NodeScope::Yoked`] (re-projected from the cart).
    /// - The active cart's outer-chain reaches `scope`'s region → [`NodeScope::YokedChild`]: `scope` is
    ///   a block scope a builtin allocated in a cart *ancestor* region, pinned by the cart's
    ///   `FrameStorage.outer` chain. Stored erased, reattached frame-bounded.
    /// - No active frame but the `run_frame` (which adopts the run root) *is* `scope` → `Yoked`.
    pub(in crate::machine::execute) fn resolve_node_scope<'a>(
        &self,
        scope: &'a Scope<'a>,
    ) -> NodeScope {
        if let Some(f) = self.active_frame_ref() {
            if f.with_scope(|fs| scopes_eq(fs, scope)) {
                return NodeScope::Yoked;
            }
            if f.with_scope(|fs| fs.chain_reaches_region(scope.region())) {
                return NodeScope::YokedChild(SealedExtern::<ScopeRefFamily>::erase(scope));
            }
            unreachable!("a framed submission's scope is the cart's own or a cart-ancestor child");
        }
        if self
            .run_frame_ref()
            .is_some_and(|rf| rf.with_scope(|rs| scopes_eq(rs, scope)))
        {
            return NodeScope::Yoked;
        }
        unreachable!("a frameless submission targets the run root adopted by the run frame");
    }

    /// Submit `work` against the executing slot's own [`NodeScope`] handle, read back from the
    /// ambient payload. Backs the re-dispatch-against-my-own-scope path.
    pub(in crate::machine::execute) fn submit_in_own_scope(
        &mut self,
        work: NodeWork<KoanWorkload>,
    ) -> NodeId {
        // Clone the payload off the ambient before taking `&mut self` for the submit.
        let payload = self
            .active_payload()
            .expect("a slot step installs the ambient payload before the body submits")
            .clone();
        let (cart, framed) = self.submission_cart();
        let anchor = SlotFrame::new(cart, payload.scope, payload.chain);
        self.sched.alloc_node(work, anchor, framed)
    }

    /// Submit each `statement` as a fresh lexical block over `scope`, minting a frame `(scope_id,
    /// i+1)` per statement. The program / REPL / test entry point for top-level statements.
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
    /// inheriting the ambient (or, at top level, detached) lexical chain. The only public way to
    /// add work.
    pub fn dispatch_in_scope<'a>(&mut self, expr: KExpression<'a>, scope: &'a Scope<'a>) -> NodeId {
        let chain = self.ambient_or_detached_chain();
        self.dispatch_in_scope_with_chain(expr, scope, chain)
    }

    /// Submit `expr` against a run-lived `scope`: establish the run frame, decide the slot's
    /// [`NodeScope`] handle, then submit with the caller's resolved lexical `chain`.
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

    /// Dispatch `expr` as a `Yoked` sub-slot of the currently-active per-call frame. The caller must
    /// have installed the per-call frame as `active_frame` (the run loop does this per step;
    /// [`Self::dispatch_body`] does it transiently).
    pub(in crate::machine::execute) fn dispatch_in_active_frame<'a>(
        &mut self,
        expr: KExpression<'a>,
        chain: Option<Rc<LexicalFrame>>,
    ) -> NodeId {
        let frame = self
            .current_frame()
            .expect("in-frame dispatch requires an active frame");
        // Re-project the scope from the frame cart at a `for<'b>` brand confined to the
        // `submit_expression` call, so no borrow rides up the `&mut self` path.
        frame.with_scope(|scope| self.submit_expression(expr, scope, NodeScope::Yoked, chain))
    }

    /// Dispatch `expr` against the executing slot's own scope handle (the `OwnScope` dep placement).
    /// A `YokedChild` slot reuses its erased cart-ancestor pointer; a `Yoked` slot re-projects via
    /// [`Self::dispatch_in_active_frame`]. Both route through [`Self::submit_expression`], so a binder
    /// spliced here still installs its placeholder.
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
                // Hold the cart `Rc` in a local so the reattach is witnessed by an owned handle: it
                // keeps the cart's `FrameStorage.outer` chain alive while `with_node_scope` opens the
                // `YokedChild` pointer at a `for<'b>` brand, so no borrow escapes the call.
                let cart = self.active_frame_ref().cloned();
                with_node_scope(&node_scope, cart.as_ref(), |scope| {
                    self.submit_expression(expr, scope, node_scope, chain)
                })
            }
            NodeScope::Yoked => self.dispatch_in_active_frame(expr, chain),
        }
    }

    /// Dispatch a body's non-tail `statements` as sibling sub-slots in `frame`, each at body-chain
    /// index `i + 1` (params / `it` sit at idx 0) over the frame's body scope, with the parent chain
    /// reconstructed from the call site via [`assemble_body_chain`]. The shared "execute a block of
    /// expressions" primitive (FN body, deferred return-type dep, MATCH/TRY arm body); the caller
    /// tail-replaces into the last statement separately.
    pub(in crate::machine::execute) fn dispatch_body<'a>(
        &mut self,
        frame: &Rc<CallFrame>,
        statements: Vec<KExpression<'a>>,
    ) -> Vec<NodeId> {
        let call_site_chain = self
            .active_payload()
            .map(|p| p.chain.clone())
            .expect("a body block runs inside an active lexical chain");
        // Open the body scope at a `for<'b>` brand: the id copies out and the chain returns as an
        // unbranded `Rc`, so nothing branded escapes the read.
        let (body_scope_id, parent) = frame.with_scope(|body_scope| {
            (
                body_scope.id,
                assemble_body_chain(body_scope, call_site_chain, 0)
                    .parent
                    .clone(),
            )
        });
        let mut ids = Vec::with_capacity(statements.len());
        for (i, statement) in statements.into_iter().enumerate() {
            let statement_chain = LexicalFrame::push(parent.clone(), body_scope_id, i + 1);
            // Bracket `frame` as the ambient cart so the sub-slot inherits it (not the caller's),
            // restoring the previous on every exit path.
            let bid = self.with_active_frame(Rc::clone(frame), |rt| {
                rt.dispatch_in_active_frame(statement, Some(statement_chain))
            });
            ids.push(bid);
        }
        ids
    }

    /// Schedule an `AwaitDeps` against the slot's own scope whose finish folds the resolved deps into
    /// a witnessed aggregate carrier, naming every region the result reaches. `deps` carries park
    /// producers (read, not owned) and owned subs (cascade-freed on success); the finish addresses
    /// them through a [`DepResults`](crate::scheduler::DepResults) view.
    pub(in crate::machine::execute) fn submit_dep_finish_witnessed_in_own_scope<'a>(
        &mut self,
        deps: ResolvedDeps,
        finish: WitnessedDepFinish<'a>,
    ) -> NodeId {
        self.submit_in_own_scope(awaiting_witnessed(deps, finish))
    }
}

/// Test-fixture submission prims that mint a run-lifetime [`SlotFrame`] anchor from a raw `scope`, so
/// scheduler tests stand up raw `NodeWork` slots through the harness. The run path routes a
/// `Dispatch` through [`KoanRuntime::submit_expression`] instead.
#[cfg(test)]
impl<'run> KoanRuntime<'run> {
    /// Ambient-chain submission for any `NodeWork`; with no slot step installed it synthesizes a
    /// detached chain so the visibility predicate treats every scope as "complete".
    pub(in crate::machine::execute) fn add(
        &mut self,
        work: NodeWork<KoanWorkload>,
        scope: &'run Scope<'run>,
    ) -> NodeId {
        let explicit_chain = self.ambient_or_detached_chain();
        self.add_with_chain(work, scope, explicit_chain)
    }

    /// Run-lifetime submission funnel: establish the run frame, decide the slot's [`NodeScope`]
    /// handle, default the chain to the ambient one, and submit the assembled [`SlotFrame`] anchor.
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
        let anchor = SlotFrame::new(cart, scope_handle, chain);
        self.sched.alloc_node(work, anchor, framed)
    }

    /// Schedule a dep-finish slot against an explicit `scope`. `deps` carries the owned sub-Dispatches
    /// (cascade-freed on success) and any park producers (read, not owned).
    pub(in crate::machine::execute) fn add_dep_finish(
        &mut self,
        deps: ResolvedDeps,
        scope: &'run Scope<'run>,
        finish: TerminalDepFinish<'run>,
    ) -> NodeId {
        self.add(awaiting(deps, finish), scope)
    }
}
