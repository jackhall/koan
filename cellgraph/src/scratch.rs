//! The table's scratch region: the one place a verb's transients live. Reset at the entry of
//! every verb, never inside one, so a value in it lives exactly as long as the verb that built
//! it. See [design/cellgraph.md](../design/cellgraph.md) § Verbs.
//!
//! A reset runs no destructor — a bump releases its chunks whole — which is why nothing with drop
//! glue may go in it. The doors below assert that at compile time, the way the region doors assert
//! [`DropFree`](crate::DropFree).

use bumpalo::Bump;

use crate::sealed::ScratchSet;

/// A growable transient, homed in the scratch region rather than on the heap.
pub(crate) type ScratchVec<'s, T> = bumpalo::collections::Vec<'s, T>;

/// One table's scratch bump, sized at construction and reset at every verb's entry.
#[derive(Default)]
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
    pub(crate) fn reset(&mut self) {
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

    /// An empty sorted id set in scratch — what a walk seeds its seen set from.
    pub(crate) fn ids(&self) -> ScratchSet<'_> {
        ScratchSet::over(self.vec())
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
