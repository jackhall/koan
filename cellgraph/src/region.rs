//! [`Region`] — the **cell region**: one cell's bump, the only place a value with reach may rest,
//! in pointer-stable chunks that a sealing cell's storage detaches with unmoved. See
//! [../README.md](../README.md) § The cell.
//!
//! The bump is lifetime-free, so a region borrow `'r` enters only at the allocating call. That is
//! what lets a value written here hold an `&'r` back into the very region it lives in with no
//! residence check: a lifetime-*typed* slot would have to name a lifetime a region has no
//! parameter for.
//!
//! Nothing stored in a region is ever dropped — a bump releases its chunks whole — which is why
//! every family a region hosts is [`DropFree`](crate::DropFree). The write surface is
//! [`Writer`], a `Copy` handle a step receives at a brand it cannot widen: a build closure's own
//! for a foreign destination, and the executing cell's `'cell` for its own region.
//!
//! A region is a **bundle** of bumps: the one it writes into, plus the bumps of every region
//! absorbed into it. Absorption is how a merge splices storage
//! ([graph/README.md § Locality tactics](graph/README.md#locality-tactics)) —
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
/// Holding a raw pointer is what makes a `Region` `!Send`, which costs nothing — a graph is not
/// `Send` either, since it carries the embedder's boxed verdict.
struct BumpRun<T: Copy> {
    ptr: NonNull<T>,
    len: usize,
}

/// One cell's storage: the bump it writes into, plus the bumps it has absorbed. An empty bump
/// claims no chunk, so a region a cell never writes into costs nothing to have.
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
    /// The frozen-closure memo of the sealed cell this region belongs to, written once by a price
    /// query and never cleared — see [`SealedCell`](crate::sealed::SealedCell) for why it can never
    /// go stale. Region state because its bytes are region bytes: the sealed cell's price counts
    /// them like any other chunk.
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

    /// The memoized sealed-cell set, or `None` while nothing has primed it.
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
    /// which is what keeps the sealed cell's retained total in step — `0` when a memo is already
    /// there.
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

    /// The write surface at a caller-chosen `'r` — the executing cell's own writer, minted once at
    /// `enter` and handed to the step for the whole of it.
    ///
    /// The `&self` is load-bearing beyond the borrow it widens: a bump is interior-mutable
    /// throughout, so a *shared* borrow of a region tolerates every read and write the graph's
    /// other paths make into the same bump while this writer is out. An exclusive borrow would
    /// not — it is unique over those bytes whatever their type — so the first price query to walk
    /// this region would end the writer's life.
    ///
    /// # Safety
    ///
    /// `'r` must lie within one step of the cell this region belongs to: for all of `'r` the region
    /// is neither moved off its slot, taken, nor dropped. The graph verbs that do any of those
    /// (`release`, `release_tree`, disposal) cannot run inside a step, because `enter` holds the
    /// graph exclusively for its whole length. The caller must also reach this region through a
    /// shared borrow, for the reason above.
    pub(crate) unsafe fn writer_at<'r>(&self) -> Writer<'r> {
        // SAFETY: see the contract. The `Bump` sits inline in a slab slot (a `Box<[SlabCell]>`
        // that is never reallocated) or in a tree-pool entry, and a bump's chunks are heap
        // allocations it never moves — so the bytes this reference names stay where they are for
        // all of `'r`, and only a graph verb could take the region away.
        Writer(unsafe { &*(&self.bump as *const Bump) })
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
        // An empty bundle takes the source over whole rather than listing it, which is what keeps a
        // merge into a cell that never wrote off the allocator. Nothing of this region's is lost:
        // a bump with no chunk holds no value, and a memo would have cost bytes.
        if self.bump.allocated_bytes() == 0 && self.absorbed.is_empty() {
            *self = other;
            return;
        }
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

    /// Splice an optional region into a region — the storage half of every merge into a sealed
    /// cell. A source with no region contributes nothing.
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

/// The write surface into a region's bytes, at the brand the door that hands one out chose: a
/// build closure's own, or the executing cell's `'cell`.
///
/// `Copy` with a private field, so a writer exists only where the graph hands one out, and every
/// verb returns a shared `&'r` rather than the `&mut` the bump itself yields: a written value is
/// region state its holder names, never one it owns.
///
/// Two verbs, because two is what a region cannot be given by an embedder: [`fill`](Writer::fill)
/// lays down a run by index under a compile-time no-destructor check, and [`text`](Writer::text)
/// writes a `str`, whose bytes have no `T` to index by. Every simpler shape — one value, a copied
/// slice, a run collected from an iterator — is the embedder's, derived from `fill`.
#[derive(Clone, Copy)]
pub struct Writer<'r>(&'r Bump);

impl<'r> Writer<'r> {
    /// Write a run of `len` values, each built from its index, and hand back the borrow of it that
    /// lives in the region.
    ///
    /// A bump releases its chunks whole and never walks a value, so `T` must run no destructor.
    /// The check is a `const` assert on `T` rather than a `T: Copy` bound: a bound would also
    /// refuse the interior mutability a cell-resident table needs (`Cell<u32>` is drop-free but
    /// not `Copy`), and the assert fires at the instantiation site either way.
    pub fn fill<T>(self, len: usize, fill: impl FnMut(usize) -> T) -> &'r [T] {
        const { assert!(!std::mem::needs_drop::<T>()) };
        self.0.alloc_slice_fill_with(len, fill)
    }

    /// Write text.
    pub fn text(self, text: &str) -> &'r str {
        self.0.alloc_str(text)
    }
}
