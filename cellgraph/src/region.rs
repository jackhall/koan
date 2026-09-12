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
//! Every live cell's region sits in one table, [`Regions`], that a step borrows **shared** for
//! its whole length while it holds the rest of the graph exclusively. The step's own writer is a
//! plain `&'cell` into that table, and every placement's writer a shorter borrow of it: two
//! writers may name one bump — a placement into the executing cell is exactly that, the build's
//! writer beside the step's own — and a bump's bytes are interior-mutable throughout, so shared
//! borrows of it tolerate each other's reads and writes. Every verb that moves or drops a region
//! takes the table exclusively, which no step can do, so no `&mut` over a live region's bytes can
//! exist under a writer into it: the borrow checker holds that line.
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

    /// The write surface for as long as this borrow lasts: a placement's, for the length of its
    /// build, or the executing cell's, for the length of the step that borrowed the table.
    pub(crate) fn writer(&self) -> Writer<'_> {
        Writer(&self.bump)
    }

    /// Take `other`'s chunks into this bundle — the storage half of every merge. The bumps move;
    /// the chunks do not, so a borrow minted before the merge still names its bytes — `other`'s
    /// own memo included, whose `BumpRun`
    /// goes with `other` and leaves its bytes behind as a bump's dead bytes.
    ///
    /// The shorter list moves into the longer one rather than the source into the target, which is
    /// what makes a splice up a chain O(1) apiece: a cell absorbing a bundle far larger than its
    /// own takes that bundle over instead of copying it in. Order carries no meaning here — the
    /// list exists to keep the chunks alive and to total their bytes — so the swap costs nothing.
    pub(crate) fn absorb(&mut self, mut other: Region) {
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

    /// Bytes the chunks occupy, whether or not a value still uses them — a bump never reclaims
    /// within a chunk, so this is what the region costs while anything holds it. Absorbed bumps
    /// count: the bundle is answerable for every chunk it took in. O(1), off the running total the
    /// merges maintain, since a priced operand reads this figure and a bundle grows by splices.
    pub(crate) fn allocated_bytes(&self) -> usize {
        self.bump.allocated_bytes() + self.absorbed_bytes
    }

    /// Whether the region is as it was built: no chunk claimed, nothing absorbed, no memo. Such a
    /// region owns nothing, so a slot that recycles with one can keep it rather than build another.
    fn is_untouched(&self) -> bool {
        self.bump.allocated_bytes() == 0 && self.absorbed.is_empty() && self.memo.get().is_none()
    }
}

/// Every live cell's region, slab and tree, in one table the graph keeps beside its cells.
///
/// Held apart from the cells so a step can borrow the two differently: the table shared, at the
/// step's `'cell` brand, and the cells exclusively. Every slot carries a region from the start —
/// an empty bump claims no chunk, so a cell that never writes costs the table one small struct
/// and nothing else — and a region leaves the table only through the `&mut` doors below, which
/// the graph verbs that dispose of a cell take outside any step. A writer at `'cell` therefore
/// names a bump nothing can move or drop for as long as it is out, by the borrow checker's word;
/// and since no `&mut` into the table exists inside a step, every write a step makes into a bump
/// descends from a shared borrow, which a bump's interior-mutable bytes tolerate.
pub(crate) struct Regions {
    /// One region per slab slot, replaced with an empty one when the slot recycles.
    slab: Box<[Region]>,
    /// One region per tree-pool index, every index up to the table's capacity filled. The table
    /// starts a slab's width long, like the pool itself, and doubles from there in step with it;
    /// a cell's creation finds its region already waiting, and pays a length check for it.
    tree: Vec<Region>,
}

impl Regions {
    pub(crate) fn new(cap: u32) -> Self {
        Regions {
            slab: (0..cap).map(|_| Region::new()).collect(),
            tree: (0..cap).map(|_| Region::new()).collect(),
        }
    }

    /// A slab cell's region.
    pub(crate) fn slab(&self, slot: u32) -> &Region {
        &self.slab[slot as usize]
    }

    /// A tree cell's region.
    pub(crate) fn tree(&self, index: u32) -> &Region {
        &self.tree[index as usize]
    }

    /// Chunk bytes a slab cell's region bundle occupies, `0` where it never allocated.
    pub(crate) fn slab_bytes(&self, slot: u32) -> usize {
        self.slab[slot as usize].allocated_bytes()
    }

    /// Chunk bytes a tree cell's region bundle occupies, `0` where it never allocated.
    pub(crate) fn tree_bytes(&self, index: u32) -> usize {
        self.tree[index as usize].allocated_bytes()
    }

    /// Make room for a tree-pool index the pool has just minted. Idempotent for one it already
    /// covers.
    pub(crate) fn reach_tree(&mut self, index: u32) {
        let index = index as usize;
        if self.tree.len() <= index {
            let needed = index + 1 - self.tree.len();
            self.tree.reserve(needed.max(self.tree.len()));
            let room = self.tree.capacity();
            self.tree.resize_with(room, Region::new);
        }
    }

    /// Take a slab cell's storage off it, leaving an empty region behind — for the seal, the
    /// merge or the drop that disposal performs.
    pub(crate) fn take_slab(&mut self, slot: u32) -> Region {
        std::mem::replace(&mut self.slab[slot as usize], Region::new())
    }

