//! The second tier: regions whose cell died while something still reached their storage. A sealed
//! region has no slab slot, no matrix row, and no generation — only a frozen aggregate, a holder
//! count, and the chunks its cell detached. See
//! [graph/README.md](graph/README.md) § The sealed tier.
//!
//! Ids come from a monotone space and are never reused, which is what lets the tier skip
//! generations entirely: a sealed name cannot be re-bound, so it cannot go stale. The *slot* an id
//! names in the tier's slab is reused; the serial packed beside it is what tells a live id from a
//! retired one whose index came back. Sealedness is enforced by what this module cannot express — a
//! sealed cell has no write path into its aggregate beyond the seal transition's own rewrite, so a
//! pin *out of* a sealed region is unrepresentable.

use std::cell::Cell;

use smallvec::SmallVec;

use crate::handle::SlabHandle;
use crate::reach::GraphReach;
use crate::region::Region;
use crate::scratch::ScratchVec;

#[cfg(test)]
mod tests;

/// The name of one sealed region: a graph-wide monotone `serial` in the high half, the tier's own
/// slab `index` in the low half. Drawn in creation order and never reused, so an id names the same
/// region for the whole life of the graph — and because the serial leads, the derived ordering is
/// creation order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) struct SealedId(u64);

impl SealedId {
    fn pack(serial: u32, index: u32) -> Self {
        SealedId(u64::from(serial) << 32 | u64::from(index))
    }

    /// Where in the tier's slab this id's sealed cell sits, live or retired.
    fn index(self) -> u32 {
        self.0 as u32
    }

    /// An id with a chosen serial and index, for the tests outside this module that need concrete
    /// ones. Minting is the tier's job everywhere else.
    #[cfg(test)]
    pub(crate) fn packed(serial: u32, index: u32) -> Self {
        SealedId::pack(serial, index)
    }

    /// Which mint handed this id out. What separates a live sealed cell from a retired one that
    /// gave its index back.
    fn serial(self) -> u32 {
        (self.0 >> 32) as u32
    }
}

/// Where a sorted id set keeps its ids: inline, spilling to the heap, for the sets the graph
/// stores durably; the scratch region for the seen set a walk builds and throws away.
///
/// The two buffers differ in nothing the sorted-insert logic reads, so the set is generic over
/// them rather than written twice.
pub(crate) trait IdBuffer: std::ops::Deref<Target = [SealedId]> {
    fn insert(&mut self, at: usize, id: SealedId);
    fn remove(&mut self, at: usize) -> SealedId;
}

impl IdBuffer for SmallVec<[SealedId; 2]> {
    fn insert(&mut self, at: usize, id: SealedId) {
        SmallVec::insert(self, at, id);
    }

    fn remove(&mut self, at: usize) -> SealedId {
        SmallVec::remove(self, at)
    }
}

impl IdBuffer for ScratchVec<'_, SealedId> {
    fn insert(&mut self, at: usize, id: SealedId) {
        ScratchVec::insert(self, at, id);
    }

    fn remove(&mut self, at: usize) -> SealedId {
        ScratchVec::remove(self, at)
    }
}

/// A sparse set of sealed ids, kept sorted so union is a merge and membership a binary search.
///
/// Sealed sets are the sparse half of every hold set and every reach mask. They stay small because
/// only a *retained* region takes an id, so a sorted run beats a hash set on both the union that
/// reach composition performs and the iteration the cascade performs — and small enough that the
/// durable buffer keeps two ids inline and reaches the allocator only past that.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub(crate) struct IdSet<V> {
    ids: V,
}

/// A durable id set: a hold set, a reverse-naming entry, a mask's sparse half. Two ids inline, so
/// the sets that dominate — a reach naming one region, a hold set naming a couple — cost no
/// allocation and no indirection to read.
pub(crate) type SealedSet = IdSet<SmallVec<[SealedId; 2]>>;

