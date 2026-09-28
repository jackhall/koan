//! The **three-channel run**: a body's declared names, the value channel, the type channel and the
//! registration channel each sorted by symbol, sharing one index space with values first, types
//! after, and registrations last.

use crate::memory::{BumpAllocator, BumpVec, Writer, collect};
use crate::symbols::{BinderSymbol, RegistrationSymbol, TypeSymbol, ValueSymbol};

/// Three sorted runs sharing one index space: value entries first, type entries after, and
/// registration entries last.
#[derive(Clone, Copy)]
pub(crate) struct Channels<'a, P> {
    values: &'a [(ValueSymbol, P)],
    types: &'a [(TypeSymbol, P)],
    registrations: &'a [(RegistrationSymbol, P)],
}

impl<'a, P: Copy> Channels<'a, P> {
    /// The run with no entry in any channel.
    pub(crate) const EMPTY: Self = Channels {
        values: &[],
        types: &[],
        registrations: &[],
    };

    /// A view over three runs, each already sorted by symbol with no name repeated.
    pub(crate) fn new(
        values: &'a [(ValueSymbol, P)],
        types: &'a [(TypeSymbol, P)],
        registrations: &'a [(RegistrationSymbol, P)],
    ) -> Self {
        debug_assert!(values.is_sorted_by(|left, right| left.0 < right.0));
        debug_assert!(types.is_sorted_by(|left, right| left.0 < right.0));
        debug_assert!(registrations.is_sorted_by(|left, right| left.0 < right.0));
        Channels {
            values,
            types,
            registrations,
        }
    }

    /// The value and type channels sorted in `scratch` and laid down in `writer`'s region, with no
    /// registration.
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
            registrations: &[],
        }
    }

    /// How many entries the channels hold.
    pub(crate) fn len(&self) -> usize {
        self.values.len() + self.types.len() + self.registrations.len()
    }

    /// The combined index of `name`, searched in its own channel.
    pub(crate) fn find(&self, name: BinderSymbol) -> Option<usize> {
        match name {
            BinderSymbol::Value(name) => search(self.values, name),
            BinderSymbol::Type(name) => {
                search(self.types, name).map(|index| self.values.len() + index)
            }
            BinderSymbol::Registration(name) => search(self.registrations, name)
                .map(|index| self.values.len() + self.types.len() + index),
        }
    }

    /// The name at `index`. Panics past the end, like a slice index.
    pub(crate) fn name(&self, index: usize) -> BinderSymbol {
        match self.locate(index) {
            (Channel::Value, index) => BinderSymbol::Value(self.values[index].0),
            (Channel::Type, index) => BinderSymbol::Type(self.types[index].0),
            (Channel::Registration, index) => {
                BinderSymbol::Registration(self.registrations[index].0)
            }
        }
    }

    /// The payload at `index`. Panics past the end, like a slice index.
    pub(crate) fn get(&self, index: usize) -> P {
        match self.locate(index) {
            (Channel::Value, index) => self.values[index].1,
            (Channel::Type, index) => self.types[index].1,
            (Channel::Registration, index) => self.registrations[index].1,
        }
    }

    /// The channel `index` falls in, and its index within that channel.
    fn locate(&self, index: usize) -> (Channel, usize) {
        let Some(past_values) = index.checked_sub(self.values.len()) else {
            return (Channel::Value, index);
        };
        match past_values.checked_sub(self.types.len()) {
            None => (Channel::Type, past_values),
            Some(registration) => (Channel::Registration, registration),
        }
    }
}

/// One of the three channels.
enum Channel {
    Value,
    Type,
    Registration,
}

/// Where `name` sits in the sorted `run`.
fn search<K: Ord + Copy, P>(run: &[(K, P)], name: K) -> Option<usize> {
    run.binary_search_by_key(&name, |(symbol, _)| *symbol).ok()
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
