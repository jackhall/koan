use std::rc::Rc;

use crate::machine::model::ast::KExpression;
use crate::machine::{LexicalFrame, NodeId, Scope};

use super::super::harness::KoanHarness;
use super::super::nodes::{work_park_producers, CallFrame, Node, NodeScope, NodeWork};
use super::super::CombineFinish;
use super::dep_graph::work_owned_edges;
use super::Scheduler;

impl<'run> Scheduler<'run> {
    /// The explicit chain a submission passes when there is no ambient `active_chain`: a detached
    /// chain so the visibility predicate treats every scope as "complete" (test fixtures /
    /// top-level), else `None` to inherit the ambient one.
    pub(in crate::machine::execute) fn ambient_or_detached_chain(
        &self,
    ) -> Option<Rc<LexicalFrame>> {
        self.active_chain.is_none().then(LexicalFrame::detached)
    }

    /// Schedule a `Combine` slot against an explicit `scope`. `owned_subs` are sub-Dispatches
    /// this Combine allocated (cascade-freed on success); `park_producers` are existing sibling
    /// slots it splices but does not own (kept alive past success via `Notify` edges). The finish
    /// closure sees results as `[park_producers..., owned_subs...]`. Test fixture entry point; the
    /// run path uses [`KoanHarness::combine_here`](super::super::harness::KoanHarness::combine_here).
    #[cfg(test)]
    pub(in crate::machine::execute) fn add_combine(
        &mut self,
        owned_subs: Vec<NodeId>,
        park_producers: Vec<NodeId>,
        scope: &'run Scope<'run>,
        finish: CombineFinish<'run>,
    ) -> NodeId {
        let park_count = park_producers.len();
        let mut deps = park_producers;
        deps.extend(owned_subs);
        self.add(NodeWork::combine(deps, park_count, finish), scope)
    }

    /// Generic ambient-chain submission for any `NodeWork` — a test fixture entry point. When
    /// there is no ambient chain (test fixtures) it synthesizes a detached chain so the visibility
    /// predicate treats every scope as "complete". The run path submits a `Dispatch` through
    /// [`KoanHarness::submit_dispatch`](super::super::harness::KoanHarness::submit_dispatch)
    /// (binder-aware) and a `Combine`/`Catch` through `KoanHarness::combine_here` / the harness.
    #[cfg(test)]
    pub(in crate::machine::execute::scheduler) fn add(
        &mut self,
        work: NodeWork<'run>,
        scope: &'run Scope<'run>,
    ) -> NodeId {
        let explicit_chain = self.ambient_or_detached_chain();
        self.add_with_chain(work, scope, explicit_chain)
    }

    /// Run-lifetime submission funnel. `explicit_chain` is `Some` for
    /// `enter_block`-routed submissions (top-level, MODULE / SIG body, TRY body
    /// success-as-block, FN body invoke), `None` otherwise (inherits
    /// `self.active_chain`). Decides the slot's [`NodeScope`] handle — `Yoked` when this
    /// runs inside the per-call frame whose own child is `scope` (re-projected from the
    /// cart), else `Root` — then hands off to [`Self::submit_node`]. Test fixture entry; the run
    /// path routes a `Dispatch` through `KoanHarness::submit_dispatch`.
    #[cfg(test)]
    pub(super) fn add_with_chain(
        &mut self,
        work: NodeWork<'run>,
        scope: &'run Scope<'run>,
        explicit_chain: Option<Rc<LexicalFrame>>,
    ) -> NodeId {
        self.ensure_run_frame(scope);
        let node_scope = self.resolve_node_scope(scope);
        self.submit_node(work, node_scope, explicit_chain)
    }

    /// Establish the run frame on the first run-lifetime submission (top-level run scope), so every
    /// top-level slot carries a frame cart and `active_frame` is never `None` during a top-level
    /// step. Adopts the passed scope without minting a child. Idempotent.
    pub(super) fn ensure_run_frame(&mut self, scope: &'run Scope<'run>) {
        if self.run_frame.is_none() {
            self.run_frame = Some(crate::machine::CallArena::adopting(scope));
        }
    }

