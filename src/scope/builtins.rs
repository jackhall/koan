//! The **builtin table**: every builtin value and type, sorted by name, laid down once and shared by
//! every activation through its header's base pointer.
//!
//! A builtin is unshadowable, so a name found here resolves here from any depth, and a shape turns
//! it into a [`BuiltinIndex`] where it is built. Values come first and types after, so one index
//! space serves both channels.

use crate::memory::{BumpAllocator, Writer, resident};
use crate::symbols::{BinderSymbol, TypeSymbol, ValueSymbol};
use crate::values::{Knotted, Nothing, Value};

use super::channels::Channels;
use super::shape::BuiltinIndex;

/// The builtin names and what they are bound to.
#[derive(Clone, Copy)]
pub struct Builtins<'graph, 'cell, X = Nothing> {
    names: Channels<'cell, Value<'graph, 'cell, X>>,
}

impl<'graph, 'cell, X: Knotted> Builtins<'graph, 'cell, X> {
    /// The table with no builtin in it.
    pub fn empty() -> &'cell Builtins<'graph, 'cell, X> {
        &Builtins {
            names: Channels::EMPTY,
        }
    }

    /// Lay a table down in `writer`'s region, sorting each channel in `scratch` first.
    ///
    /// Panics on a name given twice in one channel: the embedder assembles the table, and two
    /// builtins under one name is its bug.
    pub fn new(
        writer: Writer<'cell>,
        scratch: BumpAllocator<'_>,
        values: &[(ValueSymbol, Value<'graph, 'cell, X>)],
        types: &[(TypeSymbol, Value<'graph, 'cell, X>)],
    ) -> &'cell Builtins<'graph, 'cell, X> {
        let names = Channels::sorted_in(writer, scratch, values, types);
        resident(writer, Builtins { names })
    }

    /// The index `name` names, in either channel.
    pub fn lookup(&self, name: BinderSymbol) -> Option<BuiltinIndex> {
        self.names
            .find(name)
            .map(|index| BuiltinIndex(index as u32))
    }

    /// What `index` is bound to. Panics past the table's end, like a slice index.
    pub fn get(&self, index: BuiltinIndex) -> Value<'graph, 'cell, X> {
        self.names.get(index.index())
    }

    /// How many builtins the table holds, in both channels.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
