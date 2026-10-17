//! What rebuilding a value at a destination costs, and where its types already resolve: two memos
//! set on every composite at construction, so a crossing or an adoption reads them off the value
//! instead of walking it.
//!
//! The byte count is the weight proper. The two flags in its top bits, [`MemoBits`], record where
//! every type memo the value reaches resolves; an adoption does not enter a part that carries one
//! ([design/type-tiers.md § An adoption stops at a settled part](../../design/type-tiers.md#an-adoption-stops-at-a-settled-part)).
//! [settled.rs](settled.rs) computes them.

/// Set when every memo the value reaches is the root's.
const ROOT_CLOSED: usize = 1 << (usize::BITS - 1);
/// Set when every memo the value reaches resolves in the root or in the branch of the half the
/// value lies in, below a part whose own bit answers for what lies under it.
const SETTLED: usize = 1 << (usize::BITS - 2);
/// The byte count's bits, where every sum saturates.
const BYTES: usize = SETTLED - 1;

/// Bytes a total rebuild of a value writes into a destination region, saturating below the two
/// [`MemoBits`] it carries in its top bits.
///
/// A composite's weight is its own resident struct plus every cell it lays down; a cell is a whole
/// [`Value`](super::Value) word plus whatever that word points at in the region. Program storage
/// weighs nothing past the pointer: a quoted expression crosses every verdict as the same node.
/// Equality and order read the bytes alone.
#[derive(Clone, Copy)]
pub struct Weight(usize);

/// Where a value's type memos resolve, as its [`Weight`] records it. A clear bit is always safe.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MemoBits {
    /// Every memo the value reaches is the root's.
    pub root_closed: bool,
    /// Every memo the value reaches resolves in the root or its home half's branch.
    pub settled: bool,
}

impl MemoBits {
    /// Nothing known: an adoption walks the value.
    pub const NONE: MemoBits = MemoBits {
        root_closed: false,
        settled: false,
    };

    /// Both set: a leaf, or program storage.
    pub const BOTH: MemoBits = MemoBits {
        root_closed: true,
        settled: true,
    };

    /// Whether either bit is set — whether an adoption may stop at the value.
    pub const fn meets(self) -> bool {
        self.root_closed || self.settled
    }
}

impl Weight {
    /// The weight of nothing.
    pub const ZERO: Weight = Weight(0);

    /// One `T` laid down in a region.
    pub const fn flat<T>() -> Weight {
        Weight::saturated(size_of::<T>())
    }

    /// A run of `len` bytes — a string's text.
    pub const fn text(len: usize) -> Weight {
        Weight::saturated(len)
    }

    /// A run of `len` `T`s laid down in a region — a function's quantifier map.
    pub const fn run<T>(len: usize) -> Weight {
        Weight::saturated(size_of::<T>().saturating_mul(len))
    }

    /// Both weights' bytes, saturating; the sum carries no bits.
    pub const fn plus(self, other: Weight) -> Weight {
        Weight::saturated(self.bytes().saturating_add(other.bytes()))
    }

    /// The figure [`Operand::copy_bytes`](crate::memory::Operand) takes.
    pub const fn bytes(self) -> usize {
        self.0 & BYTES
    }

    /// The memo bits the weight carries.
    pub const fn bits(self) -> MemoBits {
        MemoBits {
            root_closed: self.0 & ROOT_CLOSED != 0,
            settled: self.0 & SETTLED != 0,
        }
    }

    /// The same bytes under `bits`.
    pub const fn with_bits(self, bits: MemoBits) -> Weight {
        let mut word = self.bytes();
        if bits.root_closed {
            word |= ROOT_CLOSED;
        }
        if bits.settled {
            word |= SETTLED;
        }
        Weight(word)
    }

    /// The same bytes with both bits clear.
    pub const fn without_bits(self) -> Weight {
        Weight(self.bytes())
    }

    /// `bytes` clamped below the bits.
    const fn saturated(bytes: usize) -> Weight {
        Weight(if bytes > BYTES { BYTES } else { bytes })
    }
}

impl PartialEq for Weight {
    fn eq(&self, other: &Self) -> bool {
        self.bytes() == other.bytes()
    }
}

impl Eq for Weight {}

impl PartialOrd for Weight {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Weight {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.bytes().cmp(&other.bytes())
    }
}

impl std::fmt::Debug for Weight {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let bits = self.bits();
        f.debug_struct("Weight")
            .field("bytes", &self.bytes())
            .field("root_closed", &bits.root_closed)
            .field("settled", &bits.settled)
            .finish()
    }
}
