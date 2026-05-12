//! Tri-vector dependency-graph state pulled out of `Scheduler<'a>`. The three
//! parallel vectors — `notify_list`, `pending_deps`, `dep_edges` — encode an
//! invariant nothing in the type system enforced before this module:
//! every forward edge in `notify_list[p]` has a matching backward entry in
//! `dep_edges[c]` and contributes 1 to `pending_deps[c]`.
//!
//! ## Invariants this module enforces
//!
//! **Inv-A (wake-pending coherence).** For every consumer slot `c`,
//! `pending_deps[c] == |{ p : c appears in notify_list[p] }|`. Every public
//! method that mutates `notify_list[*]` or `pending_deps[*]` does so in a
//! single atomic body — no method can desync one of those two fields without
//! the other. The forbidden shape is a half-mutation method like
//! `install_wake(p, c)` that pushes to `notify_list` and increments
//! `pending_deps` without writing `dep_edges`; that would re-create the
//! deferred-fixup gap the roadmap item exists to close.
//!
//! **Inv-B (free-cascade source).** `dep_edges[c]` lists every Owned sub-slot
//! `c` must cascade-reclaim. Park edges are tagged `Notify` and filtered out
//! of `free`'s walk via `owned_children`. Independent of Inv-A.
//!
//! **Inv-C (lazy notify-scrub on free).** A slot `c` is only freed once
//! every producer's `drain_notify` has run and removed `c` from
//! `notify_list[*]`. The `freed_slot_does_not_appear_in_other_notify_lists`
//! test pins this. `free` doesn't have to scrub — it relies on Inv-A and
//! Inv-C still holding.

use crate::runtime::machine::NodeId;
use super::super::nodes::{NodeWork, work_deps};

/// A backward edge stored in `dep_edges[consumer]`. `Owned` marks slots the
/// consumer is responsible for reclaiming (sub-Dispatches a Bind spawned, the
/// producer a Lift wraps); `Notify` marks producer slots the consumer only
/// parked on for wake notification (bare-name short-circuit, replay-park).
/// Both kinds install a wake edge in `notify_list[producer]`; the kind
/// distinction matters only at reclaim time (`free` recurses only into
/// `Owned`, so the reclaim walk cannot transit through park edges into
/// unrelated slot graphs).
#[derive(Copy, Clone, Debug)]
pub(in crate::runtime::machine::execute) enum DepEdge {
    Owned(NodeId),
    Notify(NodeId),
}

impl DepEdge {
    /// Read the producer slot index regardless of edge kind.
    pub(in crate::runtime::machine::execute) fn node_id(self) -> NodeId {
        match self {
            DepEdge::Owned(id) | DepEdge::Notify(id) => id,
        }
    }
}

/// Owned-edge sidecar populated at `add()` time: every dep `work_deps` reports
/// comes from the work's own subs/deps/from field, so the spawning slot owns
/// it. `Dispatch` produces an empty list. Notify edges (bare-name
/// short-circuit, replay-park) are not produced here — they're pushed at the
/// call site in `run_dispatch` via `add_park_edge`.
pub(super) fn work_owned_edges<'a>(work: &NodeWork<'a>) -> Vec<DepEdge> {
    match work_deps(work) {
        Some(ids) => ids.into_iter().map(DepEdge::Owned).collect(),
        None => Vec::new(),
    }
}

/// Tri-vector dependency state. All three vectors are 1:1 with `Scheduler::nodes`
/// indices. Mutation is restricted to the small set of methods below; every
/// method preserves Inv-A atomically (or is a read-only access).
pub(in crate::runtime::machine::execute) struct DepGraph {
    /// Forward edges (producer -> consumer slot indices). Cleared on `free`'s
    /// implicit drain (consumers are scrubbed before free; see Inv-C) so a
    /// reused slot doesn't inherit phantom edges.
    notify_list: Vec<Vec<usize>>,
    /// Count of deps whose terminal result hasn't yet been observed by this
    /// slot's notify-decrement. Reaches zero -> slot routed via
    /// `WorkQueues::push_woken`.
    pending_deps: Vec<usize>,
    /// Backward edges (consumer -> producer slots), tagged by kind.
    /// `DepEdge::Owned` marks a sub-slot this slot is responsible for
    /// reclaiming (Bind subs, Combine deps, Lift's `from`); `DepEdge::Notify`
    /// marks a sibling producer this slot only parked on for wake
    /// notification. `notify_list` is the forward analogue; `free()` walks
    /// this sidecar but recurses only into `Owned` so park edges can never
    /// transit the reclaim walk into unrelated slots. Cleared by `run_bind` /
    /// `run_combine` after they eagerly free their deps on the success path.
    dep_edges: Vec<Vec<DepEdge>>,
}

impl DepGraph {
    pub(super) fn new() -> Self {
        Self {
            notify_list: Vec::new(),
            pending_deps: Vec::new(),
            dep_edges: Vec::new(),
        }
    }

