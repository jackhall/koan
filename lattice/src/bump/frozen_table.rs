//! The **frozen table**: a [`DigestTable`](super::DigestTable)'s read-only twin. It holds the same
//! 16 B slots, placed and probed the same way, but its run is laid once in a bump, filled while it
//! is still exclusively held, and never written after, so it holds no cell and is `Sync` wherever
//! its entries are. The type lattice's frozen load indexes its nodes and its verdicts with two. A
//! generated image writes one's slots as a `static` run ([`imaged`](FrozenTable::imaged)).

use super::digest_table::{Slot, home, probe, slots_for};
use super::{BumpAllocator, Keyed};

/// A read-only table of references to [`Keyed`] entries, laid once by
/// [`placed`](Self::placed).
pub struct FrozenTable<'r, N> {
    run: &'r [Slot<'r, N>],
    len: usize,
}

impl<'r, N> FrozenTable<'r, N> {
    /// The empty table, on the smallest run.
    pub const EMPTY: Self = FrozenTable {
        run: &[Slot::EMPTY; 4],
        len: 0,
    };

    /// The table over `run`, holding `len` entries, as a generated image writes one: the emitter
    /// copied the run a [`placed`](Self::placed) table laid.
    #[doc(hidden)]
    pub const fn imaged(run: &'r [Slot<'r, N>], len: usize) -> Self {
        FrozenTable { run, len }
    }

    /// The run of slots, in order, for the emitter that writes them.
    #[cfg(any(test, feature = "generator"))]
    pub(crate) fn slots(&self) -> &'r [Slot<'r, N>] {
        self.run
    }
}

impl<'r, N: Keyed> FrozenTable<'r, N> {
    /// A table over `bump` holding the `len` entries `entries` yields, whose keys are distinct:
    /// one run laid at a load of at most one half, each entry placed as a digest table places it.
    pub fn placed(
        bump: BumpAllocator<'r>,
        len: usize,
        entries: impl IntoIterator<Item = &'r N>,
    ) -> Self {
        let run: &'r mut [Slot<'r, N>] =
            bump.alloc_slice_fill_with(slots_for(len), |_| Slot::EMPTY);
        let mut placed = 0;
        for entry in entries {
            let slot = Slot::of(entry.key(), entry);
            run[home(run.len(), |index| run[index], slot.word())] = slot;
            placed += 1;
        }
        assert_eq!(
            placed, len,
            "a frozen table is laid for the entries it holds"
        );
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