/// A transient id set, living in the graph's scratch region for the length of one verb.
pub(crate) type ScratchSet<'scratch> = IdSet<ScratchVec<'scratch, SealedId>>;

impl SealedSet {
    pub(crate) const fn new() -> Self {
        IdSet {
            ids: SmallVec::new_const(),
        }
    }
}

impl<'scratch> ScratchSet<'scratch> {
    /// An empty set over a scratch buffer.
    pub(crate) fn over(ids: ScratchVec<'scratch, SealedId>) -> Self {
        debug_assert!(ids.is_empty(), "a set is built over an empty buffer");
        IdSet { ids }
    }

    /// `other`'s ids copied into a scratch buffer.
    ///
    /// A copy of the whole run, not an insert per id: the source is ascending and distinct
    /// already, so the sorted insert would re-derive at `k log k` what a copy settles at `k`.
    /// This is how a pricing walk seeds its seen set from the destination's hold set, which is
    /// the largest set it ever starts from.
    pub(crate) fn copy_of(
        mut ids: ScratchVec<'scratch, SealedId>,
        other: &IdSet<impl IdBuffer>,
    ) -> Self {
        debug_assert!(ids.is_empty(), "a set is built over an empty buffer");
        ids.extend_from_slice(other.as_slice());
        IdSet { ids }
    }
}

impl<V: IdBuffer> IdSet<V> {
    /// Add an id, reporting whether it was absent. The answer is what the holder count reads: a
    /// hold set names a region at most once, so a second mint of the same id is not a second hold.
    pub(crate) fn insert(&mut self, id: SealedId) -> bool {
        match self.ids.binary_search(&id) {
            Ok(_) => false,
            Err(at) => {
                self.ids.insert(at, id);
                true
            }
        }
    }

    pub(crate) fn remove(&mut self, id: SealedId) -> bool {
        match self.ids.binary_search(&id) {
            Ok(at) => {
                self.ids.remove(at);
                true
            }
            Err(_) => false,
        }
    }

    /// Fold another set in. The sparse half of reach composition, and idempotent for the same
    /// reason the word `OR` is: naming a region twice is naming it once.
    ///
    /// Insertion, not a merge down the two sets. A merge is the cheaper shape when the sides are
    /// comparable, but the fold reach composition performs is skewed — a handful of ids into a set
    /// that mostly names them already — and at that shape a search per id beats a walk down
    /// everything both sides name.
    pub(crate) fn union_with(&mut self, other: &IdSet<impl IdBuffer>) {
        for id in other.iter() {
            self.insert(id);
        }
    }

    /// Whether this set names the sealed region `id`.
    pub(crate) fn contains(&self, id: SealedId) -> bool {
        self.ids.binary_search(&id).is_ok()
    }

    /// The ids, in id order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = SealedId> + '_ {
        self.ids.iter().copied()
    }

    /// The ids as a slice — what a caller that only walks them takes, so the two buffers reach
    /// one signature.
    pub(crate) fn as_slice(&self) -> &[SealedId] {
        &self.ids
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub(crate) fn len(&self) -> usize {
        self.ids.len()
    }
}

/// One retained region: everything its cell had, minus everything a cell needs to run.
pub(crate) struct SealedCell<const W: usize> {
    /// The cell's hold set, frozen at its death instead of cleared. Monotone holds make this
    /// exactly the union of every reach ever minted into the region, so the freeze is a word copy
    /// and consults no storage.
    pub(crate) aggregate: GraphReach<W>,
    /// The chunks, detached from the slot unmoved — a bundle with no chunk at all for a cell that
    /// never allocated, and the sealed cell's own bytes from then on: its memo is written here too.
    pub(crate) storage: Region,
    /// How many hold sets name this region — live cells' sealed halves plus other sealed cells'
    /// aggregates. Decremented only in batch, when a holder dies or reclaims.
    pub(crate) holders: u32,
    /// The most holders this sealed cell has ever had. Test-only, and the tell a wound-down run
    /// reads: a region no more than one hold set ever named is one the merges reach, so a survivor
    /// of a full wind-down must have been shared at some point.
    #[cfg(test)]
    pub(crate) peak_holders: u32,
    /// The head of the chain of departed cells whose dormant carriers this sealed cell now answers
    /// for, threaded through the graph's relocation entries themselves. Bounded by merges, never by
    /// values — a cell contributes at most one entry, however many dormant carriers it kept — and
    /// it is what lets the sealed cell's retirement drop exactly its own entries from that map.
    pub(crate) lineage: Option<SlabHandle>,
}

