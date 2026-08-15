//! Bare-name forward splice — all the graph logic for eliminating a forwarding node.
//!
//! When a slot resolves to a downstream producer, its result *is* that producer's result. Rather
//! than keep a forwarding node, the slot is **spliced out**: its parked edges are re-pointed at the
//! real producer, which stays the single producer of that result, and the slot reclaims. Nothing
//! survives as a residual — no alias state, no alias walk on reads — because an edge's producer
//! pointer is the scheduler's to rewrite for exactly as long as the edge is unfilled. Post-delivery
//! the resident value *is* the value, so the surgery window closes at delivery, which is the right
//! semantics.
//!
//! See [design/dag-scheduler.md § Alias splice](../../design/dag-scheduler.md#alias-splice).

use super::{NodeId, Scheduler, Workload};

impl<W: Workload> Scheduler<W> {
    /// Splice `slot` out onto `producer`: re-point every live edge waiting on `slot` at `producer`
    /// and move them onto its notify list, then reclaim the slot. A released entry recycles here
    /// exactly as it would in the walk (Inv-C) — the splice is the other place a notify list is
    /// dropped.
    ///
    /// Crate-internal: an embedder names the producer by an edge, so it splices through
    /// [`splice_forward_from`](Self::splice_forward_from).
    pub(crate) fn splice_forward(&mut self, slot: NodeId, producer: NodeId) {
        let mut moved = self.deps.take_notify(slot);
        let edges = &mut self.edges;
        moved.retain(|&edge| {
            if edges.is_free(edge) {
                edges.recycle_released(edge);
                false
            } else {
                // The consumers' pending counts are unchanged: each still waits on one edge, now
                // serviced by `producer`'s single fire.
                edges.rewrite_producer(edge, producer);
                true
            }
        });
        self.deps.extend_notify(producer, moved);
        self.deps.clear_anchor(slot);
        self.store.free_one(slot);
    }
}
