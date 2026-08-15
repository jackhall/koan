use std::rc::Rc;

use super::nodes::{NodeWork, seal_work};
use super::{EdgeId, NodeId, Scheduler, Workload};

impl<W: Workload> Scheduler<W> {
    /// Node-creation core: allocate a slot for `work`, wire its dep edges, install its memory anchor,
    /// and queue it if its deps are already satisfied. `anchor` is the slot's per-slot memory anchor
    /// (the workload mints it from its own active/run frame); the scheduler stores it and hands it
    /// back but calls only [`Anchor::owner`](super::Anchor::owner). `framed` is whether the workload
    /// had an active frame (`false` selects the fresh-top-level queue for a dep-free slot, matching
    /// the in-flight-vs-fresh split). This allocator never names a workload type — it only wires the
    /// slot's deps and installs its anchor. The work arrives with its continuation live; this is one
    /// of the erase doors that seals it against `anchor`.
    ///
    /// `sources` are the **edges the embedder holds**, one per dep in dep order. The door mints the
    /// slot's own edge off each, inheriting its destination — the same act
    /// [`install_deps`](Self::install_deps) performs for an already-allocated slot. This is the
    /// submit-time sibling of that door, routing the same wire primitive.
    ///
    /// The slot is allocated before its edges because an edge is *the consumer's own* — minting one
    /// names the consumer — so the row and the anchor go up first and the realized list lands on the
    /// slot's dep row. Queue routing then reads the row rather than arithmetic the caller did on the
    /// side.
    pub fn alloc_node(
        &mut self,
        work: NodeWork<'_, W>,
        sources: &[EdgeId],
        anchor: Rc<W::Frame>,
        framed: bool,
    ) -> NodeId {
        let id = self.store.alloc_slot(seal_work(work, &anchor));
        self.deps.install_for_slot(id);
        self.deps.install_anchor(id, anchor);
        let _verdicts = self.install_deps(id, sources);
        if self.deps.pending_count(id) == 0 {
            if !framed && sources.is_empty() {
                self.queues.push_fresh(id);
            } else {
                self.queues.push_in_flight_submit(id);
            }
        }
        id
    }
}
