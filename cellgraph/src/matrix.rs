//! Bit storage for the two cell relations, and the single row the executing flag occupies. See
//! [design/liveness-matrix.md](../design/liveness-matrix.md) § Two relations, two structures.
//!
//! Both relations take the same square shape, indexed the same way: row `holder`, bit `held`, so a
//! cell's whole hold set is one contiguous row. That is the axis each relation's *write* wants —
//! the birth derivation ORs a parent's row into its child's, and the pin mint ORs a reach mask
//! into a destination's. It is the wrong axis for the reclaim query, "does anything still hold
//! this cell", which reads down a column instead; a matrix answers that from a tally it keeps as
//! it writes, so the query costs one read rather than a scan across every row.
//!
//! One type, [`Bits`], is every row of bits in the crate: a matrix row, the executing row, and the
//! slab half of a reach mask. It is `W` words held inline, so it is `Copy` and building, copying,
//! or comparing one touches no allocator, and [`Bits::place`] is the only word-and-bit arithmetic
//! written outside the two loops that keep a matrix's tally in step with its rows.

#[cfg(test)]
mod tests;

use crate::mask::Mask;

/// A row of bits indexed by slab slot: `W` words held inline, naming `64 · W` slots.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Bits<const W: usize> {
    words: [u64; W],
}

/// The set bits of `words`, in bit order. Each word is consumed lowest set bit first, by the
/// clear-lowest-bit identity `word &= word - 1`.
fn ones_of(words: &[u64]) -> impl Iterator<Item = u32> + '_ {
    words.iter().enumerate().flat_map(|(index, word)| {
        let base = (index * u64::BITS as usize) as u32;
        let mut rest = *word;
        std::iter::from_fn(move || {
            (rest != 0).then(|| {
                let bit = base + rest.trailing_zeros();
                rest &= rest - 1;
                bit
            })
        })
    })
}

impl<const W: usize> Bits<W> {
    /// How many slots a row names — the slab width of every table built at this word count.
    pub(crate) const CELLS: u32 = (W * u64::BITS as usize) as u32;

    /// A clear row.
    pub(crate) const fn new() -> Self {
        Bits { words: [0; W] }
    }

    /// The word index and the one-bit mask for `bit`. Every other bit operation goes through here,
    /// which is what gives each of them the width assertion.
    fn place(bit: u32) -> (usize, u64) {
        debug_assert!(
            bit < Self::CELLS,
            "bit {bit} is outside a {}-bit row",
            Self::CELLS
        );
        (bit as usize / 64, 1u64 << (bit % 64))
    }

    pub(crate) fn test(&self, bit: u32) -> bool {
        let (index, mask) = Self::place(bit);
        self.words[index] & mask != 0
    }

    /// The bits this row sets, in bit order.
    pub(crate) fn ones(&self) -> impl Iterator<Item = u32> + '_ {
        ones_of(&self.words)
    }

    /// Whether this row sets everything `other` sets — the containment invariant a birth chain
    /// satisfies from parent to child.
    #[cfg(test)]
    pub(crate) fn contains_all(&self, other: &Bits<W>) -> bool {
        self.words
            .iter()
            .zip(&other.words)
            .all(|(outer, inner)| outer & inner == *inner)
    }

    /// Set one bit, reporting whether it was clear before.
    pub(crate) fn set(&mut self, bit: u32) -> bool {
        let (index, mask) = Self::place(bit);
        let word = &mut self.words[index];
        let was = *word & mask == 0;
        *word |= mask;
        was
    }

    /// Clear one bit, reporting whether it was set.
    pub(crate) fn clear(&mut self, bit: u32) -> bool {
        let (index, mask) = Self::place(bit);
        let word = &mut self.words[index];
        let was = *word & mask != 0;
        *word &= !mask;
        was
    }

    /// OR in every bit `source` sets that `exclude` does not — the frontier step of a walk over
    /// the slab half of the hold graph, where `exclude` is what the walk has already seen. One
    /// word-wise pass in place of a bit-at-a-time expansion of the row into a worklist.
    pub(crate) fn union_not_with(&mut self, source: &Bits<W>, exclude: &Bits<W>) {
        for (word, (source, exclude)) in self
            .words
            .iter_mut()
            .zip(source.words.iter().zip(&exclude.words))
        {
            *word |= *source & !*exclude;
        }
    }

    /// Take the lowest bit this row sets, clearing it — a walk popping its frontier. `None` when
    /// the row is clear.
    pub(crate) fn take_one(&mut self) -> Option<u32> {
        let (index, word) = self
            .words
            .iter_mut()
            .enumerate()
            .find(|(_, word)| **word != 0)?;
        let bit = word.trailing_zeros();
        *word &= *word - 1;
        Some((index * u64::BITS as usize) as u32 + bit)
    }

    /// OR another row in.
    pub(crate) fn union_with(&mut self, other: &Bits<W>) {
        for (word, source) in self.words.iter_mut().zip(&other.words) {
            *word |= *source;
        }
    }
}

/// A `64·W` x `64·W` bit matrix over slab slots, held inline. A set bit at `(holder, held)` means
/// the cell in `holder` keeps the cell in `held` alive.
///
/// Written as `W` chunks of 64 rows only because `W · 64` cannot be an array length on stable: the
/// block is contiguous and row `r` sits at word offset `r · W`, exactly where a flat array would
/// put it, so the chunking is index arithmetic ([`Matrix::at`]) and nothing else.
pub(crate) struct Matrix<const W: usize> {
    rows: [[Bits<W>; 64]; W],
    /// How many rows name each slot, one entry per column, chunked the same way. The reclaim query
    /// asks only whether a cell is named at all, so carrying the count turns that question from a
    /// scan across every row into a single read.
    holders: [[u32; 64]; W],
}

