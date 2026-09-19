//! [`Region`] — the **cell region**: one cell's bump, the only place a value with reach may rest,
//! in pointer-stable chunks that a sealing cell's storage detaches with unmoved. See
//! [../README.md](../README.md) § The cell.
//!
//! The bump is lifetime-free, so a region borrow `'cell` enters only at the allocating call. That is
//! what lets a value written here hold an `&'cell` back into the very region it lives in with no
//! residence check: a lifetime-*typed* slot would have to name a lifetime a region has no
//! parameter for.
//!
//! Nothing stored in a region is ever dropped — a bump releases its chunks whole — which is why
//! every family a region hosts is [`DropFree`](crate::DropFree). The write surface is
//! [`Writer`], a `Copy` handle a step receives at a brand it cannot widen: a build closure's own
//! for a foreign destination, and the executing cell's `'here` for its own region.
//!
//! Every live cell's region sits in one table, [`Regions`], that a step borrows **shared** for
//! its whole length while it holds the rest of the graph exclusively. The step's own writer is a
//! plain `&'here` into that table, and every placement's writer a shorter borrow of it: two
//! writers may name one bump — a placement into the executing cell is exactly that, the build's
//! writer beside the step's own — and a bump's bytes are interior-mutable throughout, so shared
//! borrows of it tolerate each other's reads and writes. Every verb that moves or drops a region
//! takes the table exclusively, which no step can do, so no `&mut` over a live region's bytes can
//! exist under a writer into it: the borrow checker holds that line.
//!
//! Beside each region the table keeps the cell's **scratch bump** — the half of its habitat a
//! step writes at its `'scratch` brand and the table hands back whole once nothing names it — and,
//! for the whole graph, a bounded **spare list** of reset bumps: a reclaimed region's chunks wait
//! there for the next birth, so a release-then-create loop stays off the allocator. Both are reset
//! only through the table's `&mut` doors, outside any step.
//!
//! A region is a **bundle** of bumps: the one it writes into, plus the bumps of every region
//! absorbed into it. Absorption is how a merge splices storage
//! ([graph/README.md § Locality tactics](graph/README.md#locality-tactics)) —
//! a `Bump` moves without moving a chunk byte, so the pointer stability a detached seal already
//! relies on carries a borrow across the merge unchanged.

use std::alloc::Layout;
use std::cell::OnceCell;
use std::collections::VecDeque;
use std::marker::PhantomData;
use std::ptr::NonNull;

use bumpalo::Bump;

