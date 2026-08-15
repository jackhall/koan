//! [`NodeId`] — the stable handle to a node in the scheduler's DAG.

/// Stable handle to a node in the scheduler's DAG. Minted only by the node store
/// (`alloc_slot`) and used to name a slot for the lifetime of a run.
///
/// The index is private and unreadable outside the crate, so a `NodeId` an embedder holds
/// can only have come from the store — the whole scheduler surface speaks this currency,
/// never a raw slot index, and the round-trip that would let a caller fabricate one does
/// not exist.
///
/// An id names a *position* in the slot table, not an incarnation. Slots reclaim at finalize and
/// their indices return to circulation, so two ids for one index from different allocations
/// compare equal — an id is only meaningful for as long as the incarnation that minted it lives.
/// That is the whole of its contract: the drive loop pops, steps, wires, and finalizes with one
/// inside a single step, and nothing holds one across a reclaim. A holder that needs identity to
/// survive reclamation is asking for something else and should mint its own — as koan does, with
/// `StatementId` for declaration identity and `ProducerId` for the producer of a value.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId {
    index: usize,
}

impl NodeId {
    /// The store's mint. `pub(crate)`, and the node store's slot allocator is its only caller, so
    /// every id in circulation names a slot that allocator handed out.
    pub(crate) const fn new(index: usize) -> Self {
        NodeId { index }
    }

    pub(crate) fn index(self) -> usize {
        self.index
    }

    /// Fabricate an id for a white-box test that drives no store. Gated so it cannot reach
    /// production code, and widened past `cfg(test)` for an embedder's own white-box tests,
    /// which compile against this crate as a dependency.
    #[cfg(any(test, feature = "test-hooks"))]
    pub const fn for_test(index: usize) -> Self {
        NodeId::new(index)
    }
}