    /// Decide a run-scope submission's [`NodeScope`] handle: `Yoked` when this runs inside the
    /// per-call frame whose own child is `scope` (re-projected from the cart at the read boundary —
    /// no fabricated `&'run` persisted), else `Anchored` at `'run`.
    pub(super) fn resolve_node_scope(&self, scope: &'run Scope<'run>) -> NodeScope<'run> {
        match &self.active_frame {
            Some(f)
                if std::ptr::eq(
                    f.scope() as *const Scope<'_> as *const (),
                    scope as *const Scope<'_> as *const (),
                ) =>
            {
                NodeScope::Yoked
            }
            _ => NodeScope::Anchored(scope),
        }
    }

    /// Submit `work` against the executing slot's own [`NodeScope`] handle (`active_node_scope`):
    /// `Anchored(&'run)` re-uses the genuine run-lived borrow the slot already holds; `Yoked`
    /// re-projects from the active frame cart at the read boundary. Backs the `*_here` methods —
    /// the honest re-dispatch-against-my-own-scope path.
    pub(super) fn submit_here(&mut self, work: NodeWork<'run>) -> NodeId {
        let node_scope = self
            .active_node_scope
            .expect("a slot step installs active_node_scope before the body submits");
        let explicit_chain = self.ambient_or_detached_chain();
        self.submit_node(work, node_scope, explicit_chain)
    }

    /// Node-creation core, shared by the run-lifetime [`Self::add_with_chain`] and the framed
    /// [`KoanHarness::add_dispatch_in_frame`](super::super::harness::KoanHarness::add_dispatch_in_frame).
    /// `scope` is read only transiently
    /// (binder-install, placeholder install, `pre_subs` recursion) and never retained — the
    /// node keeps a `NodeScope<'run>` handle, not this borrow — so it is clamped to a `'step`
    /// read: a run scope and a `scope_for_bind` re-projection both shorten into it.
    /// `node_scope` is the pre-decided slot handle the caller built (`Root` at `'run` for a run
    /// scope, `Yoked` for a framed one).
    pub(in crate::machine::execute) fn submit_node(
        &mut self,
        work: NodeWork<'run>,
        node_scope: NodeScope<'run>,
        explicit_chain: Option<Rc<LexicalFrame>>,
    ) -> NodeId {
        // A binder-shaped Dispatch arrives with its `pre_subs` already populated and its
        // placeholder already installed by `dispatch::submit_dispatch`; this allocator never
        // inspects the work's AST.
        let chain = explicit_chain.or_else(|| self.active_chain.clone()).expect(
            "every dispatched node has a chain — submission outside enter_block / \
                 ambient active_chain is a bug",
        );
        let owned_edges = work_owned_edges(&work);
        let no_owned = owned_edges.is_empty();
        // Top-level submissions (no active frame) fall back to the run frame, so every slot
        // carries a cart and `active_frame` is `Some` during its step. `run_frame` is
        // established by `add_with_chain` before the first submission, so the fallback is
        // always `Some` — the cart is non-optional node state.
        let cart = self.active_frame.clone().unwrap_or_else(|| {
            self.run_frame
                .clone()
                .expect("run_frame established by add_with_chain before any submission")
        });
        let pending_owned: Vec<NodeId> = owned_edges
            .iter()
            .map(|e| e.node_id())
            .filter(|p| !self.is_result_ready(*p))
            .collect();
        let pending_park: Vec<NodeId> = work_park_producers(&work)
            .iter()
            .copied()
            .filter(|p| !self.is_result_ready(*p))
            .collect();
        let no_park = work_park_producers(&work).is_empty();
        let id = self.store.alloc_slot(Node {
            work,
            scope: node_scope,
            frame: CallFrame {
                cart,
                reserve: None,
                contract: None,
            },
            chain,
        });
        self.deps.install_for_slot(id, owned_edges, &pending_owned);
        for p in &pending_park {
            self.deps.add_park_edge(*p, id);
        }
        if pending_owned.is_empty() && pending_park.is_empty() {
            if self.active_frame.is_none() && no_owned && no_park {
                self.queues.push_fresh(id.index());
            } else {
                self.queues.push_in_flight_submit(id.index());
            }
        }
        id
    }
}