use crate::carrier::CellHome;
use crate::graph::Config;
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
        // region's own bump and reachable through no other path. The bump is reset or freed only
        // once this `Region` has been taken apart, which drops the memo first, and moving the
        // `Bump` moves no chunk byte, so the run stays where it was written for as long as the
        // region lives. `SealedId: Copy`, so
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
    ///
    /// Hands back **the bump that did not join the bundle**, where there is one: this region's own
    /// when the source took it over whole, or the source's when nothing was written into it. Either
    /// holds no value, and a warm one still owns a chunk, so the caller retires it rather than
    /// letting it drop.
    #[must_use = "a bump left out of the bundle may own a chunk, which its caller retires"]
    pub(crate) fn absorb(&mut self, mut other: Region) -> Option<Bump> {
        // An unwritten bundle takes the source over whole rather than listing it, which is what
        // keeps a merge into a cell that never wrote off the allocator. Nothing of this region's
        // is lost: a bump that handed out no byte holds no value, and a memo would have cost
        // bytes.
        if self.is_unwritten() && self.absorbed.is_empty() {
            let displaced = std::mem::replace(self, other);
            return Some(displaced.bump);
        }
        if self.absorbed.len() < other.absorbed.len() {
            std::mem::swap(&mut self.absorbed, &mut other.absorbed);
        }
        self.absorbed.append(&mut other.absorbed);
        self.absorbed_bytes += other.absorbed_bytes;
        // A bump nothing was written into keeps no borrow alive, so taking it in would only
        // lengthen the list and bill the bundle for a chunk no value sits in.
        if other.is_unwritten() {
            return Some(other.bump);
        }
        self.absorbed_bytes += other.bump.allocated_bytes();
        self.absorbed.push(other.bump);
        None
    }

    /// Bytes the chunks occupy, whether or not a value still uses them — a bump never reclaims
    /// within a chunk, so this is what the region costs while anything holds it. Absorbed bumps
    /// count: the bundle is answerable for every chunk it took in. O(1), off the running total the
    /// merges maintain, since a priced operand reads this figure and a bundle grows by splices.
    ///
    /// A bump drawn off the spare list counts its chunk from the cell's birth, written or not: the
    /// region occupies it.
    pub(crate) fn allocated_bytes(&self) -> usize {
        self.bump.allocated_bytes() + self.absorbed_bytes
    }

    /// Whether the region's own bump has handed out no byte: a cold bump, which owns no chunk, or a
    /// warm one whose chunk is whole. "Did this cell write?" is this reading and never
    /// `allocated_bytes() == 0`, which a warm bump fails with nothing written.
    pub(crate) fn is_unwritten(&self) -> bool {
        self.bump.allocated_bytes() == self.bump.chunk_capacity()
    }

    /// Whether the region is as it was built: no chunk claimed, nothing absorbed, no memo. Such a
    /// region owns nothing, so a slot that recycles with one can keep it rather than build another.
    fn is_untouched(&self) -> bool {
        self.bump.allocated_bytes() == 0 && self.absorbed.is_empty() && self.memo.get().is_none()
    }
}

/// What one table index holds for the cell that occupies it: both halves of its habitat, the
/// region and the scratch bump beside it.
///
/// One index reaches the two together, and they stay separate structs because a [`Region`] is what
/// seals, splices and absorbs and a scratch bump is none of that — it is pinned to its index and
/// never travels, so an absorb leaves the absorber's scratch alone and a seal retains no scratch,
/// by construction.
struct Habitat {
    region: Region,
    /// The second bump a step writes, at its `'scratch` brand, reset at a step's end once nothing
    /// names it. Its bytes are in no price: a hold never extends a scratch byte's life.
    scratch: Bump,
}

impl Habitat {
    fn new() -> Self {
        Habitat {
            region: Region::new(),
            scratch: Bump::new(),
        }
    }
}

/// Every live cell's habitat, slab and tree, in one table the graph keeps beside its cells.
///
/// Held apart from the cells so a step can borrow the two differently: the table shared, at the
/// step's `'here` brand, and the cells exclusively. Every slot carries a region from the start —
/// an empty bump claims no chunk, so a cell that never writes costs the table one small struct
/// and nothing else — and a region leaves the table only through the `&mut` doors below, which
/// the graph verbs that dispose of a cell take outside any step. A writer at `'here` therefore
/// names a bump nothing can move or drop for as long as it is out, by the borrow checker's word;
/// and since no `&mut` into the table exists inside a step, every write a step makes into a bump
/// descends from a shared borrow, which a bump's interior-mutable bytes tolerate.
pub(crate) struct Regions {
    /// One habitat per slab slot, its region replaced with an empty one when the slot recycles.
    slab: Box<[Habitat]>,
    /// One habitat per tree-pool index, every index up to the table's capacity filled. The table
    /// starts a slab's width long, like the pool itself, and doubles from there in step with it;
    /// a cell's creation finds its region already waiting, and pays a length check for it.
    tree: Vec<Habitat>,
    /// Reset bumps a reclaim gave up, each still owning its chunk, for the next birth to draw:
    /// last in, first out, so a cell born right after a death writes into the chunk that death
    /// freed, and a push past [`bound`](Self::bound) evicts from the other end, the oldest first. Always empty under Miri, where a reclaimed
    /// chunk goes back to the allocator so that a use after the reclaim is an error it can see.
    spare: VecDeque<Bump>,
    /// Cells that own a region and have not disposed — what the spare list serves. Sealed storage
    /// and absorbed bumps are retention rather than demand, and are not counted.
    live: u32,
    /// A moving average of `live` in 24.8 fixed point, sampled at every birth and every disposal
    /// and nowhere else: no clock and no float.
    average: u32,
    /// How many spares the list may hold per averaged live cell.
    spare_proportion: u32,
    /// The average's window, as a shift: each sample closes `1 / 2^shift` of the gap to `live`.
    spare_window_shift: u32,
}