    /// Atomic init of all three vectors when extending the node-space by one
    /// slot. `pending_producers` is the subset of `owned_edges`'s `node_id()`s
    /// whose producers are not yet terminal; the caller pre-filters via
    /// `Scheduler::is_result_ready` so `DepGraph` stays oblivious to results
    /// storage (phase 3 can move results to `NodeStore` without touching this
    /// surface). Returns the installed pending count so the caller can decide
    /// enqueue routing.
    pub(super) fn extend_for_new_slot(
        &mut self,
        consumer: NodeId,
        owned_edges: Vec<DepEdge>,
        pending_producers: &[NodeId],
    ) -> usize {
        self.notify_list.push(Vec::new());
        self.pending_deps.push(pending_producers.len());
        self.dep_edges.push(owned_edges);
        for p in pending_producers {
            self.notify_list[p.index()].push(consumer.index());
        }
        pending_producers.len()
    }

    /// Atomic reset of all three vectors for a recycled slot. Same semantics
    /// as `extend_for_new_slot` but writing into existing indices.
    pub(super) fn reset_slot_deps(
        &mut self,
        consumer: NodeId,
        owned_edges: Vec<DepEdge>,
        pending_producers: &[NodeId],
    ) -> usize {
        self.notify_list[consumer.index()].clear();
        self.pending_deps[consumer.index()] = pending_producers.len();
        self.dep_edges[consumer.index()] = owned_edges;
        for p in pending_producers {
            self.notify_list[p.index()].push(consumer.index());
        }
        pending_producers.len()
    }

    /// Atomic +1 across all three vectors for a mid-run owned dep. Caller
    /// guarantees `producer` is not yet terminal at install time (see audit
    /// in the roadmap plan).
    pub(in crate::runtime::machine::execute) fn add_owned_edge(
        &mut self,
        producer: NodeId,
        consumer: NodeId,
    ) {
        self.notify_list[producer.index()].push(consumer.index());
        self.pending_deps[consumer.index()] += 1;
        self.dep_edges[consumer.index()].push(DepEdge::Owned(producer));
    }

    /// Atomic +1 across all three vectors for a mid-run park edge. The
    /// backward entry is `Notify(producer)` so `free` skips past it; the
    /// forward wake on `notify_list[producer]` is identical to an owned edge.
    /// Caller guarantees `producer` is not yet terminal.
    pub(in crate::runtime::machine::execute) fn add_park_edge(
        &mut self,
        producer: NodeId,
        consumer: NodeId,
    ) {
        self.notify_list[producer.index()].push(consumer.index());
        self.pending_deps[consumer.index()] += 1;
        self.dep_edges[consumer.index()].push(DepEdge::Notify(producer));
    }

    /// Atomic batch decrement across the wake-pending pair. Drains
    /// `notify_list[producer_idx]` and returns the consumers whose
    /// `pending_deps` hit zero — the caller routes those through
    /// `WorkQueues::push_woken`.
    pub(super) fn drain_notify(&mut self, producer_idx: usize) -> Vec<usize> {
        let notifees = std::mem::take(&mut self.notify_list[producer_idx]);
        let mut woken = Vec::new();
        for consumer in notifees {
            self.pending_deps[consumer] -= 1;
            if self.pending_deps[consumer] == 0 {
                woken.push(consumer);
            }
        }
        woken
    }

    /// Free-cascade source for `Scheduler::free`. Drains `dep_edges[idx]`
    /// (so a repeat free is a no-op) and yields only `Owned` children;
    /// `Notify` edges are dropped so the reclaim walk cannot transit into
    /// the producer's subtree.
    pub(super) fn owned_children(&mut self, idx: usize) -> impl Iterator<Item = NodeId> {
        let edges = std::mem::take(&mut self.dep_edges[idx]);
        edges.into_iter().filter_map(|e| match e {
            DepEdge::Owned(id) => Some(id),
            DepEdge::Notify(_) => None,
        })
    }

    /// Eager-free on the success path. Inv-C ensures `notify_list[idx]` is
    /// already drained by the time the caller hits this — the notify-walk
    /// runs before any consumer reclaims its deps.
    pub(in crate::runtime::machine::execute) fn clear_dep_edges(&mut self, idx: usize) {
        self.dep_edges[idx].clear();
    }

    /// Pure read of the parked-dep counter for the Replace branch.
    pub(super) fn pending_count(&self, idx: usize) -> usize {
        self.pending_deps[idx]
    }

    /// Pure read for `Scheduler::free`'s early-skip guard: paired with
    /// `results[i].is_none()` to detect already-reclaimed slots and avoid a
    /// duplicate `free_list` push.
    pub(super) fn is_dep_edges_empty(&self, idx: usize) -> bool {
        self.dep_edges[idx].is_empty()
    }

    // --- Test-only accessors for direct synthetic-state setup in tests. ---

    #[cfg(test)]
    pub(super) fn dep_edges_at(&self, idx: usize) -> &[DepEdge] {
        &self.dep_edges[idx]
    }

    #[cfg(test)]
    pub(super) fn set_dep_edges(&mut self, idx: usize, edges: Vec<DepEdge>) {
        self.dep_edges[idx] = edges;
    }

    #[cfg(test)]
    pub(super) fn notify_list_iter(&self) -> impl Iterator<Item = (usize, &Vec<usize>)> {
        self.notify_list.iter().enumerate()
    }
}
