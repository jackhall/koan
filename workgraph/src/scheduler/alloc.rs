use std::rc::Rc;

use super::dep_graph::work_owned_edges;
use super::nodes::NodeWork;
use super::{NodeId, Scheduler, Workload};

impl<W: Workload> Scheduler<W> {
    /// Node-creation core: allocate a slot for `work`, wire its dep edges, install its memory anchor,
    /// and queue it if its deps are already satisfied. `anchor` is the slot's per-slot memory anchor
    /// (the workload mints it from its own active/run frame); the scheduler stores it and hands it
    /// back but calls only [`Anchor::owner`](super::Anchor::owner). `framed` is whether the workload
    /// had an active frame (`false` selects the fresh-top-level queue for a dep-free / park-free slot,
    /// matching the in-flight-vs-fresh split). This allocator never names a workload type — it only
    /// wires the slot's deps and installs its anchor.
    pub fn alloc_node(&mut self, work: NodeWork<W>, anchor: Rc<W::Frame>, framed: bool) -> NodeId {
        let owned_edges = work_owned_edges(&work);
        let no_owned = owned_edges.is_empty();
        let pending_owned: Vec<NodeId> = owned_edges
            .iter()
            .map(|e| e.node_id())
            .filter(|p| !self.is_result_ready(*p))
            .collect();
        let pending_park: Vec<NodeId> = work
            .deps
            .parks()
            .iter()
            .copied()
            .filter(|p| !self.is_result_ready(*p))
            .collect();
        let no_park = work.deps.parks().is_empty();
        let id = self.store.alloc_slot(work);
        self.deps.install_for_slot(id, owned_edges, &pending_owned);
        self.deps.install_anchor(id.index(), anchor);
        for p in &pending_park {
            self.deps.add_park_edge(*p, id);
        }
        if pending_owned.is_empty() && pending_park.is_empty() {
            if !framed && no_owned && no_park {
                self.queues.push_fresh(id.index());
            } else {
                self.queues.push_in_flight_submit(id.index());
            }
        }
        id
    }
}