impl<const W: usize> Matrix<W> {
    pub(crate) fn new() -> Self {
        Matrix {
            rows: [[Bits::new(); 64]; W],
            holders: [[0; 64]; W],
        }
    }

    /// Which chunk a slot's row and tally sit in, and where within it.
    fn at(slot: u32) -> (usize, usize) {
        (slot as usize / 64, slot as usize % 64)
    }

    /// One cell's whole hold set. Contiguity is why the seal transition's freeze consults no
    /// storage and scans no values: the row copies out whole.
    pub(crate) fn row(&self, holder: u32) -> &Bits<W> {
        let (chunk, row) = Self::at(holder);
        &self.rows[chunk][row]
    }

    pub(crate) fn row_mut(&mut self, holder: u32) -> &mut Bits<W> {
        let (chunk, row) = Self::at(holder);
        &mut self.rows[chunk][row]
    }

    fn tally(&mut self, held: u32) -> &mut u32 {
        let (chunk, row) = Self::at(held);
        &mut self.holders[chunk][row]
    }

    pub(crate) fn set(&mut self, holder: u32, held: u32) {
        if self.row_mut(holder).set(held) {
            *self.tally(held) += 1;
        }
    }

    pub(crate) fn clear(&mut self, holder: u32, held: u32) {
        if self.row_mut(holder).clear(held) {
            *self.tally(held) -= 1;
        }
    }

    /// How many rows name `held`.
    pub(crate) fn holders(&self, held: u32) -> u32 {
        let (chunk, row) = Self::at(held);
        self.holders[chunk][row]
    }

    pub(crate) fn test(&self, holder: u32, held: u32) -> bool {
        self.row(holder).test(held)
    }

    /// Fold a reach mask into `holder`'s hold set, minus `holder`'s own bit — the mint OR, and the
    /// only write into the pin relation ([liveness-matrix.md § Reach as a hybrid
    /// mask](../design/liveness-matrix.md#reach-as-a-hybrid-mask)). The and-not is the self rule: a
    /// cell that held itself alive would never reach a zero hold count.
    pub(crate) fn mint(&mut self, holder: u32, reach: &Mask<W>) {
        self.union_into(holder, *reach.slab());
        self.clear(holder, holder);
    }

    /// Whether any row in `rows` holds `held` — the scan [`Matrix::holders`] stands in for, kept
    /// as the check the reclaim query asserts itself against.
    #[cfg(test)]
    pub(crate) fn held_by_any(&self, rows: impl Iterator<Item = u32>, held: u32) -> bool {
        rows.into_iter().any(|holder| self.test(holder, held))
    }

    /// The cells `holder` names, in slot order — the hold graph's outgoing edges from one cell,
    /// one at a time, which is what the test-only ring walk wants and the pricing walk does not.
    #[cfg(test)]
    pub(crate) fn held_by(&self, holder: u32) -> impl Iterator<Item = u32> + '_ {
        self.row(holder).ones()
    }

    /// OR `source`'s row into `dest`'s. The birth relation's only compound write: a new cell's row
    /// starts as its parent's, which is what makes transitive closure hold by construction.
    pub(crate) fn inherit_row(&mut self, dest: u32, source: u32) {
        debug_assert_ne!(dest, source, "a row does not inherit from itself");
        self.union_into(dest, *self.row(source));
    }

    /// OR a row in, counting a hold for every bit the union newly sets — the shared body of the
    /// mint and the birth derivation. Every write that can set a bit passes through here or
    /// through [`Matrix::set`], which is what lets [`Matrix::holders`] answer from a tally rather
    /// than a scan across rows.
    ///
    /// The source arrives by value, which is `W` words, so the two rows are never borrowed from
    /// the matrix at once.
    fn union_into(&mut self, dest: u32, source: Bits<W>) {
        let Matrix { rows, holders } = self;
        let (chunk, row) = Self::at(dest);
        for (index, (word, source)) in rows[chunk][row]
            .words
            .iter_mut()
            .zip(&source.words)
            .enumerate()
        {
            let base = (index * u64::BITS as usize) as u32;
            let mut newly = *source & !*word;
            *word |= *source;
            while newly != 0 {
                let (chunk, row) = Self::at(base + newly.trailing_zeros());
                holders[chunk][row] += 1;
                newly &= newly - 1;
            }
        }
    }

    /// Drop every name in `holder`'s row. The whole-row release a cell's death performs.
    pub(crate) fn clear_row(&mut self, holder: u32) {
        let Matrix { rows, holders } = self;
        let (chunk, row) = Self::at(holder);
        for (index, word) in rows[chunk][row].words.iter_mut().enumerate() {
            let base = (index * u64::BITS as usize) as u32;
            let mut rest = *word;
            while rest != 0 {
                let (chunk, row) = Self::at(base + rest.trailing_zeros());
                holders[chunk][row] -= 1;
                rest &= rest - 1;
            }
            *word = 0;
        }
    }

    /// Whether `outer`'s row names everything `inner`'s row names — the containment invariant a
    /// birth chain satisfies from parent to child.
    #[cfg(test)]
    pub(crate) fn row_contains(&self, outer: u32, inner: u32) -> bool {
        self.row(outer).contains_all(self.row(inner))
    }
}
