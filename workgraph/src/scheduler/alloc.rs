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
    /// `owned` are the producers this slot spawned. The door mints one edge apiece, destined at the
    /// slot's own anchor region — the same act [`install_deps`](Self::install_deps) performs for an
    /// already-allocated slot.
    ///
    /// Owned deps only: a realized park is minted from the source edge an embedder holds, so a fresh
    /// slot that parks routes through [`alloc_node_with_parks`](Self::alloc_node_with_parks).
    pub fn alloc_node(
        &mut self,
        work: NodeWork<'_, W>,
        owned: &[NodeId],
        anchor: Rc<W::Frame>,
        framed: bool,
    ) -> NodeId {
        self.alloc_wiring(work, &[], owned, anchor, framed)
    }

    /// [`alloc_node`](Self::alloc_node) for a fresh slot whose parks arrive as the **edges the
    /// embedder holds**: the submit-time sibling of the [`install_deps`](Self::install_deps) door,
    /// routing the same wire primitive. Each source's edge is minted off it, inheriting its
    /// destination, exactly as the install door does.
    pub fn alloc_node_with_parks(
        &mut self,
        work: NodeWork<'_, W>,
        parks: &[EdgeId],
        owned: &[NodeId],
        anchor: Rc<W::Frame>,
        framed: bool,
    ) -> NodeId {
        self.alloc_wiring(work, parks, owned, anchor, framed)
    }

    /// The shared body of both allocators: allocate the slot, initialize its row and anchor, wire the
    /// whole dep list through the one install door, and route it by the pending count that wiring
    /// settled.
    ///
    /// The slot is allocated before its edges because an edge is *the consumer's own* — minting one
    /// names the consumer and, for an owned dep, its anchor region — so the row and the anchor go up
    /// first and the realized list is written back onto the stored work. Queue routing then reads the
    /// row rather than arithmetic the caller did on the side.
    fn alloc_wiring(
        &mut self,
        work: NodeWork<'_, W>,
        parks: &[EdgeId],
        owned: &[NodeId],
        anchor: Rc<W::Frame>,
        framed: bool,
    ) -> NodeId {
        debug_assert!(
            work.deps.is_empty(),
            "a fresh slot's realized dep list is this door's to write; the work arrives with none",
        );
        let id = self.store.alloc_slot(seal_work(work, &anchor));
        self.deps.install_for_slot(id);
        self.deps.install_anchor(id, anchor);
        let (resolved, _verdicts) = self.install_deps(id, parks, owned);
        self.store.write_deps(id, resolved);
        if self.deps.pending_count(id) == 0 {
            if !framed && owned.is_empty() && parks.is_empty() {
                self.queues.push_fresh(id);
            } else {
                self.queues.push_in_flight_submit(id);
            }
        }
        id
    }
}