impl Regions {
    pub(crate) fn new(config: Config) -> Self {
        Regions {
            slab: (0..config.cap).map(|_| Habitat::new()).collect(),
            tree: (0..config.cap).map(|_| Habitat::new()).collect(),
            spare: VecDeque::with_capacity(config.cap as usize),
            live: 0,
            average: 0,
            spare_proportion: config.spare_proportion,
            spare_window_shift: config.spare_window_shift,
        }
    }

    /// One index's habitat.
    fn habitat(&self, home: CellHome) -> &Habitat {
        match home {
            CellHome::Slab(slot) => &self.slab[slot as usize],
            CellHome::Tree(index) => &self.tree[index as usize],
        }
    }

    fn habitat_mut(&mut self, home: CellHome) -> &mut Habitat {
        match home {
            CellHome::Slab(slot) => &mut self.slab[slot as usize],
            CellHome::Tree(index) => &mut self.tree[index as usize],
        }
    }

    /// The most bumps the spare list may hold right now: the proportion of the averaged live
    /// count, rounded up — so a loop holding one live cell keeps one spare. Enforced where a bump
    /// is pushed and nowhere else: a list an earlier peak left long sheds its whole excess at the
    /// next retire.
    fn bound(&self) -> usize {
        ((u64::from(self.spare_proportion) * u64::from(self.average) + 255) >> 8) as usize
    }

    /// Fold the live count into the moving average.
    fn sample(&mut self) {
        let gap = (i64::from(self.live) << 8) - i64::from(self.average);
        self.average = (i64::from(self.average) + (gap >> self.spare_window_shift)) as u32;
    }

    /// A region-owning cell was born: count it, and warm its bump off the spare list if one is
    /// there. A slot's region is cold at every birth — disposal left it so — which is why the draw
    /// is a plain replacement. The spare comes off the list before the habitat is indexed, so the
    /// two borrows never overlap.
    pub(crate) fn warm(&mut self, home: CellHome) {
        self.live += 1;
        self.sample();
        let spare = self.spare.pop_back();
        let region = &mut self.habitat_mut(home).region;
        debug_assert!(region.is_untouched(), "a cell is born into a region in use");
        if let Some(bump) = spare {
            region.bump = bump;
        }
    }

    /// A region-owning cell disposed, by whichever exit.
    pub(crate) fn departed(&mut self) {
        debug_assert!(self.live > 0, "a cell departed that no birth counted");
        self.live -= 1;
        self.sample();
    }

    /// Give up a region nothing reaches: every bump of its bundle is retired, its own last, so the
    /// next birth draws the chunk the departing cell wrote into most recently. The memo goes with
    /// the region, before any of its bytes can be handed out again.
    pub(crate) fn retire(&mut self, region: Region) {
        let Region { bump, absorbed, .. } = region;
        for absorbed in absorbed {
            self.retire_bump(absorbed);
        }
        self.retire_bump(bump);
    }

    /// Reset one bump onto the spare list, or drop it: a cold one owns no chunk to keep, and under
    /// Miri every one goes back to the allocator. A full list makes room by evicting its oldest
    /// spares, the coldest chunks, so a bump retired now always waits unless the bound is zero.
    ///
    /// The caller owns the bump, so nothing borrows its bytes: the safety argument is the one a
    /// reclaim's drop already rests on, and a reset is that drop with the largest chunk kept.
    pub(crate) fn retire_bump(&mut self, mut bump: Bump) {
        if bump.allocated_bytes() == 0 {
            return;
        }
        let bound = self.bound();
        if cfg!(miri) || bound == 0 {
            return;
        }
        while self.spare.len() >= bound {
            self.spare.pop_front();
        }
        bump.reset();
        self.spare.push_back(bump);
    }

