//! [`Region`] — the **cell region**: one cell's bump, the only place a value with reach may rest,
//! in pointer-stable chunks that a sealing cell's storage detaches with unmoved. See
//! [design/cellgraph.md](../design/cellgraph.md) § The cell.
//!
//! The bump is lifetime-free, so a region borrow `'r` enters only at the allocating call. That is
//! what lets a value written here hold an `&'r` back into the very region it lives in with no
//! residence check: a lifetime-*typed* slot would have to name a lifetime a region has no
//! parameter for.
//!
//! Nothing stored in a region is ever dropped — a bump releases its chunks whole — which is why
//! every family a region hosts is [`DropFree`](crate::DropFree). The write surface is
//! [`Writer`], a `Copy` handle a step receives inside a build closure's brand and cannot widen.
//!
//! A region is a **bundle** of bumps: the one it writes into, plus the bumps of every region
//! absorbed into it. Absorption is how a merge splices storage
//! ([liveness-matrix.md § Locality tactics](../design/liveness-matrix.md#locality-tactics)) —
//! a `Bump` moves without moving a chunk byte, so the pointer stability a detached seal already
//! relies on carries a borrow across the merge unchanged.

use std::cell::OnceCell;
use std::ptr::NonNull;

use bumpalo::Bump;

use crate::sealed::SealedId;

#[cfg(test)]
mod tests;

/// A run of `Copy` values written into this region's own bump and read back only through the
/// region that wrote it.
///
/// Private to this module: it has no `Deref`, no public constructor, and no `Drop`, so the only
/// way to reach the values is [`Region::memo`], whose `&self` bounds the slice it hands back.
/// Holding a raw pointer is what makes a `Region` `!Send`, which costs nothing — a table is not
/// `Send` either, since it carries the embedder's boxed verdict.
struct BumpRun<T: Copy> {
    ptr: NonNull<T>,
    len: usize,
}

/// One cell's storage: the bump it writes into, plus the bumps it has absorbed. Minted lazily at
/// the cell's first allocation, so a cell that never allocates costs no chunk.
pub(crate) struct Region {
    bump: Bump,
    /// Bumps merged in from regions this one absorbed. Read-only from here on — nothing is ever
    /// allocated into an absorbed bump again — but their chunks stay at their addresses, which is
    /// what the borrows minted before the merge still name.
    absorbed: Vec<Bump>,
    /// What those bumps' chunks occupy, maintained at every merge. A running total rather than a
    /// walk: the byte figure is read once per priced operand, and a chain of splices would
    /// otherwise make each reading linear in the bundle it has accumulated.
    absorbed_bytes: usize,
    /// The frozen-closure memo of the record this region belongs to, written once by a price query
    /// and never cleared — see [`SealedRecord`](crate::sealed::SealedRecord) for why it can never
    /// go stale. Region state because its bytes are region bytes: the record's price counts them
    /// like any other chunk.
    memo: OnceCell<BumpRun<SealedId>>,
}

impl Region {
    pub(crate) fn new() -> Self {
        Region {
            bump: Bump::new(),
            absorbed: Vec::new(),
            absorbed_bytes: 0,
            memo: OnceCell::new(),
        }
    }

    /// The memoized record set, or `None` while nothing has primed it.
    pub(crate) fn memo(&self) -> Option<&[SealedId]> {
        let run = self.memo.get()?;
        // SAFETY: the `BumpRun` is a private field of this region, minted by `set_memo` out of this
        // region's own bump and reachable through no other path. The bump is never reset and frees
        // its chunks only when this `Region` drops, and moving the `Bump` moves no chunk byte, so
        // the run stays where it was written for as long as the region lives. `SealedId: Copy`, so
        // nothing there was ever dropped in place. The returned borrow is bounded by `&self`.
        Some(unsafe { std::slice::from_raw_parts(run.ptr.as_ptr(), run.len) })
    }

