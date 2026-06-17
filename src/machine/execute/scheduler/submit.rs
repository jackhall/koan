use crate::machine::NodeId;

use super::super::nodes::{work_park_producers, CallFrame, Node, NodeWork};
use super::dep_graph::work_owned_edges;
use super::{Scheduler, Workload};

impl<W: Workload> Scheduler<W> {
    /// Whether a slot step is currently installed (a non-`None` ambient payload). The workload reads
    /// this to decide whether to default a submission's chain to the ambient one or synthesize a
    /// detached chain (test fixtures / top level).
    pub(in crate::machine::execute) fn has_active_payload(&self) -> bool {
        self.active_payload.is_some()
    }

    /// Node-creation core, shared by the run-lifetime [`KoanRuntime::add_with_chain`] and the framed
    /// [`KoanRuntime::dispatch_in_active_frame`](super::super::runtime::KoanRuntime::dispatch_in_active_frame).
    /// `payload` is the ready-built opaque workload payload (Koan: the pre-decided `NodeScope` handle
    /// plus the resolved lexical chain); the scheduler stores it on the slot and hands it back but
    /// never inspects it. This allocator never names a Koan type — it only wires the slot's deps and
    /// its frame cart.
    pub(in crate::machine::execute) fn submit_node(
        &mut self,
        work: NodeWork,
        payload: W::Payload,
    ) -> NodeId {
        // A binder-shaped Dispatch arrives with its `pre_subs` already populated and its
        // placeholder already installed by `dispatch::submit_dispatch`; this allocator never
        // inspects the work's AST.
        let owned_edges = work_owned_edges(&work);
        let no_owned = owned_edges.is_empty();
        // Top-level submissions (no active frame) fall back to the run frame, so every slot
        // carries a cart and `active_frame` is `Some` during its step. `run_frame` is
        // established by `add_with_chain` before the first submission, so the fallback is
        // always `Some` — the cart is non-optional node state.
        let cart = self.active_frame.clone().unwrap_or_else(|| {
            self.run_frame
                .clone()
                .expect("run_frame established by add_with_chain before any submission")
        });
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
            if self.active_frame.is_none() && no_owned && no_park {
                self.queues.push_fresh(id.index());
            } else {
                self.queues.push_in_flight_submit(id.index());
            }
        }
        id
    }
}
