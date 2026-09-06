//! [`Mask`] — a value's reach: the set of regions whose storage the value's borrows read. Slab
//! slots as an inline [`Bits`] row, sealed regions as a sparse id set, so union and dedup are a
//! word `OR` plus a sorted merge and membership is a bit test or a binary search. See
//! [design/liveness-matrix.md](../design/liveness-matrix.md) § Reach as a hybrid mask.
//!
//! A mask is never handed to a caller beside a bare value: it exists only inside a
//! [`Sealed`](crate::Sealed) carrier or a stored continuation, and the type is crate-private, so
//! the pairing of a value with its reach cannot be fabricated from outside.

use crate::matrix::Bits;
use crate::sealed::{SealedId, SealedSet};

/// The set of regions a value's borrows reach: slab slots as bits over the table's width, sealed
/// regions as ids.
///
/// The dense half is inline, so a mask naming no sealed region is built, copied, and compared
/// without touching the allocator — which is what keeps a reach off the per-value allocation path.
///
/// Masks compose by `OR` and merge, so a value built from several operands names the union of
/// their reaches with no deduplication step and no subsumption fold.
///
/// **The type is crate-private**, which is what makes a loose value-plus-mask pair
/// unrepresentable: a caller cannot mint a reach of its own choosing and hand it to a placement
/// door beside a value, so the only reach a value ever travels with is the one a door composed
/// for it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Mask<const W: usize> {
    slab: Bits<W>,
    sealed: SealedSet,
}

impl<const W: usize> Mask<W> {
    /// A mask naming nothing — the reach of a value whose borrows leave the table entirely.
    pub(crate) const fn empty() -> Self {
        Mask {
            slab: Bits::new(),
            sealed: SealedSet::new(),
        }
    }

    /// A mask naming exactly `slot` — the reach a value takes on the moment it is homed in a cell.
    pub(crate) fn single(slot: u32) -> Self {
        let mut mask = Mask::empty();
        mask.add(slot);
        mask
    }

    /// A mask naming exactly the sealed region `id` — the reach a value takes on when it is
    /// redeemed out of a record: a hold on the record keeps its aggregate alive transitively, so
    /// the id alone covers everything the value reads.
    pub(crate) fn single_sealed(id: SealedId) -> Self {
        let mut mask = Mask::empty();
        mask.add_sealed(id);
        mask
    }

    /// A mask over an already-built slab row and a sealed half — how a dying cell's hold set is
    /// frozen into its aggregate.
    pub(crate) fn from_parts(slab: Bits<W>, sealed: SealedSet) -> Self {
        Mask { slab, sealed }
    }

    pub(crate) fn add(&mut self, slot: u32) {
        self.slab.set(slot);
    }

    /// Add a sealed id, reporting whether it was absent. A merge reads the answer: an id already
    /// in the target's set is a hold the source's copy of duplicates rather than adds.
    pub(crate) fn add_sealed(&mut self, id: SealedId) -> bool {
        self.sealed.insert(id)
    }

    /// Clear one slab bit, reporting whether it was set — how a merge strikes the dead cell's own
    /// name out of a mask that named it.
    pub(crate) fn remove_slot(&mut self, slot: u32) -> bool {
        self.slab.clear(slot)
    }

    /// Drop one sealed id, reporting whether it was there — how a seal-time merge strikes the
    /// absorbed record's id out of the aggregate that named it.
    pub(crate) fn remove_sealed(&mut self, id: SealedId) -> bool {
        self.sealed.remove(id)
    }

    /// Trade a slab bit for a sealed id — the seal transition's rewrite, applied to one stored
    /// mask. Reports whether the bit was there, so a caller can skip the masks that never named
    /// the dying slot.
    pub(crate) fn replace_slot(&mut self, slot: u32, id: SealedId) -> bool {
        if !self.slab.clear(slot) {
            return false;
        }
        self.sealed.insert(id);
        true
    }

    /// Fold `other`'s reach into this one. The composition rule for a value built over operands:
    /// it reaches everything every operand reaches.
    pub(crate) fn union_with(&mut self, other: &Mask<W>) {
        self.slab.union_with(&other.slab);
        self.sealed.union_with(&other.sealed);
    }

    /// Fold only `other`'s slab half in. A merge folds the sparse half id by id instead, since it
    /// needs each insert's answer to tell a transferred hold from a duplicated one.
    pub(crate) fn union_slab_with(&mut self, other: &Mask<W>) {
        self.slab.union_with(&other.slab);
    }

    /// Whether the value's borrows reach the live cell in `slot`.
    pub(crate) fn names(&self, slot: u32) -> bool {
        self.slab.test(slot)
    }

    /// Whether the value's borrows reach the sealed region `id`.
    pub(crate) fn names_sealed(&self, id: SealedId) -> bool {
        self.sealed.contains(id)
    }

    /// The slab slots this mask names, in slot order.
    pub(crate) fn slab_slots(&self) -> impl Iterator<Item = u32> + '_ {
        self.slab.ones()
    }

    /// The sealed half.
    pub(crate) fn sealed(&self) -> &SealedSet {
        &self.sealed
    }

    /// The sealed half, taken out of a mask that has no further use — a retiring record's
    /// aggregate, whose ids the caller releases.
    pub(crate) fn into_sealed(self) -> SealedSet {
        self.sealed
    }

    /// The dense half, for the mint's OR.
    pub(crate) fn slab(&self) -> &Bits<W> {
        &self.slab
    }
}
