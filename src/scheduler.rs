//! The workload-independent DAG scheduler — a dynamic graph of dependency-linked nodes
//! with per-node memory frames, parameterized over a [`Workload`] and naming no Koan value,
//! error, scope, memory, or AST type.
//!
//! The execute loop drains via [`WorkQueues::pop_next`], which prioritizes in-flight slots
//! (sub-work and notify-walk wakeups) ahead of fresh top-level dispatches. Owned edges never
//! cycle — a new node's `NodeId` is strictly greater than every node it owns. Park (`Notify`)
//! edges can point at an earlier producer, so a self-referential binding (`LET x = x`) forms
//! a cycle that drains with both slots still `PreRun`; the driver detects the leftover parked
//! slots (via [`Scheduler::unresolved`]) and surfaces a deadlock.
//!
//! Generic over a single [`Workload`] `W`: an opaque per-node payload `W::Payload` (persisted across
//! a slot's steps), an inter-node value `W::Value` passed along dep edges, a terminal error
//! `W::Error`, a per-node memory cart `W::Cart` managed by `Rc`, a per-node return `W::Contract`,
//! and a one-shot `W::Continuation`. The scheduler stores all of these and hands them back but
//! inspects none. The Koan interpreter ([`crate::machine`]) is the sole workload; it instantiates
//! the scheduler and drives it through the inherent-method contract.
//!
//! See design/execution/README.md and design/memory-model.md.

use dep_graph::DepGraph;
use node_store::NodeStore;
use nodes::Node;
use work_queues::WorkQueues;

mod alloc;
mod dep_graph;
mod lifecycle;
mod node_id;
mod node_store;
pub mod nodes;
mod splice;
mod work_queues;
mod workload;

// The lifetime-erasure carrier substrate lives in the top-level `witnessed` module (below both
// `machine` and `scheduler`); re-exported here so the scheduler's carriers name it unqualified.
pub(crate) use crate::witnessed::{
    reattach_ref, reattach_ref_with, reattach_with, Erased, MergeWitness, Reattachable, Witnessed,
};
pub use node_id::NodeId;
pub(crate) use workload::{Live, Workload};

/// Re-exported for the driver's white-box reclaim tests (the only cross-module user of the edge
/// kind); production driver code never names it.
#[cfg(test)]
pub(crate) use dep_graph::DepEdge;

/// A dynamic DAG of dispatch and execution work. See the module docs for the queue-priority and
/// cycle-detection contract.
pub(crate) struct Scheduler<W: Workload> {
    pub(in crate::scheduler) queues: WorkQueues,
    pub(in crate::scheduler) deps: DepGraph,
    pub(in crate::scheduler) store: NodeStore<W>,
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
    pub(crate) fn pop_next(&mut self) -> Option<usize> {
        self.queues.pop_next()
    }

    /// Take a slot's stored node to run it (`PreRun` → `Running`); the slot sits empty until the
    /// driver finalizes or [`replace`](Self::replace)s it.
    pub(crate) fn take_for_run(&mut self, id: NodeId) -> Node<W> {
        self.store.take_for_run(id)
    }

    /// Reinstall a tail-replaced slot's node and re-enqueue it if its deps are already satisfied —
    /// the whole `Replace` apply in one step.
    pub(crate) fn replace(&mut self, id: NodeId, node: Node<W>) {
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
    pub(crate) fn unresolved(&self) -> Option<(usize, String)> {
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
    pub(crate) fn is_result_ready(&self, id: NodeId) -> bool {
        self.store.is_result_ready(self.resolve_alias(id))
    }

    /// Only safe on IDs returned by `dispatch_in_scope`; internal slots may have been eagerly
    /// freed by their parent. Follows a bare-name-forward alias to the real producer. The value is
    /// re-anchored to the `&self` borrow — the slot's frame `Rc` pins it for that long.
    pub fn read_result(&self, id: NodeId) -> Result<Live<'_, W>, &W::Error> {
        self.store.read_result(self.resolve_alias(id))
    }

    /// Panics on `Err`. Follows a bare-name-forward alias to the real producer.
    pub fn read(&self, id: NodeId) -> Live<'_, W> {
        self.store.read(self.resolve_alias(id))
    }