    /// How many bumps the spare list holds.
    #[cfg(test)]
    pub(crate) fn spare_len(&self) -> usize {
        self.spare.len()
    }

    /// What [`bound`](Self::bound) reads right now.
    #[cfg(test)]
    pub(crate) fn spare_bound(&self) -> usize {
        self.bound()
    }

    /// The region a step homed in `home` writes.
    pub(crate) fn region(&self, home: CellHome) -> &Region {
        &self.habitat(home).region
    }

    fn scratch_bump(&self, home: CellHome) -> &Bump {
        &self.habitat(home).scratch
    }

    /// The write surface of `home`'s scratch bump, which a step takes at its `'scratch` brand.
    pub(crate) fn scratch_writer(&self, home: CellHome) -> Writer<'_> {
        Writer(self.scratch_bump(home))
    }

    /// Hand `home`'s scratch bump back whole. The caller has established that nothing at rest names
    /// a byte of it, and that no step is running.
    ///
    /// Under Miri the bump is rebuilt rather than reset, so its chunk goes back to the allocator
    /// and a stale `'scratch` reference is an error Miri can see.
    pub(crate) fn reset_scratch(&mut self, home: CellHome) {
        let bump = &mut self.habitat_mut(home).scratch;
        if bump.allocated_bytes() > 0 {
            if cfg!(miri) {
                *bump = Bump::new();
            } else {
                bump.reset();
            }
        }
    }

    /// Bytes `home`'s scratch bump has handed out since its last reset.
    #[cfg(test)]
    pub(crate) fn scratch_in_use(&self, home: CellHome) -> usize {
        let bump = self.scratch_bump(home);
        bump.allocated_bytes() - bump.chunk_capacity()
    }

    /// Chunk bytes a slab cell's region bundle occupies, `0` where it never allocated.
    pub(crate) fn slab_bytes(&self, slot: u32) -> usize {
        self.slab[slot as usize].region.allocated_bytes()
    }

    /// Chunk bytes a tree cell's region bundle occupies, `0` where it never allocated.
    pub(crate) fn tree_bytes(&self, index: u32) -> usize {
        self.tree[index as usize].region.allocated_bytes()
    }

    /// Make room for a tree-pool index the pool has just minted. Idempotent for one it already
    /// covers.
    pub(crate) fn reach_tree(&mut self, index: u32) {
        let index = index as usize;
        if self.tree.len() <= index {
            let needed = index + 1 - self.tree.len();
            self.tree.reserve(needed.max(self.tree.len()));
            let room = self.tree.capacity();
            self.tree.resize_with(room, Habitat::new);
        }
    }

    /// Take a slab cell's storage off it, leaving an empty region behind — for the seal, the
    /// merge or the drop that disposal performs. The scratch bump stays where it is: a seal
    /// retains no scratch.
    pub(crate) fn take_slab(&mut self, slot: u32) -> Region {
        std::mem::replace(&mut self.slab[slot as usize].region, Region::new())
    }

    /// Retire a slab cell's storage, for the slot's recycling. A region that owns nothing stays
    /// as it is; one that claimed or drew a chunk is replaced, and its bumps retired.
    pub(crate) fn clear_slab(&mut self, slot: u32) {
        if !self.slab[slot as usize].region.is_untouched() {
            let region = self.take_slab(slot);
            self.retire(region);
        }
    }

    /// Take a tree cell's storage off it, leaving an empty region behind — for the splice or the
    /// drop that disposal performs.
    pub(crate) fn take_tree(&mut self, index: u32) -> Region {
        std::mem::replace(&mut self.tree[index as usize].region, Region::new())
    }

    /// Splice a departing cell's bump into a slab cell's bundle.
    pub(crate) fn splice_into_slab(&mut self, slot: u32, from: Region) {
        if let Some(bump) = self.slab[slot as usize].region.absorb(from) {
            self.retire_bump(bump);
        }
    }

    /// Splice a departing cell's bump into a tree cell's bundle.
    pub(crate) fn splice_into_tree(&mut self, index: u32, from: Region) {
        if let Some(bump) = self.tree[index as usize].region.absorb(from) {
            self.retire_bump(bump);
        }
    }
}

