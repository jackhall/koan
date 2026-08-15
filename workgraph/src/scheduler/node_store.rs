//! Slot-table state. A single `slots` vector of [`SlotState`] enums encodes the per-slot lifecycle:
//! every slot moves through `alloc_slot -> take_for_run -> reinstall -> free_one`.
//!
//! There is no terminal state. A finalizing slot's value is delivered into its consumers'
//! destination regions by the walk in [`lifecycle`](super::lifecycle), so the slot itself has
//! nothing left to hold and reclaims immediately: `Free` is the only state a finished slot rests
//! in, and a slot whose result *is* another producer's is spliced out rather than kept as an alias.
//!
//! ## Invariants
//!
//! - `alloc_slot` is the only path that picks an index (recycle from
//!   `free_list` or extend `slots`).
//! - `slots` is wrapped in [`SlotVec<T>`], which only impls `Index<NodeId>` /
//!   `IndexMut<NodeId>`, so a `NodeId` always names a live slot.
//! - `free_one` is the sole pusher onto `free_list`. Outer `Scheduler`
//!   orchestrates the delivery walk and the reclaim across this store and
//!   `DepGraph`.

use std::ops::{Index, IndexMut};

use super::nodes::StoredWork;
use super::{NodeId, Workload};

/// `Vec`-backed slot store keyed by [`NodeId`]. `NodeId`s are minted only
/// by [`NodeStore::alloc_slot`].
struct SlotVec<T>(Vec<T>);

impl<T> SlotVec<T> {
    fn new() -> Self {
        Self(Vec::new())
    }
    fn push(&mut self, v: T) {
        self.0.push(v);
    }
    fn len(&self) -> usize {
        self.0.len()
    }
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    fn iter(&self) -> impl Iterator<Item = &T> {
        self.0.iter()
    }
}

impl<T> Index<NodeId> for SlotVec<T> {
    type Output = T;
    fn index(&self, id: NodeId) -> &T {
        &self.0[id.index()]
    }
}

impl<T> IndexMut<NodeId> for SlotVec<T> {
    fn index_mut(&mut self, id: NodeId) -> &mut T {
        &mut self.0[id.index()]
    }
}

enum SlotState<W: Workload> {
    PreRun(StoredWork<W>),
    /// Node work has been moved out by `take_for_run`. A matching
    /// `reinstall` / `free_one` exits this state.
    Running,
    /// Reclaimed; the index sits on the free list. Every slot reaches this state at its own
    /// finalize, once the delivery walk has drained — there is no terminal state in between.
    Free,
}

/// The drain-end deadlock-sample contribution of one parked/pending slot's work.
/// `unresolved` shows the first `Preferred` (a workload-supplied expression) across all stuck slots,
/// falling back to the first `Fallback` (a generic work-shape tag) only when no slot carries an
/// expression — so a stuck named work always out-renders a bare `<wait>`.
enum DeadlockSample {
    Preferred(String),
    Fallback(&'static str),
}

/// Map a stuck slot's `work` to its deadlock-sample contribution. A `Some`-carrier wait carries a
/// renderable expression summary (`Preferred`); a carrier-less wait carries only a generic tag
/// (`Fallback`).
fn work_deadlock_sample<W: Workload>(work: &StoredWork<W>) -> DeadlockSample {
    match &work.carrier {
        Some(carrier) => DeadlockSample::Preferred(carrier.clone()),
        None => DeadlockSample::Fallback("<wait>"),
    }
}

pub(in crate::scheduler) struct NodeStore<W: Workload> {
    slots: SlotVec<SlotState<W>>,
    /// Reclaimed slot indices. `alloc_slot` pulls from here before
    /// extending `slots`, giving constant scheduler memory across
    /// tail-recursive bodies.
    free_list: Vec<NodeId>,
}

impl<W: Workload> NodeStore<W> {
    pub(super) fn new() -> Self {
        Self {
            slots: SlotVec::new(),
            free_list: Vec::new(),
        }
    }

    /// The only path that picks an index, and the only mint of a [`NodeId`].
    /// `DepGraph::install_for_slot` mirrors the recycle-vs.-extend choice by
    /// testing the consumer against its own row count.
    pub(super) fn alloc_slot(&mut self, work: StoredWork<W>) -> NodeId {
        match self.free_list.pop() {
            Some(id) => {
                self.slots[id] = SlotState::PreRun(work);
                id
            }
            None => {
                let id = NodeId::new(self.slots.len());
                self.slots.push(SlotState::PreRun(work));
                id
            }
        }
    }

    /// Panics if the slot wasn't `PreRun`.
    pub(super) fn take_for_run(&mut self, id: NodeId) -> StoredWork<W> {
        match std::mem::replace(&mut self.slots[id], SlotState::Running) {
            SlotState::PreRun(work) => work,
            _ => panic!("scheduler must not revisit a completed node"),
        }
    }

    /// Write the slot's realized dep list — the install door's write-back. A fresh slot's edges are
    /// *the slot's own*, so minting them needs the slot to exist; the work therefore lands first with
    /// an empty list and the door fills it in once its wiring is settled.
    pub(super) fn write_deps(&mut self, id: NodeId, deps: super::ResolvedDeps) {
        match &mut self.slots[id] {
            SlotState::PreRun(work) => work.deps = deps,
            _ => panic!("only a pre-run slot takes its realized dep list"),
        }
    }

