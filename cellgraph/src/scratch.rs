//! The table's scratch region: the one place a verb's transients live. Reset at the entry of
//! every verb, never inside one, so a value in it lives exactly as long as the verb that built
//! it. See [design/cellgraph.md](../design/cellgraph.md) § Verbs.
//!
//! A reset runs no destructor — a bump releases its chunks whole — which is why nothing with drop
//! glue may go in it. The doors below assert that at compile time, the way the region doors assert
//! [`DropFree`](crate::DropFree).

use bumpalo::Bump;

use crate::sealed::{IdBuffer, IdSet, ScratchSet};

/// A growable transient, homed in the scratch region rather than on the heap.
pub(crate) type ScratchVec<'s, T> = bumpalo::collections::Vec<'s, T>;

/// One table's scratch bump, sized at construction and reset at every verb's entry.
///
/// Deliberately not `Default`: a table has exactly one region, minted with its first chunk, and
/// every path that moves it moves that one — there is no shape of this type worth conjuring.
pub(crate) struct Scratch {
    bump: Bump,
}

impl Scratch {
    /// Bytes the first chunk is sized to at construction: two per-operand view lists for a wide
    /// placement plus the disposal cascade's nested id worklists fit with room to spare.
    ///
    /// Paid here rather than at the first transient, which is what keeps the region off every
    /// verb's allocation reading: construction is unmetered, like the slab itself.
    const FIRST_CHUNK: usize = 4096;

    pub(crate) fn new() -> Self {
        Scratch {
            bump: Bump::with_capacity(Self::FIRST_CHUNK),
        }
    }

    /// Drop every transient at once. Keeps the largest chunk, so a table is warm again from the
    /// next verb.
    ///
    /// A region that is already empty is left alone. The equality holds only for a single chunk
    /// with nothing handed out of it — two chunks make the total exceed what the current one has
    /// left — and that is the state most verbs start in, since a bump gives the bytes back when a
    /// transient dropped last is freed. So the common entry costs two loads rather than the chunk
    /// walk and finger rewind.
    pub(crate) fn reset(&mut self) {
        if self.bump.allocated_bytes() == self.bump.chunk_capacity() {
            return;
        }
        self.bump.reset();
    }

    pub(crate) fn vec<T>(&self) -> ScratchVec<'_, T> {
        const { assert!(!std::mem::needs_drop::<T>()) };
        ScratchVec::new_in(&self.bump)
    }

    pub(crate) fn vec_with_capacity<T>(&self, capacity: usize) -> ScratchVec<'_, T> {
        const { assert!(!std::mem::needs_drop::<T>()) };
        ScratchVec::with_capacity_in(capacity, &self.bump)
    }

    /// A run of exactly `len` transients, filled in index order.
    ///
    /// What a door builds when the count is known before the first element: no capacity to track,
    /// no growth path to take, and no vector header to carry. A per-operand list is a run, not a
    /// collection that might grow, and saying so is what keeps the placement path cheap.
    pub(crate) fn slice_with<T>(&self, len: usize, fill: impl FnMut(usize) -> T) -> &mut [T] {
        const { assert!(!std::mem::needs_drop::<T>()) };
        self.bump.alloc_slice_fill_with(len, fill)
    }

    /// An empty sorted id set in scratch — what a walk with nothing already seen starts from.
    pub(crate) fn ids(&self) -> ScratchSet<'_> {
        ScratchSet::over(self.vec())
    }

    /// A sorted id set in scratch holding what `other` holds — a walk's seen set, seeded with the
    /// ids the question already covers.
    pub(crate) fn ids_from(&self, other: &IdSet<impl IdBuffer>) -> ScratchSet<'_> {
        ScratchSet::copy_of(self.vec_with_capacity(other.len()), other)
    }

    /// Chunk bytes the region holds, handed out or not. The figure a warm-table test reads: a
    /// table whose chunk already fits a verb's transients grows this by nothing.
    #[cfg(test)]
    pub(crate) fn capacity(&self) -> usize {
        self.bump.allocated_bytes()
    }

    /// Bytes currently handed out — zero right after a reset.
    #[cfg(test)]
    pub(crate) fn in_use(&self) -> usize {
        self.bump.allocated_bytes() - self.bump.chunk_capacity()
    }
}
