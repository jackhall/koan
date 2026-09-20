//! The **two-channel run**: a body's or the builtin table's declared names, the value channel and
//! the type channel each sorted by symbol, sharing one index space with values first and types
//! after.

use crate::memory::{BumpAllocator, BumpVec, Writer, collect};
use crate::symbols::{BinderSymbol, TypeSymbol, ValueSymbol};

/// Two sorted runs sharing one index space: value entries first, type entries after.
#[derive(Clone, Copy)]
pub(crate) struct Channels<'a, P> {
    values: &'a [(ValueSymbol, P)],
    types: &'a [(TypeSymbol, P)],
}

impl<'a, P: Copy> Channels<'a, P> {
    /// The run with no entry in either channel.
    pub(crate) const EMPTY: Self = Channels {
        values: &[],
        types: &[],
    };

    /// A view over two runs, each already sorted by symbol with no name repeated.
    pub(crate) fn new(values: &'a [(ValueSymbol, P)], types: &'a [(TypeSymbol, P)]) -> Self {
        debug_assert!(values.is_sorted_by(|left, right| left.0 < right.0));
        debug_assert!(types.is_sorted_by(|left, right| left.0 < right.0));
        Channels { values, types }
    }

    /// Both channels sorted in `scratch` and laid down in `writer`'s region.
    ///
    /// Panics on a name given twice in one channel.
    pub(crate) fn sorted_in(
        writer: Writer<'a>,
        scratch: BumpAllocator<'_>,
        values: &[(ValueSymbol, P)],
        types: &[(TypeSymbol, P)],
    ) -> Self {
        Channels {
            values: sorted(writer, scratch, values),
            types: sorted(writer, scratch, types),
        }
    }

    /// How many entries both channels hold.
    pub(crate) fn len(&self) -> usize {
        self.values.len() + self.types.len()
    }

    /// The combined index of `name`, searched in its own channel.
    pub(crate) fn find(&self, name: BinderSymbol) -> Option<usize> {
        match name {
            BinderSymbol::Value(name) => self
                .values
                .binary_search_by_key(&name, |(symbol, _)| *symbol)
                .ok(),
            BinderSymbol::Type(name) => self
                .types
                .binary_search_by_key(&name, |(symbol, _)| *symbol)
                .ok()
                .map(|index| self.values.len() + index),
        }
    }

    /// The name at `index`. Panics past the end, like a slice index.
    pub(crate) fn name(&self, index: usize) -> BinderSymbol {
        match index.checked_sub(self.values.len()) {
            None => BinderSymbol::Value(self.values[index].0),
            Some(ty) => BinderSymbol::Type(self.types[ty].0),
        }
    }

    /// The payload at `index`. Panics past the end, like a slice index.
    pub(crate) fn get(&self, index: usize) -> P {
        match index.checked_sub(self.values.len()) {
            None => self.values[index].1,
            Some(ty) => self.types[ty].1,
        }
    }
}

/// `entries` sorted by name into the region, with a repeated name refused.
fn sorted<'cell, K: Ord + Copy, P: Copy>(
    writer: Writer<'cell>,
    scratch: BumpAllocator<'_>,
    entries: &[(K, P)],
) -> &'cell [(K, P)] {
    let mut staged = BumpVec::with_capacity_in(entries.len(), scratch);
    staged.extend_from_slice(entries);
    staged.sort_unstable_by_key(|(name, _)| *name);
    assert!(
        staged.windows(2).all(|pair| pair[0].0 != pair[1].0),
        "a channel names each entry once"
    );
    collect(writer, staged.iter().copied())
}