    /// Drop a slab cell's storage in place, for the slot's recycling. A region the cell never
    /// wrote into owns nothing and stays as it is; only one that claimed a chunk is replaced.
    pub(crate) fn clear_slab(&mut self, slot: u32) {
        let region = &mut self.slab[slot as usize];
        if !region.is_untouched() {
            *region = Region::new();
        }
    }

    /// Take a tree cell's storage off it, leaving an empty region behind — for the splice or the
    /// drop that disposal performs.
    pub(crate) fn take_tree(&mut self, index: u32) -> Region {
        std::mem::replace(&mut self.tree[index as usize], Region::new())
    }

    /// Splice a departing cell's bump into a slab cell's bundle.
    pub(crate) fn splice_into_slab(&mut self, slot: u32, from: Region) {
        self.slab[slot as usize].absorb(from);
    }

    /// Splice a departing cell's bump into a tree cell's bundle.
    pub(crate) fn splice_into_tree(&mut self, index: u32, from: Region) {
        self.tree[index as usize].absorb(from);
    }
}

/// The write surface into a region's bytes, at the brand the door that hands one out chose: a
/// build closure's own, or the executing cell's `'cell`.
///
/// `Copy` with a private field, so a writer exists only where the graph hands one out, and every
/// verb returns a shared `&'r` rather than the `&mut` the bump itself yields: a written value is
/// region state its holder names, never one it owns.
///
/// A verb per shape a region cannot be given by an embedder, in two pairs. Known width, where the
/// count is settled before the first element: [`fill`](Writer::fill) lays down a run by index
/// under a compile-time no-destructor check, and [`text`](Writer::text) writes a `str`, whose
/// bytes have no `T` to index by. Producer-decided width, where only the elements settle it:
/// [`run`](Writer::run) takes pushes and [`prose`](Writer::prose) takes formatted writes, each
/// handing back the region borrow once the producer is done. Every simpler shape — one value, a
/// copied slice, a run collected from an iterator of known length — is the embedder's, derived
/// from these.
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

    /// A run whose length the producer decides — a filtered or mapped list, a split — pushed one
    /// element at a time and closed with [`Run::finish`].
    ///
    /// The buffer grows in place while it is the region's newest allocation: a growth allocates
    /// the delta and slides the bytes down inside the chunk. An allocation interleaved between
    /// pushes ends that, and the next growth copies to a fresh buffer and strands the outgrown one
    /// as dead region bytes — the same cost a growing scratch transient pays. A run whose elements
    /// are themselves written into this region as the loop goes interleaves by construction; build
    /// those in the embedder's own scratch first and lay the run down with `fill`.
    ///
    /// Where the length is known before the first element, [`fill`](Self::fill) costs no header
    /// and no growth path.
    pub fn run<T>(self) -> Run<'r, T> {
        const { assert!(!std::mem::needs_drop::<T>()) };
        Run(allocator_api2::vec::Vec::new_in(self.0))
    }

    /// Text whose length the producer decides: a [`fmt::Write`](std::fmt::Write) sink into the
    /// region, closed with [`Prose::finish`]. Same growth story as [`run`](Self::run), which it is
    /// a run of bytes over.
    pub fn prose(self) -> Prose<'r> {
        Prose(self.run())
    }
}

/// A run under construction in a region, at the brand of the writer that opened it.
///
/// No `Drop`: a run abandoned mid-build is dead region bytes, like every other buffer a bump
/// outgrows. Nothing it holds runs a destructor either — [`Writer::run`] asserts that at the
/// instantiation site, the way [`Writer::fill`] does.
pub struct Run<'r, T>(allocator_api2::vec::Vec<T, &'r Bump>);

impl<'r, T> Run<'r, T> {
    /// Append one element.
    pub fn push(&mut self, value: T) {
        self.0.push(value);
    }

    /// Append every element of `values`.
    pub fn extend(&mut self, values: impl IntoIterator<Item = T>) {
        self.0.extend(values);
    }

    /// How many elements are down so far.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether nothing has been pushed yet.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Give the slack back and hand out the borrow of the run that lives in the region.
    ///
    /// The trim reclaims into the chunk only while the buffer is still the region's newest
    /// allocation; otherwise it is a no-op and the slack stays dead region bytes.
    pub fn finish(mut self) -> &'r [T] {
        self.0.shrink_to_fit();
        self.0.leak()
    }
}

/// Text under construction in a region: a [`fmt::Write`](std::fmt::Write) sink over a
/// [`Run`] of bytes, so `write!` lands its output straight in the region's chunk.
pub struct Prose<'r>(Run<'r, u8>);

impl std::fmt::Write for Prose<'_> {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        self.0.extend(text.bytes());
        Ok(())
    }
}

impl<'r> Prose<'r> {
    /// How many bytes — not characters — are down so far.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether nothing has been written yet.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Give the slack back and hand out the borrow of the text that lives in the region.
    ///
    /// The bytes only ever arrived from a `&str`, so the check passes by construction; it is a
    /// linear pass the crate pays rather than take an `unsafe` it has no other need for.
    pub fn finish(self) -> &'r str {
        std::str::from_utf8(self.0.finish()).expect("every byte came from a str")
    }
}