impl<'run> KoanHarness<'run> {
    /// Submit an unresolved expression for the scheduler to dispatch + execute
    /// against `scope`. The only public way to add work.
    pub fn add_dispatch(&mut self, expr: KExpression<'run>, scope: &'run Scope<'run>) -> NodeId {
        let explicit_chain = self.sched.ambient_or_detached_chain();
        self.sched.ensure_run_frame(scope);
        let node_scope = self.sched.resolve_node_scope(scope);
        self.submit_dispatch(expr, scope, node_scope, explicit_chain)
    }

    /// Dispatch `expr` against the executing slot's own scope handle — the honest
    /// re-dispatch-against-my-own-scope path (the `OwnScope` dep placement). Routes through
    /// [`Self::submit_dispatch`] so a binder spliced here still installs its placeholder; the
    /// concrete scope is materialized from the handle (`Anchored`'s borrow / a `Yoked` frame's
    /// `scope_for_bind`).
    pub(in crate::machine::execute) fn dispatch_here(&mut self, expr: KExpression<'run>) -> NodeId {
        let node_scope = self
            .sched
            .active_node_scope
            .expect("a slot step installs active_node_scope before the body submits");
        let explicit_chain = self.sched.ambient_or_detached_chain();
        match node_scope {
            NodeScope::Anchored(scope) => {
                self.submit_dispatch(expr, scope, node_scope, explicit_chain)
            }
            NodeScope::Yoked => {
                let frame = self
                    .sched
                    .active_frame
                    .clone()
                    .expect("a Yoked slot step has an active frame");
                let scope = frame.scope_for_bind();
                self.submit_dispatch(expr, scope, NodeScope::Yoked, explicit_chain)
            }
        }
    }

    /// Schedule a `Combine` against the executing slot's own scope handle.
    pub(in crate::machine::execute) fn combine_here(
        &mut self,
        owned_subs: Vec<NodeId>,
        park_producers: Vec<NodeId>,
        finish: CombineFinish<'run>,
    ) -> NodeId {
        let park_count = park_producers.len();
        let mut deps = park_producers;
        deps.extend(owned_subs);
        self.sched
            .submit_here(NodeWork::combine(deps, park_count, finish))
    }
}

/// Test-fixture forwarders for the AST-free submission prims that stay on [`Scheduler`]
/// (`add` / `add_with_chain` / `add_combine`), so scheduler tests build raw `NodeWork` slots
/// through the harness without naming the scheduler.
#[cfg(test)]
impl<'run> KoanHarness<'run> {
    pub(in crate::machine::execute) fn add(
        &mut self,
        work: NodeWork<'run>,
        scope: &'run Scope<'run>,
    ) -> NodeId {
        self.sched.add(work, scope)
    }

    pub(in crate::machine::execute) fn add_with_chain(
        &mut self,
        work: NodeWork<'run>,
        scope: &'run Scope<'run>,
        explicit_chain: Option<Rc<LexicalFrame>>,
    ) -> NodeId {
        self.sched.add_with_chain(work, scope, explicit_chain)
    }

    pub(in crate::machine::execute) fn add_combine(
        &mut self,
        owned_subs: Vec<NodeId>,
        park_producers: Vec<NodeId>,
        scope: &'run Scope<'run>,
        finish: CombineFinish<'run>,
    ) -> NodeId {
        self.sched
            .add_combine(owned_subs, park_producers, scope, finish)
    }
}
