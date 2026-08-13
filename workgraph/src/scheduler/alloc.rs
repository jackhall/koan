use std::rc::Rc;

use super::dep_graph::work_owned_edges;
use super::nodes::{NodeWork, seal_work};
use super::{EdgeId, NodeId, Scheduler, Workload};

impl<W: Workload> Scheduler<W> {
    /// Node-creation core: allocate a slot for `work`, wire its dep edges, install its memory anchor,
    /// and queue it if its deps are already satisfied. `anchor` is the slot's per-slot memory anchor
    /// (the workload mints it from its own active/run frame); the scheduler stores it and hands it
    /// back but calls only [`Anchor::owner`](super::Anchor::owner). `framed` is whether the workload
    /// had an active frame (`false` selects the fresh-top-level queue for a dep-free slot, matching
    /// the in-flight-vs-fresh split). This allocator never names a workload type — it only
    /// wires the slot's deps and installs its anchor. The work arrives with its continuation live;
    /// this is one of the erase doors that seals it against `anchor`.
    ///
    /// Owned deps only: a realized park is minted by the install door from the source edge an
    /// embedder holds, so a fresh slot that parks routes through
    /// [`alloc_node_with_parks`](Self::alloc_node_with_parks) instead.
    ///
    /// Edge installation here is *not*
    /// [`install_deps`](super::Scheduler::install_deps) — a fresh slot's row and its edges are
    /// initialized as one atomic step, and the slot **owns** the sub-work it spawns, so an
    /// already-finalized owned dep still records its backward `Owned` edge (the ownership record the
    /// error-path cascade walks). Only the pending counts filter by readiness.
    pub fn alloc_node(
        &mut self,
        work: NodeWork<'_, W>,
        anchor: Rc<W::Frame>,
        framed: bool,
    ) -> NodeId {
        debug_assert!(
            work.deps.parks().is_empty(),
            "a realized park is the install door's to write; this door takes owned deps only",
        );
        let owned_edges = work_owned_edges(&work);
        let no_owned = owned_edges.is_empty();
        let pending_owned: Vec<NodeId> = owned_edges
            .iter()
            .map(|e| e.node_id())
            .filter(|p| !self.is_result_ready(*p))
            .collect();
        let id = self.store.alloc_slot(seal_work(work, &anchor));
        self.deps.install_for_slot(id, owned_edges, &pending_owned);
        self.deps.install_anchor(id, anchor);
        if pending_owned.is_empty() {
            if !framed && no_owned {
                self.queues.push_fresh(id);
            } else {
                self.queues.push_in_flight_submit(id);
            }
        }
        id
    }

    /// [`alloc_node`](Self::alloc_node) for a fresh slot whose parks arrive as the **edges the
    /// embedder holds** rather than producers: the submit-time sibling of the
    /// [`install_deps`](Self::install_deps) door, routing the same wire primitive. `work` carries the
    /// slot's owned deps; its park list is written here, from the producers the door resolves the
    /// sources to, before the work is sealed.
    ///
    /// Each source's edge is minted before the slot exists (nothing about minting needs a consumer)
    /// and adopted onto the row once it does, so the row and its wires still land as one step. Queue
    /// routing then reads the row's true pending count, which the adoption has already settled.
    pub fn alloc_node_with_parks(
        &mut self,
        mut work: NodeWork<'_, W>,
        parks: &[EdgeId],
        anchor: Rc<W::Frame>,
        framed: bool,
    ) -> NodeId {
        debug_assert!(
            work.deps.parks().is_empty(),
            "the park list is the door's to write; the work carries owned deps only",
        );
        let minted: Vec<_> = parks
            .iter()
            .map(|&source| self.mint_park_edge(source))
            .collect();
        for (_, producer) in &minted {
            work.deps.push_park(*producer);
        }
        let owned_edges = work_owned_edges(&work);
        let no_owned = owned_edges.is_empty();
        let pending_owned: Vec<NodeId> = owned_edges
            .iter()
            .map(|e| e.node_id())
            .filter(|p| !self.is_result_ready(*p))
            .collect();
        let id = self.store.alloc_slot(seal_work(work, &anchor));
        self.deps.install_for_slot(id, owned_edges, &pending_owned);
        self.deps.install_anchor(id, anchor);
        for (installed, producer) in minted {
            self.adopt_park_edge(id, installed.edge_id(), producer);
        }
        if self.deps.pending_count(id) == 0 {
            if !framed && no_owned && parks.is_empty() {
                self.queues.push_fresh(id);
            } else {
                self.queues.push_in_flight_submit(id);
            }
        }
        id
    }
}
