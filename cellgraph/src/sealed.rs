//! The second tier: regions whose cell died while something still reached their storage. A sealed
//! region has no slab slot, no matrix row, and no generation — only a frozen aggregate, a holder
//! count, and the chunks its cell detached. See
//! [design/liveness-matrix.md](../design/liveness-matrix.md) § The sealed tier.
//!
//! Ids come from a monotone space and are never reused, which is what lets the tier skip
//! generations entirely: a sealed name cannot be re-bound, so it cannot go stale. Sealedness is
//! enforced by what this module cannot express — a record has no write path into its aggregate
//! beyond the seal transition's own rewrite, so a pin *out of* a sealed region is unrepresentable.

use std::collections::HashMap;

use crate::mask::Mask;
use crate::region::Region;

/// The name of one sealed region. Drawn in creation order from a space that never wraps and never
/// reuses, so an id names the same region for the whole life of the table.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct SealedId(u64);

/// A sparse set of sealed ids, kept sorted so union is a merge and membership a binary search.
///
/// Sealed sets are the sparse half of every hold set and every reach mask. They stay small because
/// only a *retained* region takes an id, so a sorted vector beats a hash set on both the union
/// that reach composition performs and the iteration the cascade performs.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct SealedSet {
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
    pub fn contains(&self, id: SealedId) -> bool {
        self.ids.binary_search(&id).is_ok()
    }

    /// The ids, in id order.
    pub fn iter(&self) -> impl Iterator<Item = SealedId> + '_ {
        self.ids.iter().copied()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }
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
}

impl SealedTier {
    pub(crate) fn new() -> Self {
        SealedTier {
            records: HashMap::new(),
            next: 0,
        }
    }

    /// Take the next id. Never reused, so nothing needs a generation to tell two occupants apart.
    pub(crate) fn mint_id(&mut self) -> SealedId {
        let id = SealedId(self.next);
        self.next += 1;
        id
    }

    pub(crate) fn insert(&mut self, id: SealedId, record: SealedRecord) {
        self.records.insert(id, record);
    }

    pub(crate) fn get(&self, id: SealedId) -> Option<&SealedRecord> {
        self.records.get(&id)
    }

    pub(crate) fn get_mut(&mut self, id: SealedId) -> Option<&mut SealedRecord> {
        self.records.get_mut(&id)
    }

    pub(crate) fn remove(&mut self, id: SealedId) -> Option<SealedRecord> {
        self.records.remove(&id)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn ids(&self) -> impl Iterator<Item = SealedId> + '_ {
        self.records.keys().copied()
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.records.len()
    }
}