impl<const W: usize> SealedCell<W> {
    /// Bytes the detached chunks still occupy, the memo's own among them. Retention lives only in
    /// this tier, so this is the occupancy a hold on the region is answerable for — what the
    /// consolidation copy buys back, and the input a pressure model prices a release against.
    pub(crate) fn retained_bytes(&self) -> usize {
        self.storage.allocated_bytes()
    }

    /// What a hold on this sealed cell keeps alive, once that answer can no longer change — the
    /// sealed-cell set, written into the sealed cell's own [`storage`](Self::storage). Written by a
    /// price query and by nothing else, and never cleared: there is no path that clears a
    /// `OnceCell`, which is the point.
    ///
    /// The node *set* is what is memoized, not a byte total: two branches of one closure may share
    /// a sub-tier, so a price folds sets together and sums once at the end. Summing memoized
    /// totals instead would bill the shared part twice.
    ///
    /// A closure is memoized only when it names no live cell, and **nothing inside such a closure
    /// ever changes**:
    ///
    /// - Every node of it is a sealed cell, and each is named by its predecessor's aggregate, so
    ///   each has a holder for as long as the root does: none retires.
    /// - A sealed cell's aggregate and storage change only in a fold, whose target is either the
    ///   sealed cell a seal just minted or a namer whose aggregate names the dying slot. Neither is
    ///   in a frozen closure: one is new, the other names a slab bit.
    /// - A sealed cell is absorbed only when its sole holder is such a target. A sealed cell inside
    ///   a frozen closure is held by a sealed cell inside it, so it is never a source either.
    /// - The seal transition's conversions rewrite live rows and aggregates naming the dying slot,
    ///   none of them frozen, and a mint writes a live cell's hold set.
    ///
    /// So the node set is fixed and the bytes are fixed, and the memo stays exact for the sealed
    /// cell's whole life ([graph/README.md § Bounding the two
    /// tiers](graph/README.md#bounding-the-two-tiers)).
    pub(crate) fn memo(&self) -> Option<&[SealedId]> {
        self.storage.memo()
    }
}

/// Every sealed region in the graph: a dense slab of sealed-cell slots, the indices retirement
/// handed back, and the monotone serial their ids come from.
///
/// No hashing anywhere. An id carries its own index, so every lookup is a bounds-checked load and a
/// serial compare, and the serial is what makes a retired id read as absent rather than as whatever
/// sealed cell later took its index.
pub(crate) struct SealedTier<const W: usize> {
    /// One entry per index the tier has ever handed out. `None` while the index is on the free
    /// list; the serial beside a present sealed cell is what tells a live id from a retired one
    /// that reused its index.
    cells: Vec<Option<(u32, SealedCell<W>)>>,
    free: Vec<u32>,
    next_serial: u32,
    live: usize,
    /// Retained bytes summed over every sealed cell present — the tier's half of the occupancy
    /// signal, maintained at the places storage enters or leaves the tier rather than scanned. A
    /// `Cell` because priming a memo grows a sealed cell's bytes and runs under `&self`.
    bytes: Cell<usize>,
}

impl<const W: usize> SealedTier<W> {
    /// A tier pre-sized to the slab's `cap`: a graph cannot have more sealed cells than it has had
    /// cells, up to what retention keeps beyond that. Construction is unmetered, like the slab
    /// itself, so the reserve costs no verb an allocation.
    pub(crate) fn new(cap: u32) -> Self {
        SealedTier {
            cells: Vec::with_capacity(cap as usize),
            free: Vec::with_capacity(cap as usize),
            next_serial: 0,
            live: 0,
            bytes: Cell::new(0),
        }
    }

