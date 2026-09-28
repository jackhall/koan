//! The **builtin table**: every builtin value and type, sorted by name, and every builtin overload,
//! grouped by bucket key — laid down once and shared by every activation through its header's base
//! pointer.
//!
//! A builtin is unshadowable, so a name found here resolves here from any depth, and a shape turns
//! it into a [`BuiltinIndex`] where it is built. An overload has no name: a keyworded use lists the
//! overloads at its key first among its candidates. Values come first, types after and overloads
//! last, so one index space serves all three.

use std::ops::Range;

use crate::memory::{BumpAllocator, BumpVec, Writer, collect, resident};
use crate::symbols::{BinderSymbol, KeySymbol, TypeSymbol, ValueSymbol};
use crate::values::{Knotted, Nothing, Value};

use super::channels::Channels;
use super::shape::BuiltinIndex;

/// The builtin names and overloads, and what they are bound to.
#[derive(Clone, Copy)]
pub struct Builtins<'cell, X = Nothing> {
    names: Channels<'cell, Value<'cell, X>>,
    /// Every overload beside its bucket key, sorted by key and in table order within one.
    overloads: &'cell [(KeySymbol, Value<'cell, X>)],
}

impl<'cell, X: Knotted> Builtins<'cell, X> {
    /// The table with no builtin in it.
    pub fn empty() -> &'cell Builtins<'cell, X> {
        &Builtins {
            names: Channels::EMPTY,
            overloads: &[],
        }
    }

    /// Lay a table down in `writer`'s region, sorting each channel and grouping the overloads by
    /// key in `scratch` first. Overloads at one key keep the order they are given in.
    ///
    /// Panics on a name given twice in one channel: the embedder assembles the table, and two
    /// builtins under one name is its bug.
    pub fn new(
        writer: Writer<'cell>,
        scratch: BumpAllocator<'_>,
        values: &[(ValueSymbol, Value<'cell, X>)],
        types: &[(TypeSymbol, Value<'cell, X>)],
        overloads: &[(KeySymbol, Value<'cell, X>)],
    ) -> &'cell Builtins<'cell, X> {
        let names = Channels::sorted_in(writer, scratch, values, types);
        let mut grouped = BumpVec::with_capacity_in(overloads.len(), scratch);
        grouped.extend_from_slice(overloads);
        grouped.sort_by_key(|(key, _)| *key);
        let overloads = collect(writer, grouped.iter().copied());
        resident(writer, Builtins { names, overloads })
    }

    /// The index `name` names, in either channel.
    pub fn lookup(&self, name: BinderSymbol) -> Option<BuiltinIndex> {
        self.names
            .find(name)
            .map(|index| BuiltinIndex(index as u32))
    }

    /// The overloads at `key`, in table order.
    pub fn overloads(&self, key: KeySymbol) -> impl Iterator<Item = BuiltinIndex> + use<X> {
        self.overload_range(key).map(BuiltinIndex)
    }

    /// What `index` is bound to. Panics past the table's end, like a slice index.
    pub fn get(&self, index: BuiltinIndex) -> Value<'cell, X> {
        match index.index().checked_sub(self.names.len()) {
            None => self.names.get(index.index()),
            Some(overload) => self.overloads[overload].1,
        }
    }

    /// How many builtins the table holds, names and overloads.
    pub fn len(&self) -> usize {
        self.names.len() + self.overloads.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The indices of the overloads at `key`.
    fn overload_range(&self, key: KeySymbol) -> Range<u32> {
        let start = self.overloads.partition_point(|(held, _)| *held < key);
        let end = start + self.overloads[start..].partition_point(|(held, _)| *held == key);
        let base = self.names.len();
        (base + start) as u32..(base + end) as u32
    }
}

/// What the shape builder reads off a builtin table, whatever callable its values hold: the index
/// a name resolves to, and the overloads at a key.
pub(crate) trait BuiltinNames {
    fn lookup(&self, name: BinderSymbol) -> Option<BuiltinIndex>;
    fn overloads(&self, key: KeySymbol) -> Range<u32>;
}

impl<X: Knotted> BuiltinNames for Builtins<'_, X> {
    fn lookup(&self, name: BinderSymbol) -> Option<BuiltinIndex> {
        Builtins::lookup(self, name)
    }

    fn overloads(&self, key: KeySymbol) -> Range<u32> {
        self.overload_range(key)
    }
}