    /// The finalized terminal's witness set — the regions it reaches, cloned out for the consumer-pull
    /// lift's `pin` accumulation (the empty set for a frameless / run-region terminal, or for an
    /// errored slot). Follows a bare-name-forward alias to the real producer.
    pub(crate) fn dep_witness(&self, id: NodeId) -> W::Witness {
        self.store.dep_witness(self.resolve_alias(id))
    }

    /// Relocate a finalized terminal into a destination region (the `Forward`-ready pull / drain
    /// re-home) and re-seal it under the set union of the regions it reaches and `dest` — routing
    /// [`Sealed::transfer_into`](crate::witnessed::Sealed::transfer_into), which owns the audited
    /// retype (via [`Witnessed::merge`]). `dest` is the destination region wrapped as a witnessed
    /// carrier; `relocate` is the workload's structural copy into it (it names the value type; the
    /// scheduler does not). Follows a bare-name-forward alias to the real producer (which holds the
    /// value). `None` only if the witness union is not representable — never for a set witness.
    // The rank-2 `relocate` closure plus the witnessed-`Result` return is irreducibly nested.
    #[allow(clippy::type_complexity)]
    pub(crate) fn transfer_lifted<B: Reattachable>(
        &self,
        id: NodeId,
        dest: Witnessed<B, W::Witness>,
        relocate: impl for<'b> FnOnce(
            <W::Value as Reattachable>::At<'b>,
            B::At<'b>,
            std::marker::PhantomData<&'b ()>,
        ) -> <W::Value as Reattachable>::At<'b>,
    ) -> Result<Option<Witnessed<W::Value, W::Witness>>, &W::Error>
    where
        W::Witness: MergeWitness,
    {
        self.store
            .transfer_lifted(self.resolve_alias(id), dest, relocate)
    }

    /// Re-home a finalized terminal (relocated into a surviving region, bundled with the witness set
    /// of any per-call source it still reaches), dropping the pinned producer frame. The drain
    /// boundary uses this for consumer-less roots. Resolves a bare-name alias so the real producer's
    /// frame — not the alias slot — is released.
    pub(crate) fn rehome_terminal(
        &mut self,
        id: NodeId,
        output: Result<Witnessed<W::Value, W::Witness>, W::Error>,
    ) {
        let target = self.resolve_alias(id);
        self.store.rehome_terminal(target, output);
    }

    /// True iff `producer` is forward-reachable from `consumer`
    /// (`DepGraph::would_create_cycle`).
    pub(crate) fn would_create_cycle(&self, producer: NodeId, consumer: NodeId) -> bool {
        self.deps.would_create_cycle(producer, consumer)
    }
}

impl<W: Workload> Default for Scheduler<W> {
    fn default() -> Self {
        Self::new()
    }
}

/// `#[cfg(test)]` forwarders that let the driver's white-box tests poke slot/edge state without
/// exposing the `store` / `deps` / `queues` fields. Each wraps an already-test-only primitive on the
/// inner store or dep graph.
#[cfg(test)]
impl<W: Workload> Scheduler<W> {
    pub(crate) fn clear_node(&mut self, id: NodeId) {
        self.store.clear_node(id);
    }
    pub(crate) fn set_result(&mut self, id: NodeId, output: Result<Live<'_, W>, W::Error>) {
        self.store.set_result(id, output);
    }
    pub(crate) fn result_is_none(&self, id: NodeId) -> bool {
        self.store.result_is_none(id)
    }
    pub(crate) fn result_is_some(&self, id: NodeId) -> bool {
        self.store.result_is_some(id)
    }
    pub(crate) fn is_live(&self, id: NodeId) -> bool {
        self.store.is_live(id)
    }
    pub(crate) fn notify_list_iter(&self) -> impl Iterator<Item = (usize, &Vec<usize>)> {
        self.deps.notify_list_iter()
    }
    pub(crate) fn free_list_snapshot(&self) -> Vec<NodeId> {
        self.store.free_list_snapshot()
    }
    pub(crate) fn free_list_len(&self) -> usize {
        self.store.free_list_len()
    }
    pub(crate) fn set_dep_edges(&mut self, idx: usize, edges: Vec<DepEdge>) {
        self.deps.set_dep_edges(idx, edges);
    }
    pub(crate) fn dep_edges_at(&self, idx: usize) -> &[DepEdge] {
        self.deps.dep_edges_at(idx)
    }
}
