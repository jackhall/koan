//! The **frozen table**: a [`DigestTable`](super::DigestTable)'s read-only copy. It holds the same
//! 16 B slots in the same places and probes them the same way, but its run is a plain slice laid
//! once and never written, so it holds no cell and is `Sync` wherever its entries are. The type
//! lattice's frozen load indexes its nodes and its verdicts with two.

use super::Keyed;
use super::digest_table::{Slot, probe};

/// A read-only table of references to [`Keyed`] entries, laid by
/// [`DigestTable::frozen`](super::DigestTable::frozen).
pub struct FrozenTable<'r, N> {
    run: &'r [Slot<'r, N>],
    len: usize,
}

impl<'r, N: Keyed> FrozenTable<'r, N> {
    /// A table over `run`, which holds `len` entries at most half its slots.
    pub(super) fn over(run: &'r [Slot<'r, N>], len: usize) -> Self {
        FrozenTable { run, len }
    }

    /// The entry whose key is `key`, if the table holds one.
    pub fn get(&self, key: N::Key) -> Option<&'r N> {
        probe(self.run.len(), |index| self.run[index], key)
    }

    /// How many entries the table holds.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the table holds no entry.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Visit every entry, in slot order.
    pub fn each(&self, mut visit: impl FnMut(&'r N)) {
        for slot in self.run {
            if let Some(entry) = slot.entry() {
                visit(entry);
            }
        }
    }
}
