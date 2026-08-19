//! **Delivery at finalize** — the walk that distributes a producer's terminal into every
//! destination waiting on it, and the slot reclaim that follows it unconditionally. See
//! [design/dag-scheduler.md § Delivery at finalize](../../design/dag-scheduler.md#delivery-at-finalize).

use crate::witnessed::{Delivered, Retained};

use super::workload::{DeliveredTerminal, SealedTerminal};
use super::{EdgeId, NodeId, Scheduler, Workload};

impl<W: Workload> Scheduler<W> {
    /// **The delivery walk.** Distributes the terminal to every live edge on this slot's notify
    /// list, then reclaims the slot.
    ///
    /// Three things happen per edge, in one pass:
    ///
    /// - A **released** entry is skipped and its slab index recycled. That is half of Inv-C:
    ///   release withholds a listed index from circulation precisely so that dropping the entry —
    ///   here or at a splice — is what returns it, which is what makes a consumer's death before
    ///   its producer fires order-free.
    /// - A **live** entry receives the terminal *at its own destination*. Adoption is per distinct
    ///   destination region, not per edge, so the second write into one region is free; an error
    ///   terminal carries no value to adopt and clones per destination instead.
    /// - Its **consumer's pending** drops by one, and a consumer that reaches zero is woken. Every
    ///   edge fills before any consumer is queued.
    ///
    /// The envelope is held across the whole walk, so its transit pins cover every adopt. The slot
    /// then reclaims unconditionally, with no retention condition of any kind, because delivery has
    /// already moved everything this producer made into regions that outlive it.
    pub(in crate::scheduler) fn finalize(
        &mut self,
        id: NodeId,
        output: Result<DeliveredTerminal<W>, W::Error>,
    ) {
        let mut notify = self.deps.take_notify(id);
        // Scrubbed up front so the walk below runs over live entries alone and the look-back scan
        // never has to re-test. Inv-C's recycle point: no list names the index once the entry drops.
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
    /// path, where a slot's result *is* the value another edge already carries. Lifting the
    /// resident back into an envelope under its own destination's owner and re-entering the
    /// ordinary walk costs one relocation per distinct onward destination and keeps `finalize` free
    /// of a special case.
    pub(in crate::scheduler) fn finalize_forward(&mut self, slot: NodeId, edge: EdgeId) {
        let output = match self.edges.resident_duplicate(edge) {
            Ok(cell) => Ok(Delivered::lift(cell, self.edges.destination_host(edge))),
            Err(error) => Err(error),
        };
        self.finalize(slot, output);
    }

    /// The look-back half of per-destination dedup. `None` when this destination is new to the walk
    /// and the terminal must be adopted into it.
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

    /// **Adopt** the terminal into `edge`'s destination region and leave it there at rest.
    /// [`Workload::deliver`] runs the embedder's relocation across a bare handle on that region —
    /// built through the one public door, [`Delivered::destination`] — deepcopy or pin, with the
    /// retention claim the verdict implies, and the product's coverage lodges in that region's
    /// union bundle for the region's life.
    fn adopt_at(&self, edge: EdgeId, envelope: &DeliveredTerminal<W>) -> SealedTerminal<W> {
        let dest = Delivered::destination(self.edges.destination_host(edge));
        let handle = self.edges.destination_handle(edge);
        Retained::from_sealed(W::deliver(envelope, dest).rest_into(handle))
    }
}
