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
    /// Walk the slots paired with the id naming each — the store's own mint, so a scan that
    /// reports a slot hands back a currency the rest of the surface speaks.
    fn iter_ids(&self) -> impl Iterator<Item = (NodeId, &T)> {
        self.0.iter().enumerate().map(|(i, v)| (NodeId::new(i), v))
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

    /// Scan for slots still parked (`PreRun`) after the work queues drained — each is a node
    /// waiting on a dependency that can no longer fire (a dependency cycle). Returns
    /// `(count, first)` where `first` names the lowest-indexed such slot, or `None` when every slot
    /// has reclaimed. What that slot renders as is the embedder's to answer off its anchor; the
    /// store holds no diagnostic payload of its own.
    pub(super) fn unresolved(&self) -> Option<(usize, NodeId)> {
        let mut count = 0usize;
        let mut first: Option<NodeId> = None;
        for (id, slot) in self.slots.iter_ids() {
            if matches!(slot, SlotState::PreRun(_)) {
                count += 1;
                first.get_or_insert(id);
            }
        }
        first.map(|id| (count, id))
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

    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) fn free_list_len(&self) -> usize {
        self.free_list.len()
    }
}
