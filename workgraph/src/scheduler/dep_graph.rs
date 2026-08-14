//! Per-slot dependency-graph state. Each slot's [`DepRow`] holds the two coordinated fields
//! (`notify`, `pending`) that share the slot index — keeping them in one row makes Inv-A
//! (wake-pending coherence) structural rather than enforced — plus the slot's memory anchor and its
//! TCO handoff hold. See
//! [design/dag-scheduler.md § The dep row and its invariants](../../design/dag-scheduler.md#the-dep-row-and-its-invariants).
//!
//! Both coordinated fields speak **edges**. `notify` lists the parked edges waiting on this slot as
//! a producer — the walk `finalize` runs, delivering into each one's destination — and `pending`
//! counts the unfilled edges this slot waits on as a consumer. The row holds no retention and no
//! backward edge list: a delivered value lives in its destination region, so nothing here has to
//! keep a producer alive past its own finalize.

use std::rc::Rc;

use super::{EdgeId, NodeId, Workload};

/// The two coordinated per-slot fields plus the slot's memory anchor and TCO handoff. Mutations go
/// through the row, so `notify` / `pending` cannot desync — Inv-A holds by construction.
struct DepRow<W: Workload> {
    /// Every live parked edge on this slot **as a producer** — what the finalize walk delivers
    /// into. Root and placeholder edges are listed here alongside consumer-bearing ones, which is
    /// what makes them receive delivery.
    notify: Vec<EdgeId>,
    /// Unfilled consumer-bearing edges this slot waits on **as a consumer**; zero routes via
    /// `WorkQueues::push_woken`.
    pending: usize,
    /// The slot's memory anchor, held from alloc until finalize — the scheduler-owned per-slot
    /// `Rc<W::Frame>`. `None` for a slot with no anchor installed yet (freshly recycled, before
    /// `install_anchor`) or one that has finalized (delivery moved every value out, so the anchor's
    /// region is free to die).
    anchor: Option<Rc<W::Frame>>,
    /// The **TCO handoff hold**: a framed tail replace's *displaced* incarnation anchor, parked
    /// here by [`Scheduler::replace`](crate::scheduler::Scheduler::replace) so the retiring region
    /// outlives the reinstalled incarnation's first step — where it adopts the loop-carried
    /// arguments. The displaced anchor pins the retiring region transitively through its projected
    /// owner. The run loop takes it just before running that step and drops it after, ordering the
    /// retiring region's free after the adoption.
    handoff: Option<Rc<W::Frame>>,
}

impl<W: Workload> Default for DepRow<W> {
    fn default() -> Self {
        DepRow {
            notify: Vec::new(),
            pending: 0,
            anchor: None,
            handoff: None,
        }
    }
}

pub(in crate::scheduler) struct DepGraph<W: Workload> {
    rows: Vec<DepRow<W>>,
}

impl<W: Workload> DepGraph<W> {
    pub(super) fn new() -> Self {
        Self { rows: Vec::new() }
    }

    /// The two row accessors. Every method below reaches its row through one of these, so the
    /// slot index a `NodeId` wraps is unwrapped in exactly one place per direction.
    fn row(&self, id: NodeId) -> &DepRow<W> {
        &self.rows[id.index()]
    }

    fn row_mut(&mut self, id: NodeId) -> &mut DepRow<W> {
        &mut self.rows[id.index()]
    }

    /// Init the slot's row (recycle or extend) to the empty state. The slot's wires land separately
    /// through [`wire_parked`](Self::wire_parked), which is what settles its pending count — so a
    /// fresh slot's queue routing reads the row rather than a caller-computed number.
    pub(super) fn install_for_slot(&mut self, consumer: NodeId) {
        if consumer.index() < self.rows.len() {
            let row = &mut self.rows[consumer.index()];
            debug_assert!(
                row.notify.is_empty(),
                "a recycled row's notify list was drained by the walk that reclaimed the slot",
            );
            row.pending = 0;
            row.anchor = None;
            row.handoff = None;
        } else {
            self.rows.push(DepRow::default());
        }
    }

    /// **The one wire primitive.** Register a parked `edge` on its producer's notify list and, when
    /// the edge names a consumer, count it against that consumer's pending. Every parked edge
    /// registers — placeholder and root edges among them, which is what makes them receive
    /// delivery — so the walk's list and the slab's parked set are the same set.
    pub(in crate::scheduler) fn wire_parked(
        &mut self,
        producer: NodeId,
        edge: EdgeId,
        consumer: Option<NodeId>,
    ) {
        self.list_parked(producer, edge);
        if let Some(consumer) = consumer {
            self.count_pending(consumer);
        }
    }

    /// The listing half of [`wire_parked`](Self::wire_parked): put `edge` on `producer`'s notify
    /// list. An edge is listed exactly once — a second entry would deliver into it twice.
    fn list_parked(&mut self, producer: NodeId, edge: EdgeId) {
        debug_assert!(
            !self.row(producer).notify.contains(&edge),
            "edge {edge:?} is already listed on slot {producer:?}",
        );
        self.row_mut(producer).notify.push(edge);
    }

