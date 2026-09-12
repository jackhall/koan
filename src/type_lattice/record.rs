//! `Record` — an ordered, [`BinderSymbol`]-keyed field list: the shape behind a struct schema's
//! `(name, type)` fields, a function type's parameter list, and a constructor application's
//! arguments.
//!
//! Keys are [`BinderSymbol`]s, never text: a field name is a fixed-width content digest carried
//! alongside the binding class its own parse established, so a lookup is a `u128` compare and no
//! field name is ever copied or re-classified. Rendering resolves the text back through the run's
//! label interner ([`LabelInterner`](crate::parse::LabelInterner)).
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

use crate::parse::{BinderSymbol, Symbol};

use super::handle::KType;

/// See the module-level documentation for the invariants.
#[derive(Clone, Copy, Debug)]
pub struct Record<'run>(&'run [(BinderSymbol, KType)]);

impl<'run> Record<'run> {
    /// A record over `fields`, which already live where the record will. The registry's record
    /// doors are the callers: each bumps the caller's run into the region on a miss and wraps it
    /// here.
    pub(super) fn over(fields: &'run [(BinderSymbol, KType)]) -> Self {
        debug_assert!(
            fields.iter().enumerate().all(|(index, (name, _))| {
                fields[..index]
                    .iter()
                    .all(|(earlier, _)| earlier.symbol() != name.symbol())
            }),
            "a record's field names are unique",
        );
        Record(fields)
    }

    /// Fields in declaration order, as the slice a transient record travels as.
    pub fn as_slice(self) -> &'run [(BinderSymbol, KType)] {
        self.0
    }

    /// Fields in declaration order.
    pub fn iter(
        self,
    ) -> impl DoubleEndedIterator<Item = (BinderSymbol, KType)> + ExactSizeIterator + 'run {
        self.0.iter().copied()
    }

    pub fn keys(self) -> impl DoubleEndedIterator<Item = BinderSymbol> + ExactSizeIterator + 'run {
        self.0.iter().map(|(name, _)| *name)
    }

    pub fn values(self) -> impl DoubleEndedIterator<Item = KType> + ExactSizeIterator + 'run {
        self.0.iter().map(|(_, value)| *value)
    }

    pub fn get(self, name: Symbol) -> Option<KType> {
        self.0
            .iter()
            .find(|(key, _)| key.symbol() == name)
            .map(|(_, value)| *value)
    }

    /// Recover a stored key's binding class alongside its value. Witnessed: the interning door took
    /// a classified key, so a hit hands back the class its declaration established.
    pub fn get_key_value(self, name: Symbol) -> Option<(BinderSymbol, KType)> {
        self.0.iter().find(|(key, _)| key.symbol() == name).copied()
    }

    /// The field's position in declaration order — the index a positional view aligns against.
    pub fn get_index_of(self, name: Symbol) -> Option<usize> {
        self.0.iter().position(|(key, _)| key.symbol() == name)
    }

    pub fn len(self) -> usize {
        self.0.len()
    }

    pub fn is_empty(self) -> bool {
        self.0.is_empty()
    }
}

/// Order-blind: same set of `(symbol, value)` pairs, regardless of declaration order. Keys are
/// unique, so matching every field of `self` in `other` at equal length is set equality.
impl PartialEq for Record<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len()
            && self
                .0
                .iter()
                .all(|(key, value)| other.get(key.symbol()) == Some(*value))
    }
}
impl Eq for Record<'_> {}

impl<'run> IntoIterator for Record<'run> {
    type Item = (BinderSymbol, KType);
    type IntoIter = std::iter::Copied<std::slice::Iter<'run, (BinderSymbol, KType)>>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter().copied()
    }
}
