//! Bit storage for the two cell relations, and the single row the executing flag occupies. See
//! [design/liveness-matrix.md](../design/liveness-matrix.md) § Two relations, two structures.
//!
//! Both relations take the same square shape, indexed the same way: row `holder`, bit `held`, so a
//! cell's whole hold set is one contiguous row. That is the axis each relation's *write* wants —
//! the birth derivation ORs a parent's row into its child's, and the pin mint ORs a reach mask
//! into a destination's — and it makes the reclaim query, "does anything still hold this cell",
//! the scan across rows that [`Matrix::held_by_any`] performs.

use crate::mask::{Mask, words_for};

/// A `cap` x `cap` bit matrix over slab slots, stored as a flat word slice, one contiguous row per
/// slot. A set bit at `(holder, held)` means the cell in `holder` keeps the cell in `held` alive.
pub(crate) struct Matrix {
    words_per_row: usize,
    bits: Box<[u64]>,
}

impl Matrix {
    pub(crate) fn new(cap: u32) -> Self {
        let words_per_row = words_for(cap);
        Matrix {
            words_per_row,
            bits: vec![0u64; words_per_row * cap as usize].into_boxed_slice(),
        }
    }

    fn place(&self, holder: u32, held: u32) -> (usize, u64) {
        let index = holder as usize * self.words_per_row + (held as usize) / 64;
        (index, 1u64 << (held % 64))
    }

    pub(crate) fn set(&mut self, holder: u32, held: u32) {
        let (index, bit) = self.place(holder, held);
        self.bits[index] |= bit;
    }

    pub(crate) fn clear(&mut self, holder: u32, held: u32) {
        let (index, bit) = self.place(holder, held);
        self.bits[index] &= !bit;
    }

    pub(crate) fn test(&self, holder: u32, held: u32) -> bool {
        let (index, bit) = self.place(holder, held);
        self.bits[index] & bit != 0
    }

    /// A cell's whole hold set as words — the copy-out the seal transition freezes into an
    /// aggregate. Contiguity is why the freeze consults no storage and scans no values.
    pub(crate) fn row_words(&self, holder: u32) -> &[u64] {
        let start = holder as usize * self.words_per_row;
        &self.bits[start..start + self.words_per_row]
    }

    /// Fold a reach mask into `holder`'s hold set, minus `holder`'s own bit — the mint OR, and the
    /// only write into the pin relation ([liveness-matrix.md § Reach as a hybrid
    /// mask](../design/liveness-matrix.md#reach-as-a-hybrid-mask)). The and-not is the self rule: a
    /// cell that held itself alive would never reach a zero hold count.
    pub(crate) fn mint(&mut self, holder: u32, reach: &Mask) {
        let start = holder as usize * self.words_per_row;
        for (offset, word) in reach.words().iter().enumerate() {
            self.bits[start + offset] |= *word;
        }
        let (index, bit) = self.place(holder, holder);
        self.bits[index] &= !bit;
    }

    /// Whether any row in `rows` holds `held`. The reclaim query: a cell is reclaimable only when
    /// no live cell names it in either relation.
    pub(crate) fn held_by_any(&self, rows: impl Iterator<Item = u32>, held: u32) -> bool {
        rows.into_iter().any(|holder| self.test(holder, held))
    }

    /// The cells `holder` names, in slot order — the hold graph's outgoing edges from one cell.
    #[cfg(debug_assertions)]
    pub(crate) fn held_by(&self, holder: u32, cap: u32) -> impl Iterator<Item = u32> + '_ {
        (0..cap).filter(move |held| self.test(holder, *held))
    }

    /// OR `source`'s row into `dest`'s. The birth relation's only compound write: a new cell's row
    /// starts as its parent's, which is what makes transitive closure hold by construction.
    pub(crate) fn inherit_row(&mut self, dest: u32, source: u32) {
        let dest = dest as usize * self.words_per_row;
        let source = source as usize * self.words_per_row;
        for offset in 0..self.words_per_row {
            self.bits[dest + offset] |= self.bits[source + offset];
        }
    }

    /// Drop every name in `row`. The whole-row release a cell's death performs.
    pub(crate) fn clear_row(&mut self, holder: u32) {
        let start = holder as usize * self.words_per_row;
        self.bits[start..start + self.words_per_row].fill(0);
    }

    /// Whether `outer`'s row names everything `inner`'s row names — the containment invariant a
    /// birth chain satisfies from parent to child.
    #[cfg(test)]
    pub(crate) fn row_contains(&self, outer: u32, inner: u32) -> bool {
        let outer = outer as usize * self.words_per_row;
        let inner = inner as usize * self.words_per_row;
        (0..self.words_per_row).all(|offset| {
            let inner_word = self.bits[inner + offset];
            self.bits[outer + offset] & inner_word == inner_word
        })
    }
}

/// One row of the same width as a [`Matrix`] row, indexed by slab slot. The executing flag.
pub(crate) struct BitRow {
    words: Box<[u64]>,
}

impl BitRow {
    pub(crate) fn new(cap: u32) -> Self {
        BitRow {
            words: vec![0u64; words_for(cap)].into_boxed_slice(),
        }
    }

    pub(crate) fn set(&mut self, bit: u32) {
        self.words[bit as usize / 64] |= 1u64 << (bit % 64);
    }

    pub(crate) fn clear(&mut self, bit: u32) {
        self.words[bit as usize / 64] &= !(1u64 << (bit % 64));
    }

    pub(crate) fn test(&self, bit: u32) -> bool {
        self.words[bit as usize / 64] & (1u64 << (bit % 64)) != 0
    }
}
