//! Per-slot dependency-graph state. Each slot's [`DepRow`] holds the three coordinated fields
//! (`notify`, `pending`, `edges`) that share the slot index — keeping them in one row makes Inv-A
//! (wake-pending coherence) structural rather than enforced — plus the slot's
//! **delivery-driven frame-retention** bookkeeping (`retain`, `owed`). See
//! [design/execution/scheduler.md § Dependency graph invariants](../../../design/execution/scheduler.md#dependency-graph-invariants)
//! for the Inv-A / Inv-B / Inv-C contract and
//! [design/witness-hosting.md § Retention model](../../../design/witness-hosting.md#retention-model)
//! for the pull-count release rule.

use std::rc::Rc;

use super::nodes::NodeWork;
use super::workload::OwnerOf;
use super::{NodeId, Workload};

/// Backward edge in `dep_edges[consumer]`. Kind only matters at reclaim:
/// `free` recurses into `Owned` children but stops at `Notify` so the walk
/// cannot transit into unrelated subgraphs.
#[derive(Copy, Clone, Debug)]
pub enum DepEdge {
    Owned(NodeId),
    Notify(NodeId),
}

impl DepEdge {
    pub(super) fn node_id(self) -> NodeId {
        match self {
            DepEdge::Owned(id) | DepEdge::Notify(id) => id,
        }
    }
}

/// Owned-edge sidecar built from a node's owned deps. Park edges are installed
/// separately via `add_park_edge`.
pub(super) fn work_owned_edges<W: Workload>(work: &NodeWork<W>) -> Vec<DepEdge> {
    work.deps
        .owned()
        .iter()
        .copied()
        .map(DepEdge::Owned)
        .collect()
}

/// The scheduler's frame-retention hold on one finalized producer slot: the producer frame's owner
/// `Rc`, kept alive until every destination has pulled the terminal. `pulls` counts the outstanding
/// destinations; the owner is dropped (releasing the frame) when a discharge brings `pulls` to zero.
/// A seed of zero pulls does **not** release — it means "no current destination; wait for a late
/// parker or an explicit free" — so only a decrement-to-zero triggers release. See
/// [design/witness-hosting.md § Retention model](../../../design/witness-hosting.md#retention-model).
struct RetentionHold<F> {
    /// The retained producer frame's owner. Its Drop releases the frame; the pinned read of a
    /// retained terminal re-anchors the value under a clone of it ([`DepGraph::retained_owner`]).
    owner: Rc<F>,
    pulls: usize,
}

/// The three coordinated per-slot fields plus the slot's retention bookkeeping and its memory
/// anchor. Mutations go through the row, so `notify` / `pending` / `edges` cannot desync — Inv-A
/// holds by construction. The row carries both the anchor (`Rc<W::Frame>`) and the projected
/// retention owner (`Rc<OwnerOf<W>>`); these are distinct types.
struct DepRow<W: Workload> {
    /// Forward wake edges from this producer to its consumers.
    notify: Vec<usize>,
    /// Not-yet-observed deps for this consumer; zero routes via
    /// `WorkQueues::push_woken`.
    pending: usize,
    /// Backward edges from this consumer to its producers; `free` recurses
    /// only into `Owned`.
    edges: Vec<DepEdge>,
    /// The slot's memory anchor, held from alloc until finalize/free — the scheduler-owned per-slot
    /// `Rc<W::Frame>` this item makes scheduler-side. `None` for a slot with no anchor installed
    /// yet (freshly recycled, before `install_anchor`).
    anchor: Option<Rc<W::Frame>>,
    /// The frame-retention hold while this slot is a Done producer whose region is retained; `None`
    /// for a live/frameless/released slot. Its owner `Rc` keeps the producer region alive until
    /// every destination pulls.
    retain: Option<RetentionHold<OwnerOf<W>>>,
    /// Producers this consumer wired to **after** they had already finalized (the late-park path,
    /// which installs no `notify` edge). Each entry is a retained producer whose `pulls` this
    /// consumer bumped on wiring and must discharge once — after its read, or at its death.
    owed: Vec<usize>,
    /// The **TCO handoff hold**: a framed tail replace's *displaced* incarnation anchor, parked
    /// here by [`Scheduler::replace`](crate::scheduler::Scheduler::replace) so the retiring region
    /// outlives the reinstalled incarnation's first step — where it adopts the loop-carried
    /// arguments. The displaced anchor pins the retiring region transitively through its projected
    /// owner. The run loop takes it just before running that step and drops it after, ordering the
    /// retiring region's free after the adoption. Distinct from `retain`: `retain` holds *this
    /// slot's own* Done producer region; `handoff` holds the *previous* incarnation's anchor across
    /// the reinstall.
    handoff: Option<Rc<W::Frame>>,
}

