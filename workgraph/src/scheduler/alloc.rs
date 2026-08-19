use std::rc::Rc;

use super::nodes::{NodeWork, seal_work};
use super::{EdgeId, NodeId, Scheduler, Workload};

impl<W: Workload> Scheduler<W> {
    /// Allocate a slot for `work`, wire its dep edges, install its memory anchor, and queue it if
    /// its deps are already satisfied. The work arrives with its continuation live and is sealed
    /// against `anchor` on the way in.
    ///
    /// `framed` reports whether the workload had an active frame: a dep-free slot submitted without
    /// one is a fresh top-level dispatch, so it routes to the fresh queue rather than the in-flight
    /// one.
    ///
    /// `sources` are the **edges the embedder holds**, one per dep in dep order. This is the
    /// submit-time sibling of [`install_deps`](Self::install_deps), which serves an
    /// already-allocated slot.
    ///
    /// The slot is allocated before its edges because an edge is *the consumer's own* — minting one
    /// names the consumer — so the row and the anchor go up before the realized dep list lands on
    /// the slot's dep row.
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
