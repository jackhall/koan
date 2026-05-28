//! Tri-vector dependency-graph state pulled out of `Scheduler<'a>`. The three
//! parallel vectors — `notify_list`, `pending_deps`, `dep_edges` — share an
//! index space with `Scheduler::nodes` and uphold three invariants:
//!
//! **Inv-A (wake-pending coherence).** For every consumer slot `c`,
//! `pending_deps[c] == |{ p : c appears in notify_list[p] }|`. Every mutating
//! method updates `notify_list`, `pending_deps`, and `dep_edges` in a single
//! atomic body so the two fields cannot desync.
//!
//! **Inv-B (free-cascade source).** `dep_edges[c]` lists every Owned sub-slot
//! `c` must cascade-reclaim. Park edges are tagged `Notify` and filtered out
//! of `free`'s walk via `owned_children`. Independent of Inv-A.
//!
//! **Inv-C (lazy notify-scrub on free).** A slot `c` is only freed once
//! every producer's `drain_notify` has run and removed `c` from
//! `notify_list[*]`. The `freed_slot_does_not_appear_in_other_notify_lists`
//! test pins this; `free` relies on Inv-A and Inv-C still holding rather than
//! scrubbing itself.

use crate::machine::NodeId;
use super::super::nodes::{NodeWork, work_deps};

/// A backward edge stored in `dep_edges[consumer]`. Both kinds install the
/// same forward wake edge in `notify_list[producer]`; the kind distinction
/// matters only at reclaim time, where `free` recurses into `Owned` children
/// but stops at `Notify` so the walk cannot transit into unrelated subgraphs.
#[derive(Copy, Clone, Debug)]
pub(super) enum DepEdge {
    Owned(NodeId),
    Notify(NodeId),
}

impl DepEdge {
    pub(super) fn node_id(self) -> NodeId {
        match self {
            DepEdge::Owned(id) | DepEdge::Notify(id) => id,
        }
    }
}

/// Owned-edge sidecar built from `work_deps`: every dep the spawning work
/// reports is an owned sub-slot. `Notify` (park) edges are installed
/// separately via `add_park_edge`.
pub(super) fn work_owned_edges<'a>(work: &NodeWork<'a>) -> Vec<DepEdge> {
    match work_deps(work) {
        Some(ids) => ids.into_iter().map(DepEdge::Owned).collect(),
        None => Vec::new(),
    }
}

/// Tri-vector dependency state. All three vectors share an index space with
/// `Scheduler::nodes`.
pub(super) struct DepGraph {
    /// Forward edges: producer index -> consumer indices to wake.
    notify_list: Vec<Vec<usize>>,
    /// Count of not-yet-observed deps per consumer. Reaching zero routes the
    /// slot via `WorkQueues::push_woken`.
    pending_deps: Vec<usize>,
    /// Backward edges per consumer. `free` walks this sidecar but recurses
    /// only into `Owned`, so park edges cannot transit the reclaim walk into
    /// unrelated subgraphs.
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

    /// Atomic init of all three vectors for a freshly allocated slot,
    /// branching on recycle vs. extend so the caller can't observe the
    /// difference. `pending_producers` is the caller-filtered subset of
    /// `owned_edges` whose producers are not yet terminal, keeping `DepGraph`
    /// oblivious to results storage. Returns the installed pending count for
    /// enqueue routing.
    pub(super) fn install_for_slot(
        &mut self,
        consumer: NodeId,
        owned_edges: Vec<DepEdge>,
        pending_producers: &[NodeId],
    ) -> usize {
        if consumer.index() < self.notify_list.len() {
            self.notify_list[consumer.index()].clear();
            self.pending_deps[consumer.index()] = pending_producers.len();
            self.dep_edges[consumer.index()] = owned_edges;
        } else {
            self.notify_list.push(Vec::new());
            self.pending_deps.push(pending_producers.len());
            self.dep_edges.push(owned_edges);
        }
        for p in pending_producers {
            self.notify_list[p.index()].push(consumer.index());
        }
        pending_producers.len()
    }