/// The write surface into a region's bytes, at the brand the door that hands one out chose: a
/// build closure's own, or the executing cell's `'here`.
///
/// `Copy` with a private field, so a writer exists only where the graph hands one out, and every
/// verb returns a shared `&'cell` rather than the `&mut` the bump itself yields: a written value is
/// region state its holder names, never one it owns.
///
/// A verb per shape a region cannot be given by an embedder. Known width, where the count is
/// settled before the first element: [`fill`](Writer::fill) lays down a run by index under a
/// compile-time no-destructor check, [`thin_run`](Writer::thin_run) lays one down behind its
/// length for a handle that must be one pointer wide, and [`text`](Writer::text) writes a `str`,
/// whose bytes have no `T` to index by. Producer-decided width, where only the elements settle it:
/// [`run`](Writer::run) takes pushes and [`prose`](Writer::prose) takes formatted writes, each
/// handing back the region borrow once the producer is done. Every simpler shape — one value, a
/// copied slice, a run collected from an iterator of known length — is the embedder's, derived
/// from these.
#[derive(Clone, Copy)]
pub struct Writer<'cell>(&'cell Bump);

impl<'cell> Writer<'cell> {
    /// Write a run of `len` values, each built from its index, and hand back the borrow of it that
    /// lives in the region.
    ///
    /// A bump releases its chunks whole and never walks a value, so `T` must run no destructor.
    /// The check is a `const` assert on `T` rather than a `T: Copy` bound: a bound would also
    /// refuse the interior mutability a cell-resident table needs (`Cell<u32>` is drop-free but
    /// not `Copy`), and the assert fires at the instantiation site either way.
    pub fn fill<T>(self, len: usize, fill: impl FnMut(usize) -> T) -> &'cell [T] {
        const { assert!(!std::mem::needs_drop::<T>()) };
        self.0.alloc_slice_fill_with(len, fill)
    }

    /// Write a run of `len` values behind a header holding `len`, in one allocation, and hand back
    /// the handle one pointer wide that reaches both — for a shape whose handle cannot afford a
    /// slice's second word. Same drop-glue assert as [`fill`](Self::fill).
    ///
    /// The allocation is claimed before the first element is built, and a bump's chunks never
    /// move, so `fill` may itself write into this region — an element's own sub-run through this
    /// writer — without disturbing the run it is filling.
    pub fn thin_run<T>(self, len: usize, mut fill: impl FnMut(usize) -> T) -> ThinRun<'cell, T> {
        const { assert!(!std::mem::needs_drop::<T>()) };
        let base: NonNull<u8> = self.0.alloc_layout(ThinRun::<T>::layout(len));
        let head = base.cast::<ThinHeader>();
        // SAFETY: `base` is a fresh allocation of `layout(len)` bytes, aligned to the stricter of
        // the header's and `T`'s alignment; the header sits at offset 0 and the `len` elements
        // from `OFFSET`, inside the allocation by construction of the layout. Each element is
        // written once, through a pointer derived from `base` rather than from a reference, so its
        // provenance spans the whole allocation. A `fill` that panics leaves initialised bytes
        // nothing reads and no destructor runs on (`T` is drop-free by the assert), like a run
        // `fill` abandons; the header is written first, but no handle escapes to read it.
        unsafe {
            head.write(ThinHeader { len });
            let run = base.add(ThinRun::<T>::OFFSET).cast::<T>();
            for index in 0..len {
                run.add(index).write(fill(index));
            }
        }
        ThinRun {
            head,
            _run: PhantomData,
        }
    }

    /// Write text.
    pub fn text(self, text: &str) -> &'cell str {
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
    pub fn run<T>(self) -> Run<'cell, T> {
        const { assert!(!std::mem::needs_drop::<T>()) };
        Run(allocator_api2::vec::Vec::new_in(self.0))
    }

    /// Text whose length the producer decides: a [`fmt::Write`](std::fmt::Write) sink into the
    /// region, closed with [`Prose::finish`]. Same growth story as [`run`](Self::run), which it is
    /// a run of bytes over.
    pub fn prose(self) -> Prose<'cell> {
        Prose(self.run())
    }
}

