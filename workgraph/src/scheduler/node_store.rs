//! Slot-table state. A single `slots` vector of [`SlotState`] enums encodes the per-slot
//! lifecycle: `alloc_slot` parks work, `take_for_run` moves it out, `reinstall` re-parks the index
//! on a tail replace, and `free_one` reclaims it.
//!
//! There is no terminal state. A finalizing slot's value is delivered into its consumers'
//! destination regions by the walk in [`lifecycle`](super::lifecycle), so the slot itself has
//! nothing left to hold and reclaims immediately: `Free` is the only state a finished slot rests
//! in, and a slot whose result *is* another producer's is spliced out rather than kept as an alias.
//!
//! See [design/dag-scheduler.md § Slots and the node-store lifecycle](../../design/dag-scheduler.md#slots-and-the-node-store-lifecycle).
//!
//! ## Invariants
//!
//! - `alloc_slot` is the only path that picks a slot index (recycle from `free_list` or extend
//!   `slots`); `free_one` is the sole pusher onto `free_list`.
//! - `slots` is wrapped in [`SlotVec<T>`], which impls only `Index<NodeId>` / `IndexMut<NodeId>`,
//!   so no raw index reaches the table.

use std::ops::{Index, IndexMut};

use super::nodes::StoredWork;
use super::{NodeId, Workload};

/// `Vec` behind a [`NodeId`]-only indexing surface, so a raw slot index cannot reach the table.
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
    /// Work moved out for the run; `reinstall` or `free_one` exits.
    Running,
    /// Reclaimed; the index sits on the free list.
    Free,
}

pub(in crate::scheduler) struct NodeStore<W: Workload> {
    slots: SlotVec<SlotState<W>>,
    /// Recycled ahead of extending `slots`, giving constant scheduler memory across
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

    /// The only path that picks a slot index.
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

    /// The id goes straight back on the free list, so the next `alloc_slot` hands out one equal to
    /// the id that just died. Nothing holds a [`NodeId`] across a reclaim, so there is no
    /// incarnation to tell apart.
    pub(super) fn free_one(&mut self, id: NodeId) {
        self.slots[id] = SlotState::Free;
        self.free_list.push(id);
    }

    /// A slot still parked (`PreRun`) after the work queues drained waits on a dependency that can
    /// no longer fire — a dependency cycle. Returns the count and the lowest-indexed such slot;
    /// what it renders as is the embedder's to answer off its anchor, since the store holds no
    /// diagnostic payload of its own.
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