    /// Write the memo into this region's own bytes, once. Reports the chunk bytes the write cost,
    /// which is what keeps the record's retained total in step — `0` when a memo is already there.
    pub(crate) fn set_memo(&self, ids: &[SealedId]) -> usize {
        if self.memo.get().is_some() {
            return 0;
        }
        let before = self.allocated_bytes();
        let written = self.bump.alloc_slice_copy(ids);
        let run = BumpRun {
            // An empty run gets bumpalo's dangling, aligned pointer, which `from_raw_parts` takes
            // at length zero.
            ptr: NonNull::from(&mut *written).cast::<SealedId>(),
            len: written.len(),
        };
        let _ = self.memo.set(run);
        self.allocated_bytes() - before
    }

    pub(crate) fn writer(&self) -> Writer<'_> {
        Writer(&self.bump)
    }

    /// Take `other`'s chunks into this bundle. The bumps move; the chunks do not, so a borrow
    /// minted before the merge still names its bytes — `other`'s own memo included, whose `BumpRun`
    /// goes with `other` and leaves its bytes behind as a bump's dead bytes.
    ///
    /// The shorter list moves into the longer one rather than the source into the target, which is
    /// what makes a splice up a chain O(1) apiece: a cell absorbing a bundle far larger than its
    /// own takes that bundle over instead of copying it in. Order carries no meaning here — the
    /// list exists to keep the chunks alive and to total their bytes — so the swap costs nothing.
    fn absorb(&mut self, mut other: Region) {
        if self.absorbed.len() < other.absorbed.len() {
            std::mem::swap(&mut self.absorbed, &mut other.absorbed);
        }
        self.absorbed.append(&mut other.absorbed);
        // A bump that never allocated owns no chunk, so taking it in would only lengthen the list.
        if other.bump.allocated_bytes() > 0 {
            self.absorbed_bytes += other.bump.allocated_bytes();
            self.absorbed.push(other.bump);
        }
        self.absorbed_bytes += other.absorbed_bytes;
    }

    /// Splice an optional region into a region — the storage half of every merge into a record. A
    /// source with no region contributes nothing.
    pub(crate) fn splice(into: &mut Region, from: Option<Region>) {
        if let Some(from) = from {
            into.absorb(from);
        }
    }

    /// Splice one optional region into another — the storage half of a merge into a slab cell,
    /// whose region is minted lazily. A target with none takes the source whole.
    pub(crate) fn splice_optional(into: &mut Option<Region>, from: Option<Region>) {
        let Some(from) = from else {
            return;
        };
        match into {
            Some(target) => target.absorb(from),
            None => *into = Some(from),
        }
    }

    /// Bytes the chunks occupy, whether or not a value still uses them — a bump never reclaims
    /// within a chunk, so this is what the region costs while anything holds it. Absorbed bumps
    /// count: the bundle is answerable for every chunk it took in. O(1), off the running total the
    /// merges maintain, since a priced operand reads this figure and a bundle grows by splices.
    pub(crate) fn allocated_bytes(&self) -> usize {
        self.bump.allocated_bytes() + self.absorbed_bytes
    }
}

/// The write surface into a region's bytes, handed to a build closure at the closure's own brand.
///
/// `Copy` with a private field, so a writer exists only where the table hands one out, and every
/// verb returns a shared `&'r` rather than the `&mut` the bump itself yields: a written value is
/// region state its holder names, never one it owns. `T: Copy` on the value verbs is what stands
/// in for the missing destructor — a bump never runs one.
#[derive(Clone, Copy)]
pub struct Writer<'r>(&'r Bump);

impl<'r> Writer<'r> {
    /// Write one value and hand back the borrow of it that lives in the region.
    pub fn value<T: Copy>(self, value: T) -> &'r T {
        self.0.alloc(value)
    }

    /// Write a run of values contiguously.
    pub fn slice<T: Copy>(self, items: &[T]) -> &'r [T] {
        self.0.alloc_slice_copy(items)
    }

    /// Write text.
    pub fn text(self, text: &str) -> &'r str {
        self.0.alloc_str(text)
    }
}