    /// Take the next id: a retired index if one is free, else a fresh one, under a serial no
    /// earlier mint has used. The serial is what makes an id unreusable even though its index is
    /// not.
    ///
    /// # Panics
    ///
    /// If the serial space is exhausted. A `u32` of them outlives any run that seals at a sane
    /// rate, and reusing one would let a retired id name a live sealed cell.
    pub(crate) fn mint_id(&mut self) -> SealedId {
        let index = match self.free.pop() {
            Some(index) => index,
            None => {
                self.cells.push(None);
                (self.cells.len() - 1) as u32
            }
        };
        let serial = self.next_serial;
        self.next_serial = serial
            .checked_add(1)
            .expect("the sealed tier's serial space is exhausted");
        SealedId::pack(serial, index)
    }

    pub(crate) fn insert(&mut self, id: SealedId, sealed_cell: SealedCell<W>) {
        let slot = &mut self.cells[id.index() as usize];
        debug_assert!(slot.is_none(), "a minted index is filled once");
        self.bytes
            .set(self.bytes.get() + sealed_cell.retained_bytes());
        *slot = Some((id.serial(), sealed_cell));
        self.live += 1;
    }

    pub(crate) fn get(&self, id: SealedId) -> Option<&SealedCell<W>> {
        match self.cells.get(id.index() as usize)? {
            Some((serial, sealed_cell)) if *serial == id.serial() => Some(sealed_cell),
            _ => None,
        }
    }

    pub(crate) fn get_mut(&mut self, id: SealedId) -> Option<&mut SealedCell<W>> {
        match self.cells.get_mut(id.index() as usize)? {
            Some((serial, sealed_cell)) if *serial == id.serial() => Some(sealed_cell),
            _ => None,
        }
    }

    pub(crate) fn remove(&mut self, id: SealedId) -> Option<SealedCell<W>> {
        let slot = self.cells.get_mut(id.index() as usize)?;
        match slot {
            Some((serial, _)) if *serial == id.serial() => {}
            _ => return None,
        }
        let (_, sealed_cell) = slot
            .take()
            .expect("the serial matched a present sealed cell");
        self.free.push(id.index());
        self.live -= 1;
        self.bytes
            .set(self.bytes.get() - sealed_cell.retained_bytes());
        Some(sealed_cell)
    }

    /// Splice storage into a sealed cell, keeping the running total in step. The one write into a
    /// sealed cell's storage after its construction, so the total needs no other maintenance point.
    pub(crate) fn splice_storage(&mut self, id: SealedId, from: Region) {
        self.bytes.set(self.bytes.get() + from.allocated_bytes());
        let sealed_cell = self.get_mut(id).expect("the fold target is present");
        sealed_cell.storage.absorb(from);
    }

    /// Write `ids` into `id`'s own region as its frozen closure, once, and count the bytes that
    /// cost. Under `&self` because a price query is a read of the graph everywhere else.
    pub(crate) fn prime(&self, id: SealedId, ids: &[SealedId]) {
        let Some(sealed_cell) = self.get(id) else {
            return;
        };
        let written = sealed_cell.storage.set_memo(ids);
        self.bytes.set(self.bytes.get() + written);
    }

    /// Bytes retained across the whole tier.
    pub(crate) fn retained_bytes(&self) -> usize {
        self.bytes.get()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.live == 0
    }

    #[cfg(test)]
    pub(crate) fn ids(&self) -> impl Iterator<Item = SealedId> + '_ {
        self.cells.iter().enumerate().filter_map(|(index, slot)| {
            slot.as_ref()
                .map(|(serial, _)| SealedId::pack(*serial, index as u32))
        })
    }

    pub(crate) fn len(&self) -> usize {
        self.live
    }
}