    /// Tail-call path: reuse the slot index for a new node's work.
    pub(super) fn reinstall(&mut self, id: NodeId, work: StoredWork<W>) {
        self.slots[id] = SlotState::PreRun(work);
    }

    /// Reclaim the slot and return its index to circulation — what every finalize does once its
    /// delivery walk has drained. Pairs with the row's own anchor clear in `DepGraph`.
    ///
    /// The id goes straight back on the free list, so the next `alloc_slot` hands out one equal to
    /// the id that just died. Nothing holds a `NodeId` across a reclaim, so there is no incarnation
    /// to tell apart.
    pub(super) fn free_one(&mut self, id: NodeId) {
        self.slots[id] = SlotState::Free;
        self.free_list.push(id);
    }

    /// Scan for slots still parked (`PreRun`) after the work queues drained — each
    /// is a node waiting on a dependency that can no longer fire (a dependency
    /// cycle). Returns `(count, sample)` where `sample` summarizes the first such
    /// node, or `None` when every slot has reclaimed.
    pub(super) fn unresolved(&self) -> Option<(usize, String)> {
        let mut count = 0usize;
        let mut expr_sample: Option<String> = None;
        let mut fallback_sample: Option<String> = None;
        for slot in self.slots.iter() {
            if let SlotState::PreRun(work) = slot {
                count += 1;
                match work_deadlock_sample(work) {
                    DeadlockSample::Preferred(s) if expr_sample.is_none() => expr_sample = Some(s),
                    DeadlockSample::Fallback(s) if fallback_sample.is_none() => {
                        fallback_sample = Some(s.to_string());
                    }
                    _ => {}
                }
            }
        }
        if count == 0 {
            return None;
        }
        Some((count, expr_sample.or(fallback_sample).unwrap_or_default()))
    }

    pub(super) fn len(&self) -> usize {
        self.slots.len()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub(super) fn is_live(&self, id: NodeId) -> bool {
        matches!(self.slots[id], SlotState::PreRun(_))
    }

    // --- Test-only helpers for synthetic-state setup. ---

    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) fn clear_node(&mut self, id: NodeId) {
        self.slots[id] = SlotState::Running;
    }

    /// The realized dep list the install door wrote onto a pre-run slot — the probe a wiring test
    /// reads its own edges back through.
    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) fn stored_deps(&self, id: NodeId) -> &super::ResolvedDeps {
        match &self.slots[id] {
            SlotState::PreRun(work) => &work.deps,
            _ => panic!("only a pre-run slot holds a realized dep list"),
        }
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) fn free_list_len(&self) -> usize {
        self.free_list.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::witnessed::reattachable;

    /// A lifetime-free `Reattachable` family for the trivial test value.
    struct U32Value;
    /// A lifetime-free `Reattachable` family standing in for the contract / continuation carriers.
    struct UnitCarrier;
    // Both are lifetime-free, so `At<'r>` is the same type for every `'r`; the shared `reattachable!`
    // macro discharges the obligation.
    reattachable! {
        U32Value => u32,
        UnitCarrier => (),
    }

    /// A minimal memory anchor projecting a trivial region owner. The store tests seal work against
    /// one, so it is constructible over a fresh empty region.
    struct TestAnchor(std::rc::Rc<crate::witnessed::doctest_fixture::RegionCart>);
    impl TestAnchor {
        fn new() -> std::rc::Rc<Self> {
            std::rc::Rc::new(TestAnchor(crate::witnessed::doctest_fixture::fresh_cart()))
        }
    }
    impl crate::scheduler::Anchor for TestAnchor {
        type Owner = crate::witnessed::doctest_fixture::RegionCart;
        fn owner(&self) -> &std::rc::Rc<Self::Owner> {
            &self.0
        }
    }

    /// A minimal workload for the white-box store tests: every associated type is trivial, so the
    /// generic store can be exercised without naming any Koan type. These tests only classify a
    /// parked slot's deadlock sample, so the delivery hook is never reached — the scheduler-level
    /// slates in [`tests::delivery`](super::super::tests) are what exercise it.
    struct TestWorkload;
    impl Workload for TestWorkload {
        type Value = U32Value;
        type Error = ();
        type Profile = crate::witnessed::doctest_fixture::FixtureProfile;
        type Frame = TestAnchor;
        type Continuation = UnitCarrier;

        fn deliver(
            _terminal: &super::super::workload::DeliveredTerminal<Self>,
            _dest: super::super::workload::DeliveryDestination<Self>,
        ) -> super::super::workload::DeliveredTerminal<Self> {
            unimplemented!("the deadlock-sample slate finalizes nothing")
        }
    }

    fn sample_wait(carrier: Option<String>) -> StoredWork<TestWorkload> {
        super::super::nodes::seal_work(
            super::super::nodes::NodeWork::new(super::super::ResolvedDeps::new(), (), carrier),
            &TestAnchor::new(),
        )
    }

    #[test]
    fn some_carrier_wait_prefers_the_carrier() {
        let work = sample_wait(Some("PARKED-EXPR".to_string()));
        assert!(
            matches!(work_deadlock_sample(&work), DeadlockSample::Preferred(s) if s.contains("PARKED")),
            "a Some-carrier wait must surface its carrier",
        );
    }

    #[test]
    fn carrier_less_wait_falls_back_to_a_tag() {
        let work = sample_wait(None);
        assert!(
            matches!(
                work_deadlock_sample(&work),
                DeadlockSample::Fallback("<wait>")
            ),
            "a carrier-less wait must surface a generic tag, not an empty sample",
        );
    }
}
