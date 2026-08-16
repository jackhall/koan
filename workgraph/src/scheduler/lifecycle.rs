//! **Delivery at finalize** — the walk that distributes a producer's terminal into every
//! destination waiting on it, and the slot reclaim that follows it unconditionally. See
//! [design/dag-scheduler.md § Delivery at finalize](../../design/dag-scheduler.md#delivery-at-finalize).

use crate::witnessed::{Delivered, Retained};

use super::workload::{DeliveredTerminal, SealedTerminal};
use super::{EdgeId, NodeId, Scheduler, Workload};

impl<W: Workload> Scheduler<W> {
    /// **The delivery walk.** The terminal arrives as a [`DeliveredTerminal`] — the carrier bundled
    /// with the owned coverage the workload's finalize hook composed — and is distributed to every
    /// live edge on this slot's notify list before the slot reclaims.
    ///
    /// Three things happen per edge, in one pass:
    ///
    /// - A **released** entry is skipped and its slab index recycled. That is the second half of
    ///   Inv-C: release withholds a listed index from circulation precisely so this walk can be the
    ///   one that returns it, which is what makes a consumer's death before its producer fires
    ///   order-free.
    /// - A **live** entry receives the terminal *at its own destination*. Adoption is per distinct
    ///   destination region, not per edge: a linear look-back over the entries already visited finds
    ///   an earlier edge naming the same region and shares its resident, so the second write into
    ///   one region is free. The scan allocates nothing and keeps no state past the walk. An error
    ///   terminal carries no value to adopt, so it simply clones per edge.
    /// - Its **consumer's pending** drops by one, and a consumer that reaches zero is woken. Wakes
    ///   all land before any queue push, so a later wake re-reading the slot observes the prior
    ///   transition.
    ///
    /// The envelope is held across the whole walk, so its transit pins cover every adopt; it drops
    /// when the walk ends. Then the slot's anchor is released and the slot reclaims —
    /// unconditionally, with no retention condition of any kind, because delivery has already moved
    /// everything this producer made into regions that outlive it.
    pub(in crate::scheduler) fn finalize(
        &mut self,
        id: NodeId,
        output: Result<DeliveredTerminal<W>, W::Error>,
    ) {
        let mut notify = self.deps.take_notify(id);
        // Inv-C's recycle point: drop every entry whose edge its owner already released, returning
        // each index to circulation now that no list names it. Doing it up front leaves the walk
        // below over live entries alone, so the look-back scan never has to re-test.
        let edges = &mut self.edges;
        notify.retain(|&edge| {
            if edges.is_free(edge) {
                edges.recycle_released(edge);
                false
            } else {
                true
            }
        });
        let mut woken: Vec<NodeId> = Vec::new();
        for i in 0..notify.len() {
            let edge = notify[i];
            let resident = match self.shared_resident(&notify[..i], edge) {
                Some(shared) => shared,
                None => match &output {
                    Ok(envelope) => Ok(self.adopt_at(edge, envelope)),
                    Err(error) => Err(error.clone()),
                },
            };
            self.edges.fill(edge, resident);
            if let Some(consumer) = self.edges.consumer_of(edge)
                && self.deps.decrement_pending(consumer)
            {
                woken.push(consumer);
            }
        }
        for consumer in woken {
            self.queues.push_woken(consumer);
        }
        self.deps.clear_anchor(id);
        self.store.free_one(id);
    }

    /// Finalize `slot` with the terminal already resting on `edge` — the forward-a-ready-producer
    /// path, where a slot's result *is* the value another edge already carries. The resident is
    /// lifted back into an envelope under its own destination's owner, then delivered onward by the
    /// ordinary walk, so a forward costs one relocation per distinct onward destination and no
    /// special case in `finalize`.
    ///
    /// The drain names the source by the edge the step's `Forward` verdict carries and the slot by
    /// the id it is stepping.
    pub(in crate::scheduler) fn finalize_forward(&mut self, slot: NodeId, edge: EdgeId) {
        let output = match self.edges.resident_duplicate(edge) {
            Ok(cell) => Ok(Delivered::lift(cell, self.edges.destination_host(edge))),
            Err(error) => Err(error),
        };
        self.finalize(slot, output);
    }

    /// The look-back half of per-destination dedup: an already-visited edge naming the same
    /// destination region, whose resident this edge shares. `None` when this destination is new to
    /// the walk and the terminal must be adopted into it.
    fn shared_resident(
        &self,
        visited: &[EdgeId],
        edge: EdgeId,
    ) -> Option<Result<SealedTerminal<W>, W::Error>> {
        let destination = self.edges.destination_region(edge);
        visited
            .iter()
            .find(|&&earlier| std::ptr::eq(self.edges.destination_region(earlier), destination))
            .map(|&earlier| self.edges.resident_duplicate(earlier))
    }

    /// **Adopt** the terminal into `edge`'s destination region and leave it there at rest. The
    /// destination operand is a bare handle on that region, built through the one public door
    /// ([`Delivered::destination`]) off the destination's own owner; [`Workload::deliver`] runs the
    /// embedder's relocation across it — deepcopy or pin, with the retention claim the verdict
    /// implies — and the product rests in the destination, lodging its coverage in that region's
    /// union bundle for the region's life.
    fn adopt_at(&self, edge: EdgeId, envelope: &DeliveredTerminal<W>) -> SealedTerminal<W> {
        let dest = Delivered::destination(self.edges.destination_host(edge));
        let handle = self.edges.destination_handle(edge);
        Retained::from_sealed(W::deliver(envelope, dest).rest_into(handle))
    }
}