/// What sits at the front of a [`ThinRun`]'s allocation: its length. The elements follow at
/// `ThinRun::<T>::OFFSET`.
#[repr(C)]
struct ThinHeader {
    len: usize,
}

/// A run reached through one thin pointer: a length header with the elements laid down right after
/// it at their own alignment, in one allocation a [`Writer::thin_run`] made.
///
/// `Copy` for any `T`, and read at `'cell` like every other written run.
pub struct ThinRun<'cell, T> {
    head: NonNull<ThinHeader>,
    _run: PhantomData<&'cell [T]>,
}

impl<T> Clone for ThinRun<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for ThinRun<'_, T> {}

impl<'cell, T> ThinRun<'cell, T> {
    /// The element run's byte offset from the header: a constant per `T`, whatever the length.
    const OFFSET: usize = size_of::<ThinHeader>().next_multiple_of(align_of::<T>());

    /// The allocation for `len` elements. Never zero-sized, since the header is always there.
    fn layout(len: usize) -> Layout {
        let elements = size_of::<T>()
            .checked_mul(len)
            .and_then(|bytes| bytes.checked_add(Self::OFFSET))
            .expect("a thin run's size fits usize");
        Layout::from_size_align(elements, align_of::<ThinHeader>().max(align_of::<T>()))
            .expect("a thin run's size fits isize")
    }

    /// How many elements the run holds.
    pub fn len(self) -> usize {
        // SAFETY: `head` came from `Writer::thin_run`, which wrote the header before handing the
        // handle out. A bump's chunks are reset or freed only by a verb that holds the region table
        // exclusively, which no `'cell` borrow coexists with, and moving a bump moves no chunk
        // byte.
        unsafe { self.head.as_ptr().read() }.len
    }

    /// Whether the run holds no element.
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    /// The elements, borrowed from the region.
    pub fn as_slice(self) -> &'cell [T] {
        // SAFETY: the element pointer is derived from `head`, the allocation's own pointer, so its
        // provenance covers the run; `thin_run` wrote all `len` elements from `OFFSET` before the
        // handle existed, aligned by the layout. The chunk lives for `'cell`, as in `len`. At
        // length zero or for a zero-sized `T` the pointer is still non-null and aligned.
        unsafe {
            let run = self.head.cast::<u8>().add(Self::OFFSET).cast::<T>();
            std::slice::from_raw_parts(run.as_ptr(), self.len())
        }
    }
}

/// A run under construction in a region, at the brand of the writer that opened it.
///
/// No `Drop`: a run abandoned mid-build is dead region bytes, like every other buffer a bump
/// outgrows. Nothing it holds runs a destructor either — [`Writer::run`] asserts that at the
/// instantiation site, the way [`Writer::fill`] does.
pub struct Run<'cell, T>(allocator_api2::vec::Vec<T, &'cell Bump>);

impl<'cell, T> Run<'cell, T> {
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
    pub fn finish(mut self) -> &'cell [T] {
        self.0.shrink_to_fit();
        self.0.leak()
    }
}

/// Text under construction in a region: a [`fmt::Write`](std::fmt::Write) sink over a
/// [`Run`] of bytes, so `write!` lands its output straight in the region's chunk.
pub struct Prose<'cell>(Run<'cell, u8>);

impl std::fmt::Write for Prose<'_> {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        self.0.extend(text.bytes());
        Ok(())
    }
}

impl<'cell> Prose<'cell> {
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
    pub fn finish(self) -> &'cell str {
        std::str::from_utf8(self.0.finish()).expect("every byte came from a str")
    }
}
