//! The **scratch region**: the one place a verb's transients live. Reset at the entry of every
//! verb, never inside one, so a value in it lives exactly as long as the verb that built it. See
//! [../README.md](../README.md) § Verbs.
//!
//! A reset runs no destructor — a bump releases its chunks whole — which is why nothing with drop
//! glue may go in it. The doors below assert that at compile time, the way the cell-region doors
//! assert [`DropFree`](crate::DropFree).
//!
//! One graph has one scratch region and each cell has its own **cell region**
//! ([`Region`](crate::region::Region)); both are bumps, and the qualifier is what tells them
//! apart: outside these two modules' own bodies, "the region" alone names neither.

use std::cell::Cell;

use bumpalo::Bump;

use crate::sealed::{IdBuffer, IdSet, ScratchSet};

/// A transient whose length is not known until it is built — a worklist a walk pushes onto — with
/// its buffer in the scratch region rather than on the heap.
///
/// `allocator_api2`'s `Vec` over `&Bump` rather than `bumpalo::collections::Vec`: the former is a
/// fork of std's, so a push, a pop and an `extend_from_slice` are the specialized, inlined shapes
/// the rest of the crate is measured against, and it derefs to `[T]` so a slice-taking callee is
/// reached unchanged. Same alias workgraph's `BumpVec` is, over the same two crates.
///
/// A door mints one only for a `T` with no drop glue, so nothing is lost by the reset that ends
/// its life without running a destructor. Growth abandons the old buffer as dead region bytes, so
/// a door with the final length to hand takes [`Scratch::run`] instead.
pub(crate) type ScratchVec<'s, T> = allocator_api2::vec::Vec<T, &'s Bump>;

/// One graph's scratch region, sized at construction and reset at every verb's entry.
///
/// Deliberately not `Default`: a graph has exactly one scratch region, minted with its first
/// chunk, and every path that moves it moves that one — there is no shape of this type worth
/// conjuring.
pub(crate) struct Scratch {
    bump: Bump,
    /// Whether a door has handed anything out since the last reset. `Cell`, because the doors take
    /// `&self` — a transient has to be able to coexist with the next one.
    dirty: Cell<bool>,
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
            dirty: Cell::new(false),
        }
    }

    /// Drop every transient at once. Keeps the largest chunk, so a graph is warm again from the
    /// next verb.
    ///
    /// A region no door has been through since the last reset is left alone, and that is the state
    /// most verbs start in — a verb builds no transient at all unless it places or cascades. The
    /// flag is what makes the common entry one load: the region's own occupancy figures would
    /// answer the same question, but each of them walks the chunk list to do it.
    pub(crate) fn reset(&mut self) {
        if !self.dirty.replace(false) {
            return;
        }
        self.bump.reset();
    }

    /// Record that a door is about to hand something out, so the next verb's entry knows to clear
    /// it. Conservative by one reset: a vector that is never pushed onto takes no bytes.
    fn opening(&self) {
        self.dirty.set(true);
    }

    /// A transient that grows — a worklist whose length the walk pushing onto it decides.
    ///
    /// The shape a door falls back to. Where the final length is known before the first element,
    /// [`slice_with`](Self::slice_with) builds the same contents without a capacity to track, a
    /// growth path to take, or a header to carry.
    pub(crate) fn vec<T>(&self) -> ScratchVec<'_, T> {
        const { assert!(!std::mem::needs_drop::<T>()) };
        self.opening();
        ScratchVec::new_in(&self.bump)
    }

    /// [`vec`](Self::vec) sized up front, for a worklist that starts from a run of known length and
    /// grows from there. Growth strands the buffer it outgrew, so the capacity is what keeps the
    /// common case — a walk that pushes nothing — down to one allocation.
    pub(crate) fn vec_with_capacity<T>(&self, capacity: usize) -> ScratchVec<'_, T> {
        const { assert!(!std::mem::needs_drop::<T>()) };
        self.opening();
        ScratchVec::with_capacity_in(capacity, &self.bump)
    }

    /// A run of exactly `len` transients, filled in index order.
    ///
    /// What a door builds when the count is known before the first element. A per-operand list is a
    /// run, not a collection that might grow, and saying so is what keeps the placement path cheap.
    pub(crate) fn slice_with<T>(&self, len: usize, fill: impl FnMut(usize) -> T) -> &mut [T] {
        const { assert!(!std::mem::needs_drop::<T>()) };
        self.opening();
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

    /// Chunk bytes the region holds, handed out or not. The figure a warm-graph test reads: a
    /// graph whose chunk already fits a verb's transients grows this by nothing.
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
