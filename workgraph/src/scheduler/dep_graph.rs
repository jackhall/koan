//! Per-slot dependency-graph state. Each slot's [`DepRow`] holds the two coordinated fields
//! (`notify`, `pending`) that share the slot index — keeping them in one row makes Inv-A
//! (wake-pending coherence) structural rather than enforced — plus the slot's memory anchor. See
//! [design/dag-scheduler.md § The dep row and its invariants](../../design/dag-scheduler.md#the-dep-row-and-its-invariants).
//!
//! The row holds no retention and no backward edge list: a delivered value lives in its destination
//! region, so nothing here has to keep a producer alive past its own finalize.

use std::rc::Rc;

use super::{EdgeId, NodeId, ResolvedDeps, Workload};

/// Mutations go through the row, so `notify` / `pending` cannot desync — Inv-A holds by
/// construction.
struct DepRow<W: Workload> {
    /// Every live parked edge on this slot **as a producer**. Root and placeholder edges are listed
    /// here alongside consumer-bearing ones, which is what makes them receive delivery.
    notify: Vec<EdgeId>,
    /// Unfilled consumer-bearing edges this slot waits on **as a consumer**.
    pending: usize,
    /// The slot's realized dep list — its **own** edges in dep order, written by the install door
    /// and never assembled by hand. A mid-step install writes the next incarnation's list onto the
    /// row that step start emptied.
    deps: ResolvedDeps,
    /// The scheduler-owned per-slot anchor, held from alloc until finalize. `None` for a slot with
    /// no anchor installed yet (freshly recycled, before `install_anchor`) or one that has
    /// finalized — delivery moved every value out, so the anchor's region is free to die.
    anchor: Option<Rc<W::Frame>>,
}

