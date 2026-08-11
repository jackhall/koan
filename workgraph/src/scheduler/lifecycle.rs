//! Slot terminalization and reclamation: the generic `finalize` / `free` / `reclaim_deps` the
//! workload's driver calls at a step's Done boundary. See
//! [design/dag-scheduler.md § The dep row and its invariants](../../design/dag-scheduler.md#the-dep-row-and-its-invariants).

use std::rc::Rc;

use crate::witnessed::StepCoverage;

use super::workload::DeliveredTerminal;
use super::{Anchor, NodeId, Scheduler, Workload};

impl<W: Workload> Scheduler<W> {
    /// Invariant: every consumer drained here is parked with a non-zero counter;
    /// freed slots are scrubbed from every producer's `notify_list` before the
    /// producer drains.
    ///
    /// Wakes must all land before any queue push: a later wake re-reading the
    /// slot must observe the prior transition. The terminal arrives as a
    /// [`DeliveredTerminal`] — the carrier bundled with the owned coverage the workload's finalize
    /// hook composed — so its reach is the reach of the value being stored, not a set paired with it
    /// at the call.
    ///
    /// Seeds the slot's **frame-retention hold** unconditionally by projecting the region owner from
    /// the slot's own anchor and pairing it with the terminal's own foreign bundle, derived here by
    /// releasing the envelope's residence ([`Delivered::coverage_releasing_home`](crate::witnessed::Delivered::coverage_releasing_home)):
    /// the hold owns that region as its `owner` field, so re-listing it would be a second `Rc` on the
    /// very frame the hold's release frees. An error carries no value and so reaches nothing. The
    /// region and every reached region stay retained until every destination — the consumers parked
    /// here at finalize, plus any late parker — has pulled, released at pull-count zero.
    pub fn finalize(&mut self, id: NodeId, output: Result<DeliveredTerminal<W>, W::Error>) {
        let (sealed, foreign) = match output {
            Ok(envelope) => {
                let foreign = envelope.coverage_releasing_home();
                (Ok(envelope.into_cell()), foreign)
            }
            Err(error) => (Err(error), StepCoverage::empty()),
        };
        self.store.finalize(id, sealed);
        let drained = self.deps.drain_notify(id);
        // The consumers parked on this producer at finalize are its known destinations; a late parker
        // (wiring after this point) bumps the count through the ready-branch increment. Project the
        // retention owner from the slot's own anchor, then drop the anchor — its cart/chain are dead
        // weight once the slot is terminal; only the region survives, held by the retention hold
        // alongside the terminal's threaded foreign bundle.
        let anchor = self
            .deps
            .take_anchor(id)
            .expect("a finalizing slot still holds its anchor");
        self.deps
            .seed_retain(id, Rc::clone(anchor.owner()), foreign.0, drained.len());
        let mut woken: Vec<NodeId> = Vec::new();
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
    pub fn free(&mut self, id: NodeId) {
        let mut stack: Vec<NodeId> = vec![id];
        while let Some(id) = stack.pop() {
            if self.store.is_live(id) {
                continue;
            }
            if self.store.is_reclaimed(id) && self.deps.is_dep_edges_empty(id) {
                continue;
            }
            // This slot is dying: its last possible pull on every producer it still depends on is
            // now, so discharge each (its backward edges plus any late-park debt). Then release its
            // own retention hold — an owned producer's owner is done with it, so its region dies here
            // regardless of the remaining count — and release its memory anchor. All run before
            // `owned_children` drains the edges.
            self.deps.discharge_edges(id);
            self.deps.discharge_owed(id);
            self.deps.drop_retain(id);
            self.deps.clear_anchor(id);
            for child in self.deps.owned_children(id) {
                stack.push(child);
            }
            self.store.free_one(id);
        }
    }

    /// Success-path eager free; the error path leaves deps for chain-free
    /// at slot drop. Inv-B is what makes the slot's dep-edge clear sound
    /// here — see
    /// [design/dag-scheduler.md § The dep row and its invariants](../../design/dag-scheduler.md#the-dep-row-and-its-invariants).
    pub fn reclaim_deps(&mut self, id: NodeId, deps: Vec<NodeId>) {
        // The finalizing consumer has read its deps and won't read them again: discharge any
        // late-park debt it owes (its edges' pulls on shared/persistent producers ride until those
        // producers are themselves freed or the run tears down; its owned deps are released by the
        // cascade `free` below). `clear_dep_edges` then drops the edges, so a later free of this slot
        // finds none and cannot double-discharge.
        self.deps.discharge_owed(id);
        self.deps.clear_dep_edges(id);
        for d in deps {
            self.free(d);
        }
    }
}
