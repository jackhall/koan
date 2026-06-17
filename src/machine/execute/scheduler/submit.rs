use std::rc::Rc;

use crate::machine::NodeId;

use super::super::nodes::{work_park_producers, CallFrame, Node, NodeWork};
use super::dep_graph::work_owned_edges;
use super::{Scheduler, Workload};

impl<W: Workload> Scheduler<W> {
    /// Node-creation core, shared by the run-lifetime [`KoanRuntime::add_with_chain`] and the framed
    /// [`KoanRuntime::dispatch_in_active_frame`](super::super::runtime::KoanRuntime::dispatch_in_active_frame).
    /// `payload` is the ready-built opaque workload payload (Koan: the pre-decided `NodeScope` handle
    /// plus the resolved lexical chain); the scheduler stores it on the slot and hands it back but
    /// never inspects it. `cart` is the slot's frame cart, resolved by the driver from its ambient
    /// active/run frame; `framed` is whether the driver had an active frame (`false` selects the
    /// fresh-top-level queue for a dep-free / park-free slot, matching the in-flight-vs-fresh split).
    /// This allocator never names a Koan type — it only wires the slot's deps and its frame cart.
    pub(in crate::machine::execute) fn submit_node(
        &mut self,
        work: NodeWork<W>,
        payload: W::Payload,
        cart: Rc<W::Frame>,
        framed: bool,
    ) -> NodeId {
        // A binder-shaped Dispatch arrives with its `pre_subs` already populated and its
        // placeholder already installed by `dispatch::submit_dispatch`; this allocator never
        // inspects the work's AST.
        let owned_edges = work_owned_edges(&work);
        let no_owned = owned_edges.is_empty();
        let pending_owned: Vec<NodeId> = owned_edges
            .iter()
            .map(|e| e.node_id())
            .filter(|p| !self.is_result_ready(*p))
            .collect();
        let pending_park: Vec<NodeId> = work_park_producers(&work)
            .iter()
            .copied()
            .filter(|p| !self.is_result_ready(*p))
            .collect();
        let no_park = work_park_producers(&work).is_empty();
        let id = self.store.alloc_slot(Node {
            work,
            payload,
            frame: CallFrame {
                cart,
                reserve: None,
                contract: None,
            },
        });
        self.deps.install_for_slot(id, owned_edges, &pending_owned);
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
