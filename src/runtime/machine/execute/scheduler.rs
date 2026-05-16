use std::rc::Rc;

use crate::runtime::machine::model::KObject;
use crate::runtime::machine::{CallArena, CombineFinish, KError, NodeId, Scope, SchedulerHandle};
use crate::runtime::machine::model::ast::KExpression;

use super::nodes::NodeWork;
use dep_graph::DepGraph;
use node_store::NodeStore;
use work_queues::WorkQueues;

mod dep_graph;
mod dispatch;
mod execute;
mod finish;
mod literal;
mod node_store;
mod submit;
mod work_queues;
#[cfg(test)]
mod run_tests;
#[cfg(test)]
mod tests;

/// A dynamic DAG of dispatch and execution work. The parser submits `Dispatch` nodes for each
/// top-level expression; running a `Dispatch` may add child `Dispatch`/`Bind`/`Combine`
/// nodes, and builtin bodies holding `&mut dyn SchedulerHandle` can also add `Dispatch` nodes.
///
/// The execute loop drains work via [`WorkQueues::pop_next`], which prioritizes in-flight
/// slots (sub-work spawned during another slot's run, plus consumers woken by the
/// notify-walk when a producer's terminal write decrements `pending_deps` to zero) ahead
/// of fresh top-level dispatches (submission order). Cycles are statically prevented
/// because every new node's `NodeId` is strictly greater than every node it can depend
/// on.
///
/// Each node carries the scope it should run against (`Node::scope`). Sub-nodes default to
/// the spawning node's scope; user-fn invocation installs a per-call child scope via
/// `NodeStep::Replace`.
///
/// See design/execution-model.md and design/memory-model.md.
pub struct Scheduler<'a> {
    /// Routing + priority wrapper over the `fresh` and `in_flight` bands. All push/pop
    /// sites go through [`WorkQueues`]'s five named entry points so the routing arm and
    /// drain priority are enforced by the type rather than restated at each call site.
    /// Scoped to `scheduler/` (matches `WorkQueues`'s `pub(super)`); no caller outside
    /// this module touches it.
    pub(in crate::runtime::machine::execute::scheduler) queues: WorkQueues,
    /// Tri-vector dependency state (forward notify edges, pending-deps counters,
    /// backward Owned/Notify edges) bundled behind an enforced surface that
    /// keeps the three vectors in lockstep. See `dep_graph.rs` for the
    /// invariants and the small set of mutation entry points.
    pub(in crate::runtime::machine::execute::scheduler) deps: DepGraph,
    /// Slot table — `nodes`, `results`, `free_list` bundled behind a surface
    /// that keeps the three vectors in lockstep across `alloc_slot ->
    /// take_for_run -> reinstall* -> finalize -> free_one`. See
    /// `node_store.rs` for the invariants and the small set of mutation
    /// entry points. Scope matches `deps` and `queues`; `Scheduler::finalize`
    /// reaches `store.stamp_lift_ready` from a sibling submodule to transition
    /// `NodeWork::Lift(Pending → Ready)` at notify-walk time.
    pub(in crate::runtime::machine::execute::scheduler) store: NodeStore<'a>,
    /// Frame Rc of the slot currently being executed. Read via `SchedulerHandle::current_frame`
    /// so frame-creating builtins (MATCH) can chain it onto their new frame; see
    /// [memory-model.md § Per-call-frame chaining](../../../../design/memory-model.md#per-call-frame-chaining-for-builtin-built-frames).
    pub(in crate::runtime::machine::execute::scheduler) active_frame: Option<Rc<CallArena>>,
}

impl<'a> Scheduler<'a> {
    pub fn new() -> Self {
        Self {
            queues: WorkQueues::new(),
            deps: DepGraph::new(),
            store: NodeStore::new(),
            active_frame: None,
        }
    }

    pub fn len(&self) -> usize { self.store.len() }
    pub fn is_empty(&self) -> bool { self.store.is_empty() }

    /// True iff slot `id` holds a terminal result. An errored sub counts as ready — the
    /// parent short-circuits on it in `run_bind`/`run_combine`.
    pub(in crate::runtime::machine::execute::scheduler) fn is_result_ready(&self, id: NodeId) -> bool {
        self.store.is_result_ready(id)
    }

    /// Retrieve the resolved result for a top-level dispatch. Only safe on IDs returned by
    /// `add_dispatch`; internal slots may have been eagerly freed by their parent.
    pub fn read_result(&self, id: NodeId) -> Result<&'a KObject<'a>, &KError> {
        self.store.read_result(id)
    }

    /// Convenience wrapper for the value-only path: panics on `Err`.
    pub fn read(&self, id: NodeId) -> &'a KObject<'a> {
        self.store.read(id)
    }
}

impl<'a> Default for Scheduler<'a> {
    fn default() -> Self { Self::new() }
}

impl<'a> SchedulerHandle<'a> for Scheduler<'a> {
    fn add_dispatch(&mut self, expr: KExpression<'a>, scope: &'a Scope<'a>) -> NodeId {
        Scheduler::add(self, NodeWork::Dispatch(expr), scope)
    }

    fn add_combine(
        &mut self,
        deps: Vec<NodeId>,
        scope: &'a Scope<'a>,
        finish: CombineFinish<'a>,
    ) -> NodeId {
        Scheduler::add_combine(self, deps, scope, finish)
    }

    /// Active slot's frame `Rc<CallArena>`, set by `execute` for the duration of each
    /// slot's run. Frame-creating builtins (MATCH) clone this Rc into the new frame so the
    /// call-site arena stays alive while the new frame is in use.
    fn current_frame(&self) -> Option<Rc<CallArena>> {
        self.active_frame.clone()
    }

    /// Temporarily install `frame` as the active frame while running `body`. Sub-slots
    /// spawned inside `body` inherit `frame` via the `Scheduler::add` site that reads
    /// `self.active_frame`. The previous `active_frame` is saved and restored on return,
    /// so the caller's slot-tracking invariant survives unchanged.
    fn with_active_frame(
        &mut self,
        frame: std::rc::Rc<crate::runtime::machine::core::CallArena>,
        body: &mut dyn FnMut(&mut dyn SchedulerHandle<'a>),
    ) {
        let prev = self.active_frame.take();
        self.active_frame = Some(frame);
        body(self);
        self.active_frame = prev;
    }
}
