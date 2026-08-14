//! [`NodeId`] — the stable handle to a node in the scheduler's DAG.

/// Stable handle to a node in the scheduler's DAG. Minted only by the node store
/// (`alloc_slot`) and used to name a slot for the lifetime of a run.
///
/// The index is private and unreadable outside the crate, so a `NodeId` an embedder holds
/// can only have come from the store — the whole scheduler surface speaks this currency,
/// never a raw slot index, and the round-trip that would let a caller fabricate one does
/// not exist.
///
/// An id carries its slot's **allocation stamp** alongside the index. Slots reclaim at finalize and
/// their indices return to circulation, so an index alone names a *position*, not an incarnation:
/// two ids for the same index from different allocations would compare equal. The stamp — bumped
/// once per reclaim, so it is the generation of the allocation the id came from — makes equality
/// name the incarnation. An embedder that keys anything on slot identity (Koan's declaration-site
/// handle, which decides rebind-vs-redeclare) needs that distinction to survive reuse. It is not a
/// debug aid: it carries in release builds, because the decisions that read it do.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId {
    index: usize,
    stamp: u32,
}

impl NodeId {
    /// The store's mint for a **fresh** index (extending the slot table), at the first generation.
    /// `pub(crate)`, and the node store's slot allocator is its only caller, so every id in
    /// circulation names a slot that allocator handed out.
    pub(crate) const fn new(index: usize) -> Self {
        NodeId { index, stamp: 0 }
    }

    /// The next generation of this index — what reclamation stamps onto the id it returns to the
    /// free list, so the allocation that pops it names a different incarnation than the one that
    /// just died. Wrapping: a run that reuses one index four billion times over is beyond any
    /// declaration-site handle's reach, and the alternative is a panic in a bookkeeping verb.
    pub(crate) const fn next_generation(self) -> Self {
        NodeId {
            index: self.index,
            stamp: self.stamp.wrapping_add(1),
        }
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