impl<W: Workload> Default for DepRow<W> {
    fn default() -> Self {
        DepRow {
            notify: Vec::new(),
            pending: 0,
            deps: ResolvedDeps::new(),
            anchor: None,
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

    fn row(&self, id: NodeId) -> &DepRow<W> {
        &self.rows[id.index()]
    }

    fn row_mut(&mut self, id: NodeId) -> &mut DepRow<W> {
        &mut self.rows[id.index()]
    }

    /// Init the slot's row (recycle or extend) to the empty state. Wires land separately, so a fresh
    /// slot's queue routing reads the settled row rather than a caller-computed count.
    pub(super) fn install_for_slot(&mut self, consumer: NodeId) {
        if consumer.index() < self.rows.len() {
            let row = &mut self.rows[consumer.index()];
            debug_assert!(
                row.notify.is_empty(),
                "a recycled row's notify list was drained by the walk that reclaimed the slot",
            );
            row.pending = 0;
            // Cleared, not replaced: the row keeps the buffer a previous incarnation grew, so a
            // steady-state graph shape stops allocating dep lists after warm-up.
            row.deps.clear();
            row.anchor = None;
        } else {
            self.rows.push(DepRow::default());
        }
    }

    /// Register a parked `edge` on its producer's notify list and, when the edge names a consumer,
    /// count it against that consumer's pending. Every parked edge registers — placeholder and root
    /// edges among them — so the walk's list and the slab's parked set stay the same set.
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

    /// An edge is listed exactly once — a second entry would deliver into it twice.
    fn list_parked(&mut self, producer: NodeId, edge: EdgeId) {
        debug_assert!(
            !self.row(producer).notify.contains(&edge),
            "edge {edge:?} is already listed on slot {producer:?}",
        );
        self.row_mut(producer).notify.push(edge);
    }

    /// Count one unfilled edge against `consumer`'s pending. Stands alone where the edge was already
    /// listed at its mint and only its consumer arrives later.
    pub(in crate::scheduler) fn count_pending(&mut self, consumer: NodeId) {
        self.row_mut(consumer).pending += 1;
    }

    /// Take the producer's whole notify list. The row keeps none of it: the finalize walk fills or
    /// recycles every entry, and the slot reclaims behind it. Every taker hands the buffer back
    /// through [`restore_notify`](Self::restore_notify) once it is done reading.
    pub(super) fn take_notify(&mut self, producer: NodeId) -> Vec<EdgeId> {
        std::mem::take(&mut self.row_mut(producer).notify)
    }

    /// Hand a taken notify buffer back to the row it came off, emptied. Capacity stays owned by the
    /// row that grew it, so the next incarnation of a recycled slot wires into the same allocation.
    pub(in crate::scheduler) fn restore_notify(
        &mut self,
        producer: NodeId,
        mut notify: Vec<EdgeId>,
    ) {
        notify.clear();
        debug_assert!(
            self.row(producer).notify.is_empty(),
            "a restore lands on the row the take emptied; slot {producer:?} was re-listed meanwhile",
        );
        self.row_mut(producer).notify = notify;
    }

    pub(in crate::scheduler) fn record_dep(&mut self, consumer: NodeId, edge: EdgeId) {
        self.row_mut(consumer).deps.push(edge);
    }

    pub(in crate::scheduler) fn take_deps(&mut self, id: NodeId) -> ResolvedDeps {
        std::mem::take(&mut self.row_mut(id).deps)
    }

    /// Hand a taken dep list back to the row it came off, emptied — the drain's half of the same
    /// take-and-restore pair. It runs *before* the step callback, so a mid-step install writes the
    /// next incarnation's list into the recycled buffer rather than a fresh one.
    pub(in crate::scheduler) fn restore_deps(&mut self, id: NodeId, mut deps: ResolvedDeps) {
        deps.clear();
        debug_assert!(
            self.row(id).deps.is_empty(),
            "a restore lands on the row the take emptied; slot {id:?} was re-installed meanwhile",
        );
        self.row_mut(id).deps = deps;
    }

    pub(in crate::scheduler) fn notify_of(&self, producer: NodeId) -> &[EdgeId] {
        &self.row(producer).notify
    }

    /// Returns whether the count reached zero on this decrement — the walk's per-edge wake test.
    pub(super) fn decrement_pending(&mut self, consumer: NodeId) -> bool {
        let row = self.row_mut(consumer);
        debug_assert!(row.pending > 0, "pending under-run on slot {consumer:?}");
        row.pending -= 1;
        row.pending == 0
    }

    /// Append re-pointed entries to `producer`'s notify list. Precondition: each moved edge's
    /// producer field already names `producer`, so the entry and the list it sits on agree.
    /// Takes a slice rather than the vector, so the spliced slot keeps its own buffer to restore.
    pub(in crate::scheduler) fn extend_notify(&mut self, producer: NodeId, moved: &[EdgeId]) {
        self.row_mut(producer).notify.extend_from_slice(moved);
    }

    /// Install the slot's anchor at alloc time — there is no previous anchor to displace.
    pub(super) fn install_anchor(&mut self, id: NodeId, anchor: Rc<W::Frame>) {
        self.row_mut(id).anchor = Some(anchor);
    }

    /// Swap the slot's anchor on a framed replace, returning the DISPLACED one — the retiring
    /// incarnation's. Every live slot has an anchor, so the `.expect` is total on the replace path.
    pub(super) fn set_anchor(&mut self, id: NodeId, anchor: Rc<W::Frame>) -> Rc<W::Frame> {
        self.row_mut(id)
            .anchor
            .replace(anchor)
            .expect("a replacing slot still holds its anchor")
    }

    pub(super) fn anchor_clone(&self, id: NodeId) -> Rc<W::Frame> {
        Rc::clone(
            self.row(id)
                .anchor
                .as_ref()
                .expect("every live slot has an anchor"),
        )
    }

    /// Drop the slot's anchor once its finalize walk has drained. Delivery moved every value the
    /// slot produced into its destinations, so the release is unconditional.
    pub(super) fn clear_anchor(&mut self, id: NodeId) {
        self.row_mut(id).anchor = None;
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) fn anchor_of(&self, id: NodeId) -> Option<Rc<W::Frame>> {
        self.row(id).anchor.as_ref().map(Rc::clone)
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) fn stored_deps(&self, id: NodeId) -> Vec<EdgeId> {
        self.row(id).deps.all_ids().collect()
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