    /// Atomic +1 across all three vectors for a mid-run owned dep. Caller
    /// guarantees `producer` is not yet terminal at install time.
    pub(super) fn add_owned_edge(
        &mut self,
        producer: NodeId,
        consumer: NodeId,
    ) {
        self.notify_list[producer.index()].push(consumer.index());
        self.pending_deps[consumer.index()] += 1;
        self.dep_edges[consumer.index()].push(DepEdge::Owned(producer));
    }

    /// Atomic +1 across all three vectors for a mid-run park edge. The
    /// backward entry is `Notify(producer)` so `free` skips past it. Caller
    /// guarantees `producer` is not yet terminal.
    pub(super) fn add_park_edge(
        &mut self,
        producer: NodeId,
        consumer: NodeId,
    ) {
        self.notify_list[producer.index()].push(consumer.index());
        self.pending_deps[consumer.index()] += 1;
        self.dep_edges[consumer.index()].push(DepEdge::Notify(producer));
    }

    /// True iff `producer` is forward-reachable from `consumer` along the
    /// wake graph — i.e. parking `consumer` on `producer` would deadlock
    /// (e.g. `LET Ty = Ty`, where the sub-Dispatch is the binder's Owned
    /// child and would park on its own ancestor). Caller surfaces a
    /// structured error instead of installing the park edge.
    pub(super) fn would_create_cycle(&self, producer: NodeId, consumer: NodeId) -> bool {
        if producer == consumer {
            return true;
        }
        let mut stack: Vec<usize> = vec![consumer.index()];
        let mut visited: std::collections::HashSet<usize> = std::collections::HashSet::new();
        while let Some(node) = stack.pop() {
            if !visited.insert(node) {
                continue;
            }
            for &next in &self.notify_list[node] {
                if next == producer.index() {
                    return true;
                }
                stack.push(next);
            }
        }
        false
    }

    /// Drains `notify_list[producer_idx]` and returns every consumer paired
    /// with a `hit_zero` flag indicating whether its `pending_deps` reached
    /// zero on this decrement. Atomic across the wake-pending pair (Inv-A
    /// still holds — the decrement is in-method).
    ///
    /// Callers fan-out: `Scheduler::finalize` always pushes the producer to
    /// the consumer's `recent_wakes` side-channel, and additionally stamps a
    /// pending Lift + pushes onto the woken run-set when `hit_zero` is true.
    /// Step 2 of the stateful-dispatch refactor (see
    /// `roadmap/dispatch_fix/stateful-dispatch-02-recent-wakes.md`) widened
    /// this return type from `Vec<usize>` so the caller can drive both the
    /// side-channel append (per consumer) and the queue push (per
    /// counter-zero consumer) off a single drain.
    pub(super) fn drain_notify(&mut self, producer_idx: usize) -> Vec<(usize, bool)> {
        let notifees = std::mem::take(&mut self.notify_list[producer_idx]);
        let mut out = Vec::with_capacity(notifees.len());
        for consumer in notifees {
            self.pending_deps[consumer] -= 1;
            let hit_zero = self.pending_deps[consumer] == 0;
            out.push((consumer, hit_zero));
        }
        out
    }

    /// Drains `dep_edges[idx]` (so a repeat free is a no-op) and yields only
    /// `Owned` children; `Notify` edges are dropped so the reclaim walk
    /// cannot transit into the producer's subtree.
    pub(super) fn owned_children(&mut self, idx: usize) -> impl Iterator<Item = NodeId> {
        let edges = std::mem::take(&mut self.dep_edges[idx]);
        edges.into_iter().filter_map(|e| match e {
            DepEdge::Owned(id) => Some(id),
            DepEdge::Notify(_) => None,
        })
    }

    /// Eager-free on the success path. Inv-C ensures `notify_list[idx]` is
    /// already drained by the time the caller hits this.
    pub(super) fn clear_dep_edges(&mut self, idx: usize) {
        self.dep_edges[idx].clear();
    }

    pub(super) fn pending_count(&self, idx: usize) -> usize {
        self.pending_deps[idx]
    }

    pub(super) fn is_dep_edges_empty(&self, idx: usize) -> bool {
        self.dep_edges[idx].is_empty()
    }

    // --- Test-only accessors for direct synthetic-state setup. ---

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