impl<W: Workload> Default for DepRow<W> {
    fn default() -> Self {
        DepRow {
            notify: Vec::new(),
            pending: 0,
            edges: Vec::new(),
            anchor: None,
            retain: None,
            owed: Vec::new(),
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

    /// Atomic init of the consumer's row (recycle or extend) plus the
    /// per-producer notify backlinks. `pending_producers` is the
    /// caller-filtered subset of `owned_edges` whose producers are not yet
    /// terminal, so `DepGraph` stays oblivious to results storage. Returns
    /// the installed pending count. A recycled row's stale retention state
    /// (`retain` / `owed`) is cleared — a slot is only recycled after `free`
    /// dropped its hold, so this is belt-and-suspenders.
    pub(super) fn install_for_slot(
        &mut self,
        consumer: NodeId,
        owned_edges: Vec<DepEdge>,
        pending_producers: &[NodeId],
    ) -> usize {
        let pending = pending_producers.len();
        if consumer.index() < self.rows.len() {
            let row = &mut self.rows[consumer.index()];
            row.notify.clear();
            row.pending = pending;
            row.edges = owned_edges;
            row.anchor = None;
            row.retain = None;
            row.owed.clear();
            row.handoff = None;
        } else {
            self.rows.push(DepRow {
                notify: Vec::new(),
                pending,
                edges: owned_edges,
                anchor: None,
                retain: None,
                owed: Vec::new(),
                handoff: None,
            });
        }
        for p in pending_producers {
            self.rows[p.index()].notify.push(consumer.index());
        }
        pending
    }

    /// Atomic +1 on the consumer's pending count, edges list, and the
    /// producer's notify list. Caller guarantees `producer` is not yet
    /// terminal.
    pub(in crate::scheduler) fn add_owned_edge(&mut self, producer: NodeId, consumer: NodeId) {
        self.rows[producer.index()].notify.push(consumer.index());
        let row = &mut self.rows[consumer.index()];
        row.pending += 1;
        row.edges.push(DepEdge::Owned(producer));
    }

    /// Atomic +1 across the producer's notify list and the consumer's
    /// pending count + edges; the backward entry is `Notify(producer)` so
    /// `free` skips past it. Caller guarantees `producer` is not yet
    /// terminal.
    pub(in crate::scheduler) fn add_park_edge(&mut self, producer: NodeId, consumer: NodeId) {
        self.rows[producer.index()].notify.push(consumer.index());
        let row = &mut self.rows[consumer.index()];
        row.pending += 1;
        row.edges.push(DepEdge::Notify(producer));
    }

    /// Seed a finalized producer's retention hold with the region owner (projected from the slot's
    /// anchor) and its current destination count (the consumers parked on it at finalize). Called
    /// once per Done producer, projecting the owner from the slot's own anchor.
    pub(super) fn seed_retain(&mut self, producer: usize, owner: Rc<OwnerOf<W>>, pulls: usize) {
        self.rows[producer].retain = Some(RetentionHold { owner, pulls });
    }

    /// Record that `consumer` wired to an already-finalized retained `producer`: bump the producer's
    /// outstanding pull count and remember the debt on the consumer, to be discharged once (after the
    /// consumer's read, or at its death). No-op when `producer` carries no hold.
    pub(super) fn owe_late_pull(&mut self, producer: usize, consumer: usize) {
        if let Some(hold) = self.rows[producer].retain.as_mut() {
            hold.pulls += 1;
            self.rows[consumer].owed.push(producer);
        }
    }

    /// Discharge one destination pull on `producer`, releasing its frame (dropping the owner `Rc`)
    /// when the count reaches zero. A `None` hold (frameless, or already released) is a no-op.
    fn decrement_pull(&mut self, producer: usize) {
        if let Some(hold) = self.rows[producer].retain.as_mut() {
            debug_assert!(
                hold.pulls > 0,
                "retention over-discharge on slot {producer}"
            );
            hold.pulls = hold.pulls.saturating_sub(1);
            if hold.pulls == 0 {
                self.rows[producer].retain = None;
            }
        }
    }

    /// Discharge every late-pull `consumer` owes (draining the debt so it discharges exactly once) —
    /// the after-read / at-death discharge of the late-park increments.
    pub(super) fn discharge_owed(&mut self, consumer: usize) {
        let owed = std::mem::take(&mut self.rows[consumer].owed);
        for producer in owed {
            self.decrement_pull(producer);
        }
    }

    /// Discharge one pull on each producer `consumer` still holds a backward edge to — the dying
    /// consumer's last-possible-pull discharge (`free`). Reads the edge list without draining it;
    /// the caller drains it separately (`owned_children`) for the reclaim recursion, and `free`
    /// processes each slot once, so this fires exactly once per edge.
    pub(super) fn discharge_edges(&mut self, consumer: usize) {
        let producers: Vec<usize> = self.rows[consumer]
            .edges
            .iter()
            .map(|e| e.node_id().index())
            .collect();
        for producer in producers {
            self.decrement_pull(producer);
        }
    }

    /// A clone of `producer`'s retained region owner, or `None` for a frameless / released producer —
    /// the liveness pin a retention-pinned read holds live across [`Sealed::open_with`] while it
    /// re-anchors the terminal's value.
    pub(super) fn retained_owner(&self, producer: usize) -> Option<Rc<OwnerOf<W>>> {
        self.rows[producer]
            .retain
            .as_ref()
            .map(|hold| Rc::clone(&hold.owner))
    }

    /// Drop `producer`'s retention hold outright — the owned-producer prompt release (its owning
    /// consumer is done with it) and the re-home drain's explicit clear. Releases the frame
    /// regardless of the remaining pull count.
    pub(super) fn drop_retain(&mut self, producer: usize) {
        self.rows[producer].retain = None;
    }

    /// Install the slot's memory anchor at alloc time (no previous anchor to displace). Every live
    /// slot holds an anchor from here until `free` clears it.
    pub(super) fn install_anchor(&mut self, idx: usize, anchor: Rc<W::Frame>) {
        self.rows[idx].anchor = Some(anchor);
    }

    /// Swap the slot's memory anchor for `anchor` on a framed replace, returning the DISPLACED one
    /// (the previous incarnation's anchor, which the caller parks as the TCO handoff). Every live
    /// slot has an anchor, so the `.expect` is total on the replace path.
    pub(super) fn set_anchor(&mut self, idx: usize, anchor: Rc<W::Frame>) -> Rc<W::Frame> {
        self.rows[idx]
            .anchor
            .replace(anchor)
            .expect("a replacing slot still holds its anchor")
    }

    /// `Rc::clone` of the slot's memory anchor — the run loop keeps a clone across the step while
    /// the row retains its own. Every live slot has an anchor.
    pub(super) fn anchor_clone(&self, idx: usize) -> Rc<W::Frame> {
        Rc::clone(
            self.rows[idx]
                .anchor
                .as_ref()
                .expect("every live slot has an anchor"),
        )
    }

    /// Take the slot's memory anchor by value — `finalize` does this to project the retention owner
    /// from it, then drops the anchor (its cart/chain are dead weight once the slot is terminal).
    pub(super) fn take_anchor(&mut self, idx: usize) -> Option<Rc<W::Frame>> {
        self.rows[idx].anchor.take()
    }

    /// Clear the slot's memory anchor outright — a dying slot (`free`) releases it.
    pub(super) fn clear_anchor(&mut self, idx: usize) {
        self.rows[idx].anchor = None;
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) fn anchor_of(&self, idx: usize) -> Option<Rc<W::Frame>> {
        self.rows[idx].anchor.as_ref().map(Rc::clone)
    }

    /// Park a framed tail replace's displaced incarnation anchor on the reinstalled `slot` as its
    /// TCO handoff hold (`None` clears it — a frameless `Inherit` replace turns over no region). The
    /// run loop [`take_handoff`](Self::take_handoff)s it just before the reinstalled incarnation's
    /// first step and holds it across that step, so the retiring region outlives the adoption of the
    /// carried arguments.
    pub(super) fn set_handoff(&mut self, slot: usize, displaced: Option<Rc<W::Frame>>) {
        self.rows[slot].handoff = displaced;
    }

    /// Take the reinstalled `slot`'s pending TCO handoff hold (draining it, so a slot that replaces
    /// again on this step re-parks a fresh one). The caller holds the returned `Rc` live across the
    /// step and drops it after, ordering the retiring region's free after the adoption.
    pub(super) fn take_handoff(&mut self, slot: usize) -> Option<Rc<W::Frame>> {
        if slot < self.rows.len() {
            self.rows[slot].handoff.take()
        } else {
            None
        }
    }

    /// True iff `producer` is forward-reachable from `consumer` — i.e.
    /// parking `consumer` on `producer` would deadlock (e.g. `LET Ty = Ty`,
    /// where the sub-Dispatch would park on its own ancestor). Caller surfaces
    /// a structured error instead of installing the park edge.
    pub(in crate::scheduler) fn would_create_cycle(
        &self,
        producer: NodeId,
        consumer: NodeId,
    ) -> bool {
        if producer == consumer {
            return true;
        }
        let mut stack: Vec<usize> = vec![consumer.index()];
        let mut visited: std::collections::HashSet<usize> = std::collections::HashSet::new();
        while let Some(node) = stack.pop() {
            if !visited.insert(node) {
                continue;
            }
            for &next in &self.rows[node].notify {
                if next == producer.index() {
                    return true;
                }
                stack.push(next);
            }
        }
        false
    }

    /// Drains the producer's notify list and returns every consumer paired
    /// with a `hit_zero` flag indicating whether its pending count reached
    /// zero on this decrement. The `hit_zero` channel lets the caller append
    /// to a side-channel for every consumer while only enqueueing
    /// counter-zero ones, off a single drain.
    pub(super) fn drain_notify(&mut self, producer_idx: usize) -> Vec<(usize, bool)> {
        let notifees = std::mem::take(&mut self.rows[producer_idx].notify);
        let mut out = Vec::with_capacity(notifees.len());
        for consumer in notifees {
            let row = &mut self.rows[consumer];
            row.pending -= 1;
            out.push((consumer, row.pending == 0));
        }
        out
    }

    /// Drains the slot's edges (so a repeat free is a no-op) and yields only
    /// `Owned` children; `Notify` edges are dropped so the reclaim walk
    /// cannot transit into the producer's subtree.
    pub(super) fn owned_children(&mut self, idx: usize) -> impl Iterator<Item = NodeId> {
        let edges = std::mem::take(&mut self.rows[idx].edges);
        edges.into_iter().filter_map(|e| match e {
            DepEdge::Owned(id) => Some(id),
            DepEdge::Notify(_) => None,
        })
    }

    /// Eager-free on the success path. Inv-C ensures the slot's notify list
    /// is already drained by the time the caller hits this.
    pub(in crate::scheduler) fn clear_dep_edges(&mut self, idx: usize) {
        self.rows[idx].edges.clear();
    }

    /// Move `from`'s notify list onto `into`'s — the bare-name-forward splice. `from`'s consumers
    /// keep their pending counts and `from`-labelled edges; `into`'s fire now drains them (and their
    /// reads of `from` follow the alias to `into`). Their pending counts are unchanged: each still
    /// waits on one dep, now serviced by `into`'s single fire.
    pub(in crate::scheduler) fn splice_notify(&mut self, from: usize, into: usize) {
        let moved = std::mem::take(&mut self.rows[from].notify);
        self.rows[into].notify.extend(moved);
    }

    pub(super) fn pending_count(&self, idx: usize) -> usize {
        self.rows[idx].pending
    }

    pub(super) fn is_dep_edges_empty(&self, idx: usize) -> bool {
        self.rows[idx].edges.is_empty()
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) fn dep_edges_at(&self, idx: usize) -> &[DepEdge] {
        &self.rows[idx].edges
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) fn set_dep_edges(&mut self, idx: usize, edges: Vec<DepEdge>) {
        self.rows[idx].edges = edges;
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) fn notify_list_iter(&self) -> impl Iterator<Item = (usize, &Vec<usize>)> {
        self.rows
            .iter()
            .enumerate()
            .map(|(i, row)| (i, &row.notify))
    }
}
