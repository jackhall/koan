//! [`Mask`] — a value's reach: the set of regions whose storage the value's borrows read. Slab
//! slots as a fixed-width bitmask, sealed regions as a sparse id set, so union and dedup are a
//! word `OR` plus a sorted merge and membership is a bit test or a binary search. See
//! [design/liveness-matrix.md](../design/liveness-matrix.md) § Reach as a hybrid mask.
//!
//! A mask is never handed to a caller beside a bare value: it exists only inside a
//! [`Sealed`](crate::Sealed) or [`Opened`](crate::Opened) carrier, and every constructor here is
//! crate-private, so the pairing of a value with its reach cannot be fabricated from outside.

use crate::sealed::{SealedId, SealedSet};

/// Words needed to hold `cap` bits.
pub(crate) fn words_for(cap: u32) -> usize {
    (cap as usize).div_ceil(64)
}

/// The set of regions a value's borrows reach: slab slots as bits over the table's cap, sealed
/// regions as ids.
///
/// Masks compose by `OR` and merge, so a value built from several operands names the union of
/// their reaches with no deduplication step and no subsumption fold.
///
/// **Every constructor is crate-private**, which is what makes a loose value-plus-mask pair
/// unrepresentable: a caller cannot mint a reach of its own choosing and hand it to a placement
/// door beside a value, so the only reach a value ever travels with is the one a door composed
/// for it.
///
/// ```compile_fail
/// // No public constructor: a caller cannot fabricate a reach to pair with a value.
/// let forged = cellgraph::Mask::empty(4);
/// ```
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Mask {
    words: Box<[u64]>,
    sealed: SealedSet,
}

impl Mask {
    /// A mask naming nothing — the reach of a value whose borrows leave the table entirely.
    pub(crate) fn empty(cap: u32) -> Self {
        Mask {
            words: vec![0u64; words_for(cap)].into_boxed_slice(),
            sealed: SealedSet::new(),
        }
    }

    /// A mask naming exactly `slot` — the reach a value takes on the moment it is homed in a cell.
    pub(crate) fn single(cap: u32, slot: u32) -> Self {
        let mut mask = Mask::empty(cap);
        mask.add(slot);
        mask
    }

    /// A mask over a copied-out matrix row and a sealed half — how a dying cell's hold set is
    /// frozen into its aggregate.
    pub(crate) fn with_words(words: &[u64], sealed: SealedSet) -> Self {
        Mask {
            words: words.to_vec().into_boxed_slice(),
            sealed,
        }
    }

    pub(crate) fn add(&mut self, slot: u32) {
        self.words[slot as usize / 64] |= 1u64 << (slot % 64);
    }

    pub(crate) fn add_sealed(&mut self, id: SealedId) {
        self.sealed.insert(id);
    }

    /// Trade a slab bit for a sealed id — the seal transition's rewrite, applied to one stored
    /// mask. Reports whether the bit was there, so a caller can skip the masks that never named
    /// the dying slot.
    pub(crate) fn replace_slot(&mut self, slot: u32, id: SealedId) -> bool {
        if !self.names(slot) {
            return false;
        }
        self.words[slot as usize / 64] &= !(1u64 << (slot % 64));
        self.sealed.insert(id);
        true
    }

    /// Fold `other`'s reach into this one. The composition rule for a value built over operands:
    /// it reaches everything every operand reaches.
    pub(crate) fn union_with(&mut self, other: &Mask) {
        for (word, source) in self.words.iter_mut().zip(other.words.iter()) {
            *word |= *source;
        }
        self.sealed.union_with(&other.sealed);
    }

    /// Whether the value's borrows reach the live cell in `slot`.
    pub fn names(&self, slot: u32) -> bool {
        self.words[slot as usize / 64] & (1u64 << (slot % 64)) != 0
    }

    /// Whether the value's borrows reach the sealed region `id`.
    pub fn names_sealed(&self, id: SealedId) -> bool {
        self.sealed.contains(id)
    }

    pub fn is_empty(&self) -> bool {
        self.words.iter().all(|word| *word == 0) && self.sealed.is_empty()
    }

    /// The slab slots this mask names, in slot order.
    pub fn slab_slots(&self, cap: u32) -> impl Iterator<Item = u32> + '_ {
        (0..cap).filter(|slot| self.names(*slot))
    }

    /// The sealed half.
    pub fn sealed(&self) -> &SealedSet {
        &self.sealed
    }

    pub(crate) fn words(&self) -> &[u64] {
        &self.words
    }
}
