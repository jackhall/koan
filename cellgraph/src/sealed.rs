//! The second tier: regions whose cell died while something still reached their storage. A sealed
//! region has no slab slot, no matrix row, and no generation — only a frozen aggregate, a holder
//! count, and the chunks its cell detached. See
//! [design/liveness-matrix.md](../design/liveness-matrix.md) § The sealed tier.
//!
//! Ids come from a monotone space and are never reused, which is what lets the tier skip
//! generations entirely: a sealed name cannot be re-bound, so it cannot go stale. Sealedness is
//! enforced by what this module cannot express — a record has no write path into its aggregate
//! beyond the seal transition's own rewrite, so a pin *out of* a sealed region is unrepresentable.

use std::cell::OnceCell;
use std::collections::HashMap;

use crate::handle::Handle;
use crate::mask::Mask;
use crate::region::Region;

/// The name of one sealed region. Drawn in creation order from a space that never wraps and never
/// reuses, so an id names the same region for the whole life of the table.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) struct SealedId(u64);

/// A sparse set of sealed ids, kept sorted so union is a merge and membership a binary search.
///
/// Sealed sets are the sparse half of every hold set and every reach mask. They stay small because
/// only a *retained* region takes an id, so a sorted vector beats a hash set on both the union
/// that reach composition performs and the iteration the cascade performs.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub(crate) struct SealedSet {
    ids: Vec<SealedId>,
}

impl SealedSet {
    pub(crate) fn new() -> Self {
        SealedSet { ids: Vec::new() }
    }

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
    pub(crate) fn union_with(&mut self, other: &SealedSet) {
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

    pub(crate) fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub(crate) fn len(&self) -> usize {
        self.ids.len()
    }
}

/// A frozen closure: every record a hold on one record keeps alive. Recorded only for a closure
/// that names no live cell, and exact from then on.
///
/// The node *set* is what is memoized, not a byte total: two branches of one closure may share a
/// sub-tier, so a price folds sets together and sums once at the end. Summing memoized totals
/// instead would bill the shared part twice.
pub(crate) struct Memo {
    /// The records the closure spans, root included, in walk order.
    pub(crate) records: Vec<SealedId>,
}

/// One retained region: everything its cell had, minus everything a cell needs to run.
pub(crate) struct SealedRecord {
    /// The cell's hold set, frozen at its death instead of cleared. Monotone holds make this
    /// exactly the union of every reach ever minted into the region, so the freeze is a word copy
    /// and consults no storage.
    pub(crate) aggregate: Mask,
    /// The chunks, detached from the slot unmoved. `None` for a cell that never allocated.
    pub(crate) storage: Option<Region>,
    /// How many hold sets name this region — live cells' sealed halves plus other records'
    /// aggregates. Decremented only in batch, when a holder dies or reclaims.
    pub(crate) holders: u32,
    /// The most holders this record has ever had. Test-only, and the tell a wound-down run reads:
    /// a region no more than one hold set ever named is one the merges reach, so a survivor of a
    /// full wind-down must have been shared at some point.
    #[cfg(test)]
    pub(crate) peak_holders: u32,
    /// What a hold on this record keeps alive, once that answer can no longer change. Written by a
    /// price query and by nothing else, and never cleared — there is no path that clears a
    /// `OnceCell`, which is the point.
    ///
    /// A closure is memoized only when it names no live cell, and **nothing inside such a closure
    /// ever changes**:
    ///
    /// - Every node of it is a record, and each is named by its predecessor's aggregate, so each
    ///   has a holder for as long as the root does: none retires.
    /// - A record's aggregate and storage change only in a fold, whose target is either the record
    ///   a seal just minted or a namer whose aggregate names the dying slot. Neither is in a frozen
    ///   closure: one is new, the other names a slab bit.
    /// - A record is absorbed only when its sole holder is such a target. A record inside a frozen
    ///   closure is held by a record inside it, so it is never a source either.
    /// - The seal transition's conversions rewrite live rows and aggregates naming the dying slot,
    ///   none of them frozen, and a mint writes a live cell's hold set.
    ///
    /// So the node set is fixed and the bytes are fixed, and the memo stays exact for the record's
    /// whole life ([liveness-matrix.md § Bounding the two
    /// tiers](../design/liveness-matrix.md#bounding-the-two-tiers)).
    pub(crate) closure: OnceCell<Memo>,
    /// The departed cells whose residents this record now answers for: every handle the table's
    /// relocation map points at this id. Bounded by merges, never by values — a cell contributes
    /// at most one entry, however many residents it kept — and it is what lets the record's
    /// retirement drop exactly its own entries from that map.
    pub(crate) lineage: Vec<Handle>,
}

impl SealedRecord {
    /// Bytes the detached chunks still occupy. Retention lives only in this tier, so this is the
    /// occupancy a hold on the region is answerable for — what the consolidation copy buys back,
    /// and the input a pressure model prices a release against.
    pub(crate) fn retained_bytes(&self) -> usize {
        self.storage
            .as_ref()
            .map_or(0, |storage| storage.allocated_bytes())
    }
}

/// Every sealed region in the table, and the monotone counter their ids come from.
pub(crate) struct SealedTier {
    records: HashMap<SealedId, SealedRecord>,
    next: u64,
    /// Retained bytes summed over every record present — the tier's half of the occupancy signal,
    /// maintained at the three places storage enters or leaves the tier rather than scanned.
    bytes: usize,
}

impl SealedTier {
    pub(crate) fn new() -> Self {
        SealedTier {
            records: HashMap::new(),
            next: 0,
            bytes: 0,
        }
    }

    /// Take the next id. Never reused, so nothing needs a generation to tell two occupants apart.
    pub(crate) fn mint_id(&mut self) -> SealedId {
        let id = SealedId(self.next);
        self.next += 1;
        id
    }

    pub(crate) fn insert(&mut self, id: SealedId, record: SealedRecord) {
        self.bytes += record.retained_bytes();
        self.records.insert(id, record);
    }

    pub(crate) fn get(&self, id: SealedId) -> Option<&SealedRecord> {
        self.records.get(&id)
    }

    pub(crate) fn get_mut(&mut self, id: SealedId) -> Option<&mut SealedRecord> {
        self.records.get_mut(&id)
    }

    pub(crate) fn remove(&mut self, id: SealedId) -> Option<SealedRecord> {
        let record = self.records.remove(&id)?;
        self.bytes -= record.retained_bytes();
        Some(record)
    }

    /// Splice storage into a record, keeping the running total in step. The one write into a
    /// record's storage after its construction, so the total needs no other maintenance point.
    pub(crate) fn splice_storage(&mut self, id: SealedId, from: Option<Region>) {
        self.bytes += from.as_ref().map_or(0, Region::allocated_bytes);
        let record = self
            .records
            .get_mut(&id)
            .expect("the fold target is present");
        Region::splice(&mut record.storage, from);
    }

    /// Bytes retained across the whole tier.
    pub(crate) fn retained_bytes(&self) -> usize {
        self.bytes
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn ids(&self) -> impl Iterator<Item = SealedId> + '_ {
        self.records.keys().copied()
    }

    pub(crate) fn len(&self) -> usize {
        self.records.len()
    }
}
