//! Bit storage for the cell relations: a square matrix of rows over slab slots, and a single row
//! of the same width. The birth relation is a matrix row per cell; the executing flag is one row
//! across all cells. See [design/liveness-matrix.md](../design/liveness-matrix.md).

/// Words needed to hold `cap` bits.
fn words_for(cap: u32) -> usize {
    (cap as usize).div_ceil(64)
}

/// A `cap` x `cap` bit matrix, one row per slab slot, stored as a flat word slice. Rows are read
/// and written by slot index; a set bit at `(row, bit)` means the cell in `row` names the cell in
/// `bit`.
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

    fn place(&self, row: u32, bit: u32) -> (usize, u64) {
        let index = row as usize * self.words_per_row + (bit as usize) / 64;
        (index, 1u64 << (bit % 64))
    }

    pub(crate) fn set(&mut self, row: u32, bit: u32) {
        let (index, mask) = self.place(row, bit);
        self.bits[index] |= mask;
    }

    pub(crate) fn test(&self, row: u32, bit: u32) -> bool {
        let (index, mask) = self.place(row, bit);
        self.bits[index] & mask != 0
    }

    /// OR `source`'s row into `dest`'s. The birth relation's only compound write: a new cell's row
    /// starts as its parent's, which is what makes transitive closure hold by construction.
    pub(crate) fn row_or_assign(&mut self, dest: u32, source: u32) {
        let dest = dest as usize * self.words_per_row;
        let source = source as usize * self.words_per_row;
        for offset in 0..self.words_per_row {
            self.bits[dest + offset] |= self.bits[source + offset];
        }
    }

    /// Drop every name in `row`. The whole-row release a cell's death performs.
    pub(crate) fn clear_row(&mut self, row: u32) {
        let start = row as usize * self.words_per_row;
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
