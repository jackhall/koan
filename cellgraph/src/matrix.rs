//! Bit storage for the two cell relations, and the single row the executing flag occupies. See
//! [design/liveness-matrix.md](../design/liveness-matrix.md) § Two relations, two structures.
//!
//! Both relations take the same square shape, indexed the same way: row `holder`, bit `held`, so a
//! cell's whole hold set is one contiguous row. That is the axis each relation's *write* wants —
//! the birth derivation ORs a parent's row into its child's, and the pin mint ORs a reach mask
//! into a destination's — and it makes the reclaim query, "does anything still hold this cell",
//! the scan across rows that [`Matrix::held_by_any`] performs.
//!
//! One type, [`Bits`], is every row of bits in the crate: a matrix row viewed into the flat
//! allocation, the executing row, and the slab half of a reach mask. It is generic over what holds
//! its words so a view costs no copy, and [`Bits::place`] is the only word-and-bit arithmetic
//! written anywhere.

use crate::mask::Mask;

/// Words needed to hold `cap` bits.
pub(crate) fn words_for(cap: u32) -> usize {
    (cap as usize).div_ceil(64)
}

/// A row of bits indexed by slab slot, over whatever holds its words: a box for a row that owns
/// its storage, a slice for a view into a matrix's flat allocation.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Bits<W = Box<[u64]>> {
    words: W,
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

impl Bits<Box<[u64]>> {
    /// A row of `cap` clear bits, owning its words.
    pub(crate) fn new(cap: u32) -> Self {
        Bits {
            words: vec![0u64; words_for(cap)].into_boxed_slice(),
        }
    }
}

impl Bits<&[u64]> {
    /// Copy a view out into a row that owns its words — the seal transition's freeze.
    pub(crate) fn to_owned(&self) -> Bits<Box<[u64]>> {
        Bits {
            words: self.words.to_vec().into_boxed_slice(),
        }
    }
}

#[cfg(test)]
impl<'a> Bits<&'a [u64]> {
    /// [`Bits::ones`] at the view's own lifetime, so the iterator outlives the view.
    pub(crate) fn into_ones(self) -> impl Iterator<Item = u32> + 'a {
        ones_of(self.words)
    }
}

impl<W: AsRef<[u64]>> Bits<W> {
    /// How many bits the row holds — a whole number of words, so at least the cap it was built for.
    pub(crate) fn width(&self) -> u32 {
        (self.words.as_ref().len() * u64::BITS as usize) as u32
    }

    /// The word index and the one-bit mask for `bit`. Every other bit operation goes through here,
    /// which is what gives each of them the width assertion.
    fn place(&self, bit: u32) -> (usize, u64) {
        debug_assert!(
            bit < self.width(),
            "bit {bit} is outside a {}-bit row",
            self.width()
        );
        (bit as usize / 64, 1u64 << (bit % 64))
    }

    pub(crate) fn test(&self, bit: u32) -> bool {
        let (index, mask) = self.place(bit);
        self.words.as_ref()[index] & mask != 0
    }

    /// The bits this row sets, in bit order.
    pub(crate) fn ones(&self) -> impl Iterator<Item = u32> + '_ {
        ones_of(self.words.as_ref())
    }

    /// Whether this row sets everything `other` sets — the containment invariant a birth chain
    /// satisfies from parent to child.
    #[cfg(test)]
    pub(crate) fn contains_all(&self, other: &Bits<impl AsRef<[u64]>>) -> bool {
        self.words
            .as_ref()
            .iter()
            .zip(other.words.as_ref())
            .all(|(outer, inner)| outer & inner == *inner)
    }
}

impl<W: AsRef<[u64]> + AsMut<[u64]>> Bits<W> {
    pub(crate) fn set(&mut self, bit: u32) {
        let (index, mask) = self.place(bit);
        self.words.as_mut()[index] |= mask;
    }

    /// Clear one bit, reporting whether it was set.
    pub(crate) fn clear(&mut self, bit: u32) -> bool {
        let (index, mask) = self.place(bit);
        let word = &mut self.words.as_mut()[index];
        let was = *word & mask != 0;
        *word &= !mask;
        was
    }