    /// The counting half of [`wire_parked`](Self::wire_parked): count one unfilled edge against
    /// `consumer`'s pending. Called on its own where the edge was already listed at its mint and
    /// only its consumer arrives later — the install door attaching a park to the slot that waits
    /// on it.
    pub(in crate::scheduler) fn count_pending(&mut self, consumer: NodeId) {
        self.row_mut(consumer).pending += 1;
    }

    /// Take the producer's whole notify list — the finalize walk's drain. The row keeps none of it:
    /// the walk fills or recycles every entry, and the slot reclaims behind it.
    pub(super) fn take_notify(&mut self, producer: NodeId) -> Vec<EdgeId> {
        std::mem::take(&mut self.row_mut(producer).notify)
    }

    /// The producer's notify list, borrowed — the cycle walk's read.
    pub(in crate::scheduler) fn notify_of(&self, producer: NodeId) -> &[EdgeId] {
        &self.row(producer).notify
    }

    /// Decrement a consumer's pending count, returning whether it reached zero on this decrement —
    /// the walk's per-edge wake test.
    pub(super) fn decrement_pending(&mut self, consumer: NodeId) -> bool {
        let row = self.row_mut(consumer);
        debug_assert!(row.pending > 0, "pending under-run on slot {consumer:?}");
        row.pending -= 1;
        row.pending == 0
    }

    /// Append re-pointed entries to `producer`'s notify list — the splice's bulk half. Its caller
    /// rewrote each edge's producer field first, so the entry and the list it sits on name the same
    /// slot.
    pub(in crate::scheduler) fn extend_notify(&mut self, producer: NodeId, moved: Vec<EdgeId>) {
        self.row_mut(producer).notify.extend(moved);
    }

    /// Install the slot's memory anchor at alloc time (no previous anchor to displace). Every live
    /// slot holds an anchor from here until it finalizes.
    pub(super) fn install_anchor(&mut self, id: NodeId, anchor: Rc<W::Frame>) {
        self.row_mut(id).anchor = Some(anchor);
    }

    /// Swap the slot's memory anchor for `anchor` on a framed replace, returning the DISPLACED one
    /// (the previous incarnation's anchor, which the caller parks as the TCO handoff). Every live
    /// slot has an anchor, so the `.expect` is total on the replace path.
    pub(super) fn set_anchor(&mut self, id: NodeId, anchor: Rc<W::Frame>) -> Rc<W::Frame> {
        self.row_mut(id)
            .anchor
            .replace(anchor)
            .expect("a replacing slot still holds its anchor")
    }

    /// `Rc::clone` of the slot's memory anchor — the run loop keeps a clone across the step while
    /// the row retains its own. Every live slot has an anchor.
    pub(super) fn anchor_clone(&self, id: NodeId) -> Rc<W::Frame> {
        Rc::clone(
            self.row(id)
                .anchor
                .as_ref()
                .expect("every live slot has an anchor"),
        )
    }

    /// Drop the slot's memory anchor — what a finalizing slot does once its walk has drained.
    /// Delivery moved every value the slot produced into its destinations, so the anchor's region
    /// has nothing left to keep alive and its release is unconditional.
    pub(super) fn clear_anchor(&mut self, id: NodeId) {
        self.row_mut(id).anchor = None;
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) fn anchor_of(&self, id: NodeId) -> Option<Rc<W::Frame>> {
        self.row(id).anchor.as_ref().map(Rc::clone)
    }

    /// Park a framed tail replace's displaced incarnation anchor on the reinstalled `slot` as its
    /// TCO handoff hold (`None` clears it — a frameless `Inherit` replace turns over no region). The
    /// run loop [`take_handoff`](Self::take_handoff)s it just before the reinstalled incarnation's
    /// first step and holds it across that step, so the retiring region outlives the adoption of the
    /// carried arguments.
    pub(super) fn set_handoff(&mut self, slot: NodeId, displaced: Option<Rc<W::Frame>>) {
        self.row_mut(slot).handoff = displaced;
    }

    /// Take the reinstalled `slot`'s pending TCO handoff hold (draining it, so a slot that replaces
    /// again on this step re-parks a fresh one). The caller holds the returned `Rc` live across the
    /// step and drops it after, ordering the retiring region's free after the adoption.
    pub(super) fn take_handoff(&mut self, slot: NodeId) -> Option<Rc<W::Frame>> {
        if slot.index() < self.rows.len() {
            self.row_mut(slot).handoff.take()
        } else {
            None
        }
    }

    pub(super) fn pending_count(&self, id: NodeId) -> usize {
        self.row(id).pending
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) fn notify_list_iter(&self) -> impl Iterator<Item = (NodeId, &Vec<EdgeId>)> {
        self.rows
            .iter()
            .enumerate()
            .map(|(i, row)| (NodeId::new(i), &row.notify))
    }
}
