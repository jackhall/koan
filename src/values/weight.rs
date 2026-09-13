//! What rebuilding a value at a destination costs: the bytes a deep copy writes, memoized on every
//! composite at construction so a crossing reads it off the value instead of walking it.

/// Bytes a total rebuild of a value writes into a destination region, saturating at `usize::MAX`.
///
/// A composite's weight is its own resident struct plus every cell it lays down; a cell is a whole
/// [`Value`](super::Value) word plus whatever that word points at in the region. Program storage
/// weighs nothing past the pointer: a quoted expression crosses every verdict as the same node.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Weight(usize);

impl Weight {
    /// The weight of nothing.
    pub const ZERO: Weight = Weight(0);

    /// One `T` laid down in a region.
    pub const fn flat<T>() -> Weight {
        Weight(size_of::<T>())
    }

    /// A run of `len` bytes — a string's text.
    pub const fn text(len: usize) -> Weight {
        Weight(len)
    }

    /// Both weights, saturating.
    pub const fn plus(self, other: Weight) -> Weight {
        Weight(self.0.saturating_add(other.0))
    }

    /// The figure [`Operand::copy_bytes`](crate::memory::Operand) takes.
    pub const fn bytes(self) -> usize {
        self.0
    }
}
