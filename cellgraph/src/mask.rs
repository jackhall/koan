//! [`Mask`] — a value's reach: the set of cells whose region storage the value's borrows read.
//! Fixed at the table's cap, one bit per slab slot, so union and dedup are a word `OR` and
//! membership is a bit test. See
//! [design/liveness-matrix.md](../design/liveness-matrix.md) § Reach as a hybrid mask.
//!
//! A mask is never handed to a caller beside a bare value: it exists only inside a
//! [`Sealed`](crate::Sealed) or [`Opened`](crate::Opened) carrier, and every constructor here is
//! crate-private, so the pairing of a value with its reach cannot be fabricated from outside.

/// Words needed to hold `cap` bits.
pub(crate) fn words_for(cap: u32) -> usize {
    (cap as usize).div_ceil(64)
}

/// The set of slab slots a value's borrows reach, as a fixed-width bitmask over the table's cap.
///
/// Masks compose by `OR`, so a value built from several operands names the union of their reaches
/// with no deduplication step and no subsumption fold. Cloning is a word copy of the whole row.
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
}

impl Mask {
    /// A mask naming nothing — the reach of a value whose borrows leave the table entirely.
    pub(crate) fn empty(cap: u32) -> Self {
        Mask {
            words: vec![0u64; words_for(cap)].into_boxed_slice(),
        }
    }

    /// A mask naming exactly `slot` — the reach a value takes on the moment it is homed in a cell.
    pub(crate) fn single(cap: u32, slot: u32) -> Self {
        let mut mask = Mask::empty(cap);
        mask.add(slot);
        mask
    }

    pub(crate) fn add(&mut self, slot: u32) {
        self.words[slot as usize / 64] |= 1u64 << (slot % 64);
    }

    /// Fold `other`'s reach into this one. The composition rule for a value built over operands:
    /// it reaches everything every operand reaches.
    pub(crate) fn union_with(&mut self, other: &Mask) {
        for (word, source) in self.words.iter_mut().zip(other.words.iter()) {
            *word |= *source;
        }
    }

    /// Whether the value's borrows reach `slot`'s region.
    pub fn names(&self, slot: u32) -> bool {
        self.words[slot as usize / 64] & (1u64 << (slot % 64)) != 0
    }

    pub fn is_empty(&self) -> bool {
        self.words.iter().all(|word| *word == 0)
    }

    pub(crate) fn words(&self) -> &[u64] {
        &self.words
    }
}
