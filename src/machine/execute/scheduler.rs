use crate::machine::NodeId;

use super::nodes::Node;
use dep_graph::DepGraph;
use node_store::NodeStore;
use work_queues::WorkQueues;

pub(in crate::machine::execute) use workload::{FramedRead, Workload};

mod dep_graph;
mod execute;
mod finish;
mod node_store;
#[cfg(test)]
mod run_tests;
mod splice;
mod submit;
#[cfg(test)]
mod tests;
mod work_queues;
mod workload;

/// A dynamic DAG of dispatch and execution work.
///
/// The execute loop drains via [`WorkQueues::pop_next`], which prioritizes in-flight slots
/// (sub-work and notify-walk wakeups) ahead of fresh top-level dispatches. Owned edges never
/// cycle — a new node's `NodeId` is strictly greater than every node it owns. Park (`Notify`)
/// edges can point at an earlier producer, so a self-referential binding (`LET x = x`) forms
/// a cycle that drains with both slots still `PreRun`; `execute` detects the leftover parked
/// slots and returns `KErrorKind::SchedulerDeadlock`.
///
/// Generic over a single [`Workload`] `W`: an opaque per-node payload `W::Payload` (persisted across
/// a slot's steps; Koan: scope handle + lexical chain), an inter-node value `W::Value` passed along
/// dep edges (Koan: the lifted `Carried`), a terminal error `W::Error`, and a per-node memory frame
/// `W::Frame` it manages by `Rc`. The scheduler stores all four and hands them back but inspects
/// none — it names no Koan value, error, scope, memory, or AST type. The Koan instantiation is
/// `KoanWorkload`; the Koan workload carries the scope a node runs against in its payload, sub-nodes
/// default to the spawning node's payload, and a user-fn invocation installs a per-call child via
/// `NodeStep::Replace`.
///
/// See design/execution-model.md and design/memory-model.md.
pub(in crate::machine::execute) struct Scheduler<W: Workload> {
    pub(in crate::machine::execute::scheduler) queues: WorkQueues,
    pub(in crate::machine::execute::scheduler) deps: DepGraph,
    pub(in crate::machine::execute::scheduler) store: NodeStore<W>,
}

impl<W: Workload> Scheduler<W> {
    pub fn new() -> Self {
        Self {
            queues: WorkQueues::new(),
            deps: DepGraph::new(),
            store: NodeStore::new(),
        }
    }

    /// Pop the next ready slot index — the run loop's iterator (in-flight slots ahead of fresh
    /// dispatches). `None` when the queue drains.
    pub(in crate::machine::execute) fn pop_next(&mut self) -> Option<usize> {
        self.queues.pop_next()
    }

    /// Take a slot's stored node to run it (`PreRun` → `Running`); the slot sits empty until the
    /// driver finalizes or [`replace`](Self::replace)s it.
    pub(in crate::machine::execute) fn take_for_run(&mut self, id: NodeId) -> Node<W> {
        self.store.take_for_run(id)
    }

    /// Reinstall a tail-replaced slot's node and re-enqueue it if its deps are already satisfied —
    /// the whole `NodeStep::Replace` apply in one step.
    pub(in crate::machine::execute) fn replace(&mut self, id: NodeId, node: Node<W>) {
        self.store.reinstall(id, node);
        // Replace return sites install their own edges (or clear the slot's dep edges for tail
        // rewrites), so the pending count is authoritative here.
        if self.deps.pending_count(id.index()) == 0 {
            self.queues.push_after_replace(id.index());
        }
    }

    /// Slots still `PreRun` after the queue drained — each is parked on a dependency that can no
    /// longer fire (a dependency cycle). `(count, sample)` for the deadlock error, or `None` when
    /// every slot is terminal.
    pub(in crate::machine::execute) fn unresolved(&self) -> Option<(usize, String)> {
        self.store.unresolved()
    }

    /// The live slot's opaque workload payload, or `None` once it has terminalized — at which point
    /// `take_for_run` has moved the payload out. Test-only; the workload extracts the field it wants.
    #[cfg(test)]
    pub fn payload_of(&self, id: NodeId) -> Option<&W::Payload> {
        self.store.payload_of(id)
    }

    pub fn len(&self) -> usize {
        self.store.len()
    }
    pub fn is_empty(&self) -> bool {
        self.store.is_empty()
    }

    /// An errored sub counts as ready — parents short-circuit on it. Follows a bare-name-forward
    /// alias to the real producer (see [`splice`](self::splice)).
    pub(in crate::machine::execute) fn is_result_ready(&self, id: NodeId) -> bool {
        self.store.is_result_ready(self.resolve_alias(id))
    }

    /// Only safe on IDs returned by `dispatch_in_scope`; internal slots may have been eagerly
    /// freed by their parent. Follows a bare-name-forward alias to the real producer.
    pub fn read_result(&self, id: NodeId) -> Result<W::Value, &W::Error> {
        self.store.read_result(self.resolve_alias(id))
    }

    /// Panics on `Err`. Follows a bare-name-forward alias to the real producer.
    pub fn read(&self, id: NodeId) -> W::Value {
        self.store.read(self.resolve_alias(id))
    }

    /// Read a terminal with the producer frame `Rc` backing it, for the consumer-pull lift. Follows
    /// a bare-name-forward alias to the real producer (which holds the value in its own frame).
    pub(in crate::machine::execute) fn read_result_with_frame(
        &self,
        id: NodeId,
    ) -> FramedRead<'_, W> {
        self.store.read_result_with_frame(self.resolve_alias(id))
    }

    /// Re-home a finalized terminal (already lifted into a surviving arena), dropping the pinned
    /// producer frame. The drain boundary uses this for consumer-less roots. Resolves a bare-name
    /// alias so the real producer's frame — not the alias slot — is released.
    pub(in crate::machine::execute) fn rehome_terminal(
        &mut self,
        id: NodeId,
        output: Result<W::Value, W::Error>,
    ) {
        let target = self.resolve_alias(id);
        self.store.rehome_terminal(target, output);
    }

    // ----- Narrow dispatcher-facing surface (pub(in execute)) -----
    //
    // These methods are the dispatcher's named contract with the scheduler:
    // the read view (`SchedulerView`) and the write harness route through them,
    // so the storage layout (`deps` / `store` / `queues` / `active_*` fields)
    // stays scheduler-internal.

    // `add_owned_edge` / `add_park_edge` (the alias-resolving edge installs) and the splice itself
    // live in [`splice`](self::splice), the one home for the bare-name-forward graph logic.

    /// True iff `producer` is forward-reachable from `consumer`
    /// (`DepGraph::would_create_cycle`).
    pub(in crate::machine::execute) fn would_create_cycle(
        &self,
        producer: NodeId,
        consumer: NodeId,
    ) -> bool {
        self.deps.would_create_cycle(producer, consumer)
    }
}

impl<W: Workload> Default for Scheduler<W> {
    fn default() -> Self {
        Self::new()
    }
}
