//! The **builtin table**: every builtin value and type, sorted by name, laid down once and shared by
//! every activation through its header's base pointer.
//!
//! A builtin is unshadowable, so a name found here resolves here from any depth, and a shape turns
//! it into a [`BuiltinIndex`] where it is built. Values come first and types after, so one index
//! space serves both channels.

use crate::memory::{BumpAllocator, BumpVec, Writer};
use crate::parse::{BinderSymbol, TypeSymbol, ValueSymbol};
use crate::values::{Value, collect, resident};

use super::shape::BuiltinIndex;

/// The builtin names and what they are bound to: the value channel and the type channel, each
/// sorted by symbol.
#[derive(Clone, Copy)]
pub struct Builtins<'graph, 'cell> {
    values: &'cell [(ValueSymbol, Value<'graph, 'cell>)],
    types: &'cell [(TypeSymbol, Value<'graph, 'cell>)],
}

impl<'graph, 'cell> Builtins<'graph, 'cell> {
    /// The table with no builtin in it.
    pub const EMPTY: &'static Builtins<'static, 'static> = &Builtins {
        values: &[],
        types: &[],
    };

    /// Lay a table down in `writer`'s region, sorting each channel in `scratch` first.
    ///
    /// Panics on a name given twice in one channel: the embedder assembles the table, and two
    /// builtins under one name is its bug.
    pub fn new(
        writer: Writer<'cell>,
        scratch: BumpAllocator<'_>,
        values: &[(ValueSymbol, Value<'graph, 'cell>)],
        types: &[(TypeSymbol, Value<'graph, 'cell>)],
    ) -> &'cell Builtins<'graph, 'cell> {
        let values = sorted(writer, scratch, values);
        let types = sorted(writer, scratch, types);
        resident(writer, Builtins { values, types })
    }

    /// The index `name` names, in either channel.
    pub fn lookup(&self, name: BinderSymbol) -> Option<BuiltinIndex> {
        match name {
            BinderSymbol::Value(name) => self.value(name),
            BinderSymbol::Type(name) => self.ty(name),
        }
    }

    /// The index of the builtin value `name`.
    pub fn value(&self, name: ValueSymbol) -> Option<BuiltinIndex> {
        let index = self
            .values
            .binary_search_by_key(&name, |(symbol, _)| *symbol)
            .ok()?;
        Some(BuiltinIndex(index as u32))
    }

    /// The index of the builtin type `name`, counted after every value.
    pub fn ty(&self, name: TypeSymbol) -> Option<BuiltinIndex> {
        let index = self
            .types
            .binary_search_by_key(&name, |(symbol, _)| *symbol)
            .ok()?;
        Some(BuiltinIndex((self.values.len() + index) as u32))
    }

    /// What `index` is bound to. Panics past the table's end, like a slice index.
    pub fn get(&self, index: BuiltinIndex) -> Value<'graph, 'cell> {
        let index = index.0 as usize;
        match index.checked_sub(self.values.len()) {
            None => self.values[index].1,
            Some(ty) => self.types[ty].1,
        }
    }

    /// How many builtins the table holds, in both channels.
    pub fn len(&self) -> usize {
        self.values.len() + self.types.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// `entries` sorted by name into the region, with a repeated name refused.
fn sorted<'cell, K: Ord + Copy, V: Copy>(
    writer: Writer<'cell>,
    scratch: BumpAllocator<'_>,
    entries: &[(K, V)],
) -> &'cell [(K, V)] {
    let mut staged = BumpVec::with_capacity_in(entries.len(), scratch);
    staged.extend_from_slice(entries);
    staged.sort_unstable_by_key(|(name, _)| *name);
    assert!(
        staged.windows(2).all(|pair| pair[0].0 != pair[1].0),
        "a builtin table names each builtin once"
    );
    collect(writer, staged.iter().copied())
}