    /// OR another row of the same width in.
    pub(crate) fn union_with(&mut self, other: &Bits<impl AsRef<[u64]>>) {
        debug_assert_eq!(
            self.words.as_ref().len(),
            other.words.as_ref().len(),
            "rows of different widths do not union"
        );
        for (word, source) in self.words.as_mut().iter_mut().zip(other.words.as_ref()) {
            *word |= *source;
        }
    }

    /// Drop every bit. The whole-row release a cell's death performs.
    pub(crate) fn clear_all(&mut self) {
        self.words.as_mut().fill(0);
    }
}

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

    /// The word range one row occupies in the flat allocation.
    fn range(&self, holder: u32) -> std::ops::Range<usize> {
        let start = holder as usize * self.words_per_row;
        start..start + self.words_per_row
    }

    /// One cell's whole hold set, as a view into the flat allocation. Contiguity is why the seal
    /// transition's freeze consults no storage and scans no values.
    pub(crate) fn row(&self, holder: u32) -> Bits<&[u64]> {
        Bits {
            words: &self.bits[self.range(holder)],
        }
    }

    pub(crate) fn row_mut(&mut self, holder: u32) -> Bits<&mut [u64]> {
        let range = self.range(holder);
        Bits {
            words: &mut self.bits[range],
        }
    }

    pub(crate) fn set(&mut self, holder: u32, held: u32) {
        self.row_mut(holder).set(held);
    }

    pub(crate) fn clear(&mut self, holder: u32, held: u32) {
        self.row_mut(holder).clear(held);
    }

    pub(crate) fn test(&self, holder: u32, held: u32) -> bool {
        self.row(holder).test(held)
    }

    /// Fold a reach mask into `holder`'s hold set, minus `holder`'s own bit — the mint OR, and the
    /// only write into the pin relation ([liveness-matrix.md § Reach as a hybrid
    /// mask](../design/liveness-matrix.md#reach-as-a-hybrid-mask)). The and-not is the self rule: a
    /// cell that held itself alive would never reach a zero hold count.
    pub(crate) fn mint(&mut self, holder: u32, reach: &Mask) {
        let mut row = self.row_mut(holder);
        row.union_with(reach.slab());
        row.clear(holder);
    }

    /// Whether any row in `rows` holds `held`. The reclaim query: a cell is reclaimable only when
    /// no live cell names it in either relation.
    pub(crate) fn held_by_any(&self, rows: impl Iterator<Item = u32>, held: u32) -> bool {
        rows.into_iter().any(|holder| self.test(holder, held))
    }

    /// The cells `holder` names, in slot order — the hold graph's outgoing edges from one cell.
    #[cfg(test)]
    pub(crate) fn held_by(&self, holder: u32) -> impl Iterator<Item = u32> + '_ {
        self.row(holder).into_ones()
    }

    /// OR `source`'s row into `dest`'s. The birth relation's only compound write: a new cell's row
    /// starts as its parent's, which is what makes transitive closure hold by construction.
    pub(crate) fn inherit_row(&mut self, dest: u32, source: u32) {
        debug_assert_ne!(dest, source, "a row does not inherit from itself");
        let (dest_range, source_range) = (self.range(dest), self.range(source));
        // Split the flat allocation at the later row's start, so each half holds one whole row.
        let (low, high) = self
            .bits
            .split_at_mut(dest_range.start.max(source_range.start));
        let (dest_words, source_words) = if dest_range.start < source_range.start {
            (&mut low[dest_range], &high[..self.words_per_row])
        } else {
            (&mut high[..self.words_per_row], &low[source_range])
        };
        Bits { words: dest_words }.union_with(&Bits {
            words: source_words,
        });
    }

    /// Drop every name in `holder`'s row. The whole-row release a cell's death performs.
    pub(crate) fn clear_row(&mut self, holder: u32) {
        self.row_mut(holder).clear_all();
    }

    /// Whether `outer`'s row names everything `inner`'s row names — the containment invariant a
    /// birth chain satisfies from parent to child.
    #[cfg(test)]
    pub(crate) fn row_contains(&self, outer: u32, inner: u32) -> bool {
        self.row(outer).contains_all(&self.row(inner))
    }
}
