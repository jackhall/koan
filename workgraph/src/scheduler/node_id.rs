//! [`NodeId`] — the stable handle to a node in the scheduler's DAG.

/// Handle naming a node slot in the scheduler's DAG.
///
/// The index is unreadable outside the crate and there is no public round-trip, so a `NodeId`
/// an embedder holds can only have come from the store — the whole scheduler surface speaks
/// this currency, never a raw slot index.
///
/// An id names a *position* in the slot table, not an incarnation: indices recycle through the
/// free list, so two ids for one index from different allocations compare equal, and an id is
/// meaningful only while the incarnation that minted it lives. A holder needing identity to
/// survive reclamation mints its own, as koan does with `StatementId` and `ProducerId`
/// ([design/dag-scheduler.md § Slots and the node-store lifecycle](../../design/dag-scheduler.md#slots-and-the-node-store-lifecycle)).
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId {
    index: usize,
}

impl NodeId {
    /// The mint. `pub(crate)`, so every id in circulation names a slot index this crate handed
    /// out rather than one an embedder chose.
    pub(crate) const fn new(index: usize) -> Self {
        NodeId { index }
    }

    pub(crate) fn index(self) -> usize {
        self.index
    }
}
