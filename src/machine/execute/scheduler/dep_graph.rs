//! Per-slot dependency-graph state pulled out of `Scheduler<'a>`. Each slot's
//! [`DepRow`] holds the three coordinated fields (`notify`, `pending`,
//! `edges`) that share the slot index — keeping them in one row makes Inv-A
//! (wake-pending coherence) structural rather than enforced. See
//! [design/execution-model.md § Dependency graph invariants](../../../../design/execution-model.md#dependency-graph-invariants)
//! for the Inv-A / Inv-B / Inv-C contract.

use crate::machine::NodeId;
use super::super::nodes::{NodeWork, work_deps};

/// Backward edge in `dep_edges[consumer]`. Kind only matters at reclaim:
/// `free` recurses into `Owned` children but stops at `Notify` so the walk
/// cannot transit into unrelated subgraphs.
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

/// Owned-edge sidecar built from `work_deps`. Park edges are installed
/// separately via `add_park_edge`.
pub(super) fn work_owned_edges<'a>(work: &NodeWork<'a>) -> Vec<DepEdge> {
    match work_deps(work) {
        Some(ids) => ids.into_iter().map(DepEdge::Owned).collect(),
        None => Vec::new(),
    }
}

/// The three coordinated per-slot fields. Mutations go through the row, so
/// `notify` / `pending` / `edges` cannot desync at the row level — Inv-A
/// holds by construction.
#[derive(Default)]
pub(super) struct DepRow {
    /// Forward wake edges from this producer to its consumers.
    notify: Vec<usize>,
    /// Not-yet-observed deps for this consumer; zero routes via
    /// `WorkQueues::push_woken`.
    pending: usize,
    /// Backward edges from this consumer to its producers; `free` recurses
    /// only into `Owned`.
    edges: Vec<DepEdge>,
}

pub(in crate::machine::execute::scheduler) struct DepGraph {
    rows: Vec<DepRow>,
}

impl DepGraph {
    pub(super) fn new() -> Self {
        Self { rows: Vec::new() }
    }

    /// Atomic init of the consumer's row (recycle or extend) plus the
    /// per-producer notify backlinks. `pending_producers` is the
    /// caller-filtered subset of `owned_edges` whose producers are not yet
    /// terminal, so `DepGraph` stays oblivious to results storage. Returns
    /// the installed pending count.
    pub(super) fn install_for_slot(
        &mut self,
        consumer: NodeId,
        owned_edges: Vec<DepEdge>,
        pending_producers: &[NodeId],
    ) -> usize {
        let pending = pending_producers.len();
        if consumer.index() < self.rows.len() {
            let row = &mut self.rows[consumer.index()];
            row.notify.clear();
            row.pending = pending;
            row.edges = owned_edges;
        } else {
            self.rows.push(DepRow {
                notify: Vec::new(),
                pending,
                edges: owned_edges,
            });
        }
        for p in pending_producers {
            self.rows[p.index()].notify.push(consumer.index());
        }
        pending
    }

    /// Atomic +1 on the consumer's pending count, edges list, and the
    /// producer's notify list. Caller guarantees `producer` is not yet
    /// terminal.
    pub(in crate::machine::execute::scheduler) fn add_owned_edge(
        &mut self,
        producer: NodeId,
        consumer: NodeId,
    ) {
        self.rows[producer.index()].notify.push(consumer.index());
        let row = &mut self.rows[consumer.index()];
        row.pending += 1;
        row.edges.push(DepEdge::Owned(producer));
    }

    /// Atomic +1 across the producer's notify list and the consumer's
    /// pending count + edges; the backward entry is `Notify(producer)` so
    /// `free` skips past it. Caller guarantees `producer` is not yet
    /// terminal.
    pub(in crate::machine::execute::scheduler) fn add_park_edge(
        &mut self,
        producer: NodeId,
        consumer: NodeId,
    ) {
        self.rows[producer.index()].notify.push(consumer.index());
        let row = &mut self.rows[consumer.index()];
        row.pending += 1;
        row.edges.push(DepEdge::Notify(producer));
    }

    /// True iff `producer` is forward-reachable from `consumer` — i.e.
    /// parking `consumer` on `producer` would deadlock (e.g. `LET Ty = Ty`,
    /// where the sub-Dispatch would park on its own ancestor). Caller surfaces
    /// a structured error instead of installing the park edge.
    pub(in crate::machine::execute::scheduler) fn would_create_cycle(&self, producer: NodeId, consumer: NodeId) -> bool {
        if producer == consumer {
            return true;
        }
        let mut stack: Vec<usize> = vec![consumer.index()];
        let mut visited: std::collections::HashSet<usize> = std::collections::HashSet::new();
        while let Some(node) = stack.pop() {
            if !visited.insert(node) {
                continue;
            }
            for &next in &self.rows[node].notify {
                if next == producer.index() {
                    return true;
                }
                stack.push(next);
            }
        }
        false
    }

    /// Drains the producer's notify list and returns every consumer paired
    /// with a `hit_zero` flag indicating whether its pending count reached
    /// zero on this decrement. The `hit_zero` channel lets the caller append
    /// to a side-channel for every consumer while only enqueueing
    /// counter-zero ones, off a single drain.
    pub(super) fn drain_notify(&mut self, producer_idx: usize) -> Vec<(usize, bool)> {
        let notifees = std::mem::take(&mut self.rows[producer_idx].notify);
        let mut out = Vec::with_capacity(notifees.len());
        for consumer in notifees {
            let row = &mut self.rows[consumer];
            row.pending -= 1;
            out.push((consumer, row.pending == 0));
        }
        out
    }

    /// Drains the slot's edges (so a repeat free is a no-op) and yields only
    /// `Owned` children; `Notify` edges are dropped so the reclaim walk
    /// cannot transit into the producer's subtree.
    pub(super) fn owned_children(&mut self, idx: usize) -> impl Iterator<Item = NodeId> {
        let edges = std::mem::take(&mut self.rows[idx].edges);
        edges.into_iter().filter_map(|e| match e {
            DepEdge::Owned(id) => Some(id),
            DepEdge::Notify(_) => None,
        })
    }

    /// Eager-free on the success path. Inv-C ensures the slot's notify list
    /// is already drained by the time the caller hits this.
    pub(in crate::machine::execute::scheduler) fn clear_dep_edges(&mut self, idx: usize) {
        self.rows[idx].edges.clear();
    }

    pub(super) fn pending_count(&self, idx: usize) -> usize {
        self.rows[idx].pending
    }

    pub(super) fn is_dep_edges_empty(&self, idx: usize) -> bool {
        self.rows[idx].edges.is_empty()
    }

    #[cfg(test)]
    pub(super) fn dep_edges_at(&self, idx: usize) -> &[DepEdge] {
        &self.rows[idx].edges
    }

    #[cfg(test)]
    pub(super) fn set_dep_edges(&mut self, idx: usize, edges: Vec<DepEdge>) {
        self.rows[idx].edges = edges;
    }

    #[cfg(test)]
    pub(super) fn notify_list_iter(&self) -> impl Iterator<Item = (usize, &Vec<usize>)> {
        self.rows.iter().enumerate().map(|(i, row)| (i, &row.notify))
    }
}
