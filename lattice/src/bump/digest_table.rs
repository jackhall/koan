//! The **digest table**: an insert-only hash table of references to entries that each carry their
//! own key, whose low word is uniformly distributed — a type's 128-bit digest, or a verdict's
//! fold of two. The key is already a hash, so the table runs no hash function, and the entry
//! holds it, so the table stores no copy of it.
//!
//! A slot is 16 B: the key's low word beside the reference. The word's masked low bits are the
//! slot's home, and a probe walks linearly from there to the first empty slot, comparing words
//! and reading an entry only when its word matches. The load stays at most one half, so every
//! probe ends at an empty slot. The smallest run is four slots, one 64 B line.
//!
//! The table grows by laying a run twice the length and re-placing every occupied slot by its
//! stored word, so growth reads no entry; the old run is stranded. Laying is the one capability
//! it needs, [`LaysRuns`], which an insert takes as an argument: a region's writer lays fixed runs
//! and has no allocator, so a table that grows only by laying runs is one a cell's region can
//! host. The bump tier implements [`LaysRuns`] for a bump (`bump.rs`).
//!
//! The type lattice's root keeps a hashbrown table and a branch keeps this one; see
//! [identity.md § Storage](../types/identity.md#storage-a-root-and-its-branches).

use std::cell::Cell;

/// What lays a fixed run of drop-free values and hands back its borrow for `'r`: the one
/// capability a [`DigestTable`] grows by, and the one a type branch lays its nodes through. A bump
/// serves it here; a region's writer serves it in koan.
pub trait LaysRuns<'r>: Copy {
    /// Lay a run of `len` values, each built from its index.
    fn lay<T>(self, len: usize, fill: impl FnMut(usize) -> T) -> &'r [T];

    /// Lay a copy of `text`.
    fn text(self, text: &str) -> &'r str;
}

/// An entry a [`DigestTable`] holds by reference: it carries its own key, whose low
/// [`word`](Self::word) is uniformly distributed, so the table hashes by that word and stores no
/// copy of the key.
pub trait Keyed {
    /// What the entry is found by.
    type Key: Copy + Eq;

    /// The entry's key.
    fn key(&self) -> Self::Key;

    /// The key's uniformly distributed low word: the slot's home and its stored word.
    fn word(key: Self::Key) -> u64;
}

/// One slot: the key's low word beside the entry. Empty when `entry` is `None`; the word of an
/// empty slot is never read.
struct Slot<'r, N> {
    word: u64,
    entry: Option<&'r N>,
}

// Manual impls: `#[derive(Clone, Copy)]` would demand `N: Copy`.
impl<N> Clone for Slot<'_, N> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<N> Copy for Slot<'_, N> {}

impl<N> Slot<'_, N> {
    const EMPTY: Self = Slot {
        word: 0,
        entry: None,
    };
}

/// The smallest run: one 64 B line of slots.
const SMALLEST: usize = 4;

/// An insert-only table of references to [`Keyed`] entries, laid and grown through [`LaysRuns`].
/// Its run holds at most half as many entries as slots.
pub struct DigestTable<'r, N> {
    run: Cell<&'r [Cell<Slot<'r, N>>]>,
    len: Cell<usize>,
}

const _: () = assert!(std::mem::size_of::<Cell<Slot<'static, u8>>>() == 16);
const _: () = assert!(std::mem::size_of::<DigestTable<'static, u8>>() == 24);

impl<'r, N: Keyed> DigestTable<'r, N> {
    /// An empty table on the smallest run.
    pub fn new(lay: impl LaysRuns<'r>) -> Self {
        Self::on_run(lay, SMALLEST)
    }

    /// An empty table on a run that holds `entries` inserts without growing.
    pub fn with_capacity(lay: impl LaysRuns<'r>, entries: usize) -> Self {
        Self::on_run(lay, (entries * 2).next_power_of_two().max(SMALLEST))
    }

    fn on_run(lay: impl LaysRuns<'r>, slots: usize) -> Self {
        DigestTable {
            run: Cell::new(lay.lay(slots, |_| Cell::new(Slot::EMPTY))),
            len: Cell::new(0),
        }
    }

    /// The entry whose key is `key`, if the table holds one.
    pub fn get(&self, key: N::Key) -> Option<&'r N> {
        let run = self.run.get();
        let mask = run.len() - 1;
        let word = N::word(key);
        let mut index = word as usize & mask;
        // Ends: the load is at most one half, so an empty slot always lies ahead.
        loop {
            let slot = run[index].get();
            let entry = slot.entry?;
            if slot.word == word && entry.key() == key {
                return Some(entry);
            }
            index = (index + 1) & mask;
        }
    }

    /// Insert `entry`, whose key the caller guarantees is absent, growing through `lay` first if
    /// one more entry would pass half the run.
    pub fn insert_unique(&self, lay: impl LaysRuns<'r>, entry: &'r N) {
        let key = entry.key();
        debug_assert!(self.get(key).is_none(), "an inserted key is absent");
        if (self.len.get() + 1) * 2 > self.run.get().len() {
            self.grow(lay);
        }
        place(
            self.run.get(),
            Slot {
                word: N::word(key),
                entry: Some(entry),
            },
        );
        self.len.set(self.len.get() + 1);
    }

    /// How many entries the table holds.
    pub fn len(&self) -> usize {
        self.len.get()
    }

    /// Whether the table holds no entry.
    pub fn is_empty(&self) -> bool {
        self.len.get() == 0
    }

    /// The current run's length in slots.
    pub fn slots(&self) -> usize {
        self.run.get().len()
    }

    /// Visit every entry, in slot order.
    pub fn each(&self, mut visit: impl FnMut(&'r N)) {
        for slot in self.run.get() {
            if let Some(entry) = slot.get().entry {
                visit(entry);
            }
        }
    }

    /// Lay a run twice the length and re-place every occupied slot by its stored word.
    fn grow(&self, lay: impl LaysRuns<'r>) {
        let old = self.run.get();
        let new = lay.lay(old.len() * 2, |_| Cell::new(Slot::EMPTY));
        for slot in old {
            let slot = slot.get();
            if slot.entry.is_some() {
                place(new, slot);
            }
        }
        self.run.set(new);
    }
}

/// Set `slot` in the first empty slot from its word's home, probing linearly.
fn place<'r, N>(run: &[Cell<Slot<'r, N>>], slot: Slot<'r, N>) {
    let mask = run.len() - 1;
    let mut index = slot.word as usize & mask;
    while run[index].get().entry.is_some() {
        index = (index + 1) & mask;
    }
    run[index].set(slot);
}

#[cfg(test)]
mod tests;
