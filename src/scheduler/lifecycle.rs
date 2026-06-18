//! Slot terminalization and reclamation: the generic `finalize` / `free` / `reclaim_deps` the
//! workload's driver calls at a step's Done boundary. See
//! [design/execution-model.md § Dependency graph invariants](../../design/execution-model.md#dependency-graph-invariants).

use std::rc::Rc;

use super::{Live, NodeId, Scheduler, Workload};

impl<W: Workload> Scheduler<W> {
    /// Invariant: every consumer drained here is parked with a non-zero counter;
    /// freed slots are scrubbed from every producer's `notify_list` before the
    /// producer drains.
    ///
    /// Wakes must all land before any queue push: a later wake re-reading the
    /// slot must observe the prior transition.
    pub(crate) fn finalize(
        &mut self,
        idx: usize,
        output: Result<Live<'_, W>, W::Error>,
        frame: Option<Rc<W::Frame>>,
    ) {
        let id = NodeId(idx);
        self.store.finalize(id, output, frame);
        let drained = self.deps.drain_notify(idx);
        let mut woken: Vec<usize> = Vec::new();
        for (consumer, hit_zero) in drained {
            if hit_zero {
                woken.push(consumer);
            }
        }
        for consumer in woken {
            self.queues.push_woken(consumer);
        }
    }

    /// Recurses only into `DepEdge::Owned` entries; `Notify` entries point at sibling
    /// producers this slot merely parked on, and reclaiming a consumer must not reach
    /// across a park edge into the producer's subtree.
    ///
    /// Idempotent and safe to call on a still-live slot. References handed out by `read` survive
    /// because the value lives in an arena.
    pub(crate) fn free(&mut self, idx: usize) {
        let mut stack: Vec<NodeId> = vec![NodeId(idx)];
        while let Some(id) = stack.pop() {
            if self.store.is_live(id) {
                continue;
            }
            if self.store.is_reclaimed(id) && self.deps.is_dep_edges_empty(id.index()) {
                continue;
            }
            for child in self.deps.owned_children(id.index()) {
                stack.push(child);
            }
            self.store.free_one(id);
        }
    }

    /// Success-path eager free; the error path leaves deps for chain-free
    /// at slot drop. Inv-B is what makes `dep_edges[idx].clear()` sound
    /// here — see
    /// [design/execution-model.md § Dependency graph invariants](../../design/execution-model.md#dependency-graph-invariants).
    pub(crate) fn reclaim_deps(&mut self, idx: usize, dep_indices: Vec<usize>) {
        self.deps.clear_dep_edges(idx);
        for d in dep_indices {
            self.free(d);
        }
    }
}
