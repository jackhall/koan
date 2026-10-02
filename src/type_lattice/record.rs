//! `Record` — an ordered, [`BinderSymbol`]-keyed field list: the shape behind a struct schema's
//! `(name, type)` fields, a function type's parameter list, and a constructor application's
//! arguments.
//!
//! Keys are [`BinderSymbol`]s, never text: a field name is a fixed-width content digest carried
//! alongside the binding class its own parse established, so a lookup is a `u128` compare and no
//! field name is ever copied or re-classified. Rendering resolves the text back through the run's
//! symbol interner ([`SymbolInterner`](crate::symbols::SymbolInterner)).
//!
//! Identity is the key's [`Symbol`] bits alone — equality and the type digest both read
//! `key.symbol()` and never the variant tag, so a schema's class rides past the intern boundary
//! without widening what makes two records the same. Probe doors ([`Record::get`],
//! [`Record::get_index_of`]) take a bare [`Symbol`] for the same reason; a stored key's class comes
//! back through [`Record::get_key_value`], witnessed because the interning door took a classified
//! key.
//!
//! A record is one `Copy` fat pointer into the run region — no index table. At record sizes a
//! linear symbol compare beats hashing. Only the registry's record doors build one, bumping the
//! caller's field run into the region on an intern miss.
//!
//! Two invariants define it:
//!
//! - **Declaration order is preserved** for rendering and positional construction, but
//!   **equality ignores it**: `(x :Number, y :Str)` and `(y :Str, x :Number)` are the same
//!   record. The digest agrees by feeding the fields symbol-sorted.
//! - **Names are unique** within a record. The parser rejects duplicate fields upstream, in
//!   `STRUCT` / `SIG` declarations and in record literals alike, and the constructor asserts it.
//!
//! See [README.md](README.md) § Records and schemas.

use std::marker::PhantomData;

use crate::symbols::{BinderSymbol, Symbol};

use super::handle::{Handle, TypeHandle, wrap};

/// See the module-level documentation for the invariants. The fields are stored raw and read as
/// `H`: a typed read hands each value back as the reading handle's child.
#[derive(Debug)]
pub struct Record<'run, H = Handle> {
    fields: &'run [(BinderSymbol, Handle)],
    view: PhantomData<H>,
}

impl<H> Clone for Record<'_, H> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<H> Copy for Record<'_, H> {}

impl<'run, H> Record<'run, H> {
    /// A record over `fields`, which already live where the record will. The registry's record
    /// doors are the callers: each bumps the caller's run into the region on a miss and wraps it
    /// here.
    pub(super) fn over(fields: &'run [(BinderSymbol, Handle)]) -> Self {
        debug_assert!(
            fields.iter().enumerate().all(|(index, (name, _))| {
                fields[..index]
                    .iter()
                    .all(|(earlier, _)| earlier.symbol() != name.symbol())
            }),
            "a record's field names are unique",
        );
        Record {
            fields,
            view: PhantomData,
        }
    }

    /// Fields in declaration order, raw.
    pub(super) fn raw(self) -> &'run [(BinderSymbol, Handle)] {
        self.fields
    }

    pub fn len(self) -> usize {
        self.fields.len()
    }

    pub fn is_empty(self) -> bool {
        self.fields.is_empty()
    }

    pub fn keys(self) -> impl DoubleEndedIterator<Item = BinderSymbol> + ExactSizeIterator + 'run {
        self.fields.iter().map(|(name, _)| *name)
    }

    /// The field's position in declaration order — the index a positional view aligns against.
    pub fn get_index_of(self, name: Symbol) -> Option<usize> {
        self.fields.iter().position(|(key, _)| key.symbol() == name)
    }
}

impl<'run, H: TypeHandle> Record<'run, H> {
    /// Fields in declaration order.
    pub fn iter(
        self,
    ) -> impl DoubleEndedIterator<Item = (BinderSymbol, H)> + ExactSizeIterator + 'run {
        self.fields
            .iter()
            .map(|(name, value)| (*name, wrap(*value)))
    }

    pub fn values(self) -> impl DoubleEndedIterator<Item = H> + ExactSizeIterator + 'run {
        self.fields.iter().map(|(_, value)| wrap(*value))
    }

    pub fn get(self, name: Symbol) -> Option<H> {
        self.fields
            .iter()
            .find(|(key, _)| key.symbol() == name)
            .map(|(_, value)| wrap(*value))
    }

    /// Recover a stored key's binding class alongside its value. Witnessed: the interning door took
    /// a classified key, so a hit hands back the class its declaration established.
    pub fn get_key_value(self, name: Symbol) -> Option<(BinderSymbol, H)> {
        self.fields
            .iter()
            .find(|(key, _)| key.symbol() == name)
            .map(|(key, value)| (*key, wrap(*value)))
    }
}

/// Order-blind: same set of `(symbol, value)` pairs, regardless of declaration order. Keys are
/// unique, so matching every field of `self` in `other` at equal length is set equality.
impl<H> PartialEq for Record<'_, H> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len()
            && self.fields.iter().all(|(key, value)| {
                other
                    .fields
                    .iter()
                    .any(|(held, theirs)| held.symbol() == key.symbol() && theirs == value)
            })
    }
}
impl<H> Eq for Record<'_, H> {}

impl<'run, H: TypeHandle> IntoIterator for Record<'run, H> {
    type Item = (BinderSymbol, H);
    type IntoIter = std::iter::Map<
        std::slice::Iter<'run, (BinderSymbol, Handle)>,
        fn(&(BinderSymbol, Handle)) -> (BinderSymbol, H),
    >;
    fn into_iter(self) -> Self::IntoIter {
        self.fields
            .iter()
            .map(|(name, value)| (*name, wrap(*value)))
    }
}
