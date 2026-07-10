//! Slot terminalization and reclamation: the generic `finalize` / `free` / `reclaim_deps` the
//! workload's driver calls at a step's Done boundary. See
//! [design/execution/scheduler.md § Dependency graph invariants](../../../design/execution/scheduler.md#dependency-graph-invariants).

use std::rc::Rc;

use super::{Anchor, NodeId, Scheduler, Terminal, Workload};

impl<W: Workload> Scheduler<W> {
    /// Invariant: every consumer drained here is parked with a non-zero counter;
    /// freed slots are scrubbed from every producer's `notify_list` before the
    /// producer drains.
    ///
    /// Wakes must all land before any queue push: a later wake re-reading the
    /// slot must observe the prior transition. The terminal arrives already bundled with its witness
    /// set (the producer frame ∪ the regions it reaches), built by the workload's finalize hook.
    ///
    /// Seeds the slot's **frame-retention hold** unconditionally by projecting the region owner from
    /// the slot's own anchor: the region stays retained until every destination — the consumers
    /// parked here at finalize, plus any late parker — has pulled, released at pull-count zero.
    pub fn finalize(&mut self, idx: usize, output: Result<Terminal<W>, W::Error>) {
        let id = NodeId(idx);
        self.store.finalize(id, output);
        let drained = self.deps.drain_notify(idx);
        // The consumers parked on this producer at finalize are its known destinations; a late parker
        // (wiring after this point) bumps the count through the ready-branch increment. Project the
        // retention owner from the slot's own anchor, then drop the anchor — its cart/chain are dead
        // weight once the slot is terminal; only the region survives, held by the retention hold.
        let anchor = self
            .deps
            .take_anchor(idx)
            .expect("a finalizing slot still holds its anchor");
        self.deps
            .seed_retain(idx, Rc::clone(anchor.owner()), drained.len());
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
    /// Idempotent and safe to call on a still-live slot. A value opened by a read lives in a region
    /// the carrier's frame pins, not in the slot, so freeing the slot cannot dangle it.
    pub fn free(&mut self, idx: usize) {
        let mut stack: Vec<NodeId> = vec![NodeId(idx)];
        while let Some(id) = stack.pop() {
            if self.store.is_live(id) {
                continue;
            }
            if self.store.is_reclaimed(id) && self.deps.is_dep_edges_empty(id.index()) {
                continue;
            }
            // This slot is dying: its last possible pull on every producer it still depends on is
            // now, so discharge each (its backward edges plus any late-park debt). Then release its
            // own retention hold — an owned producer's owner is done with it, so its region dies here
            // regardless of the remaining count — and release its memory anchor. All run before
            // `owned_children` drains the edges.
            self.deps.discharge_edges(id.index());
            self.deps.discharge_owed(id.index());
            self.deps.drop_retain(id.index());
            self.deps.clear_anchor(id.index());
            for child in self.deps.owned_children(id.index()) {
                stack.push(child);
            }
            self.store.free_one(id);
        }
    }

    /// Success-path eager free; the error path leaves deps for chain-free
    /// at slot drop. Inv-B is what makes `dep_edges[idx].clear()` sound
    /// here — see
    /// [design/execution/scheduler.md § Dependency graph invariants](../../../design/execution/scheduler.md#dependency-graph-invariants).
    pub fn reclaim_deps(&mut self, idx: usize, dep_indices: Vec<usize>) {
        // The finalizing consumer has read its deps and won't read them again: discharge any
        // late-park debt it owes (its edges' pulls on shared/persistent producers ride until those
        // producers are themselves freed or the run tears down; its owned deps are released by the
        // cascade `free` below). `clear_dep_edges` then drops the edges, so a later free of this slot
        // finds none and cannot double-discharge.
        self.deps.discharge_owed(idx);
        self.deps.clear_dep_edges(idx);
        for d in dep_indices {
            self.free(d);
        }
    }
}
