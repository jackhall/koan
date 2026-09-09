//! `Record<V>` — an ordered, [`BinderSymbol`]-keyed map: the shape behind a struct schema's
//! `(name, type)` fields and the FN parameter list. Generic over the value, though the registry's
//! nodes are the only residents, so `Record<KType>` is what it is instantiated at; a record
//! *value* lays its cells out in a region-hosted substrate instead
//! ([`ContainerSubstrate`](crate::machine::model::ContainerSubstrate)).
//!
//! Keys are [`BinderSymbol`]s, never text: a field name is a fixed-width content digest carried
//! alongside the binding class its own parse established, so a lookup is a `u128` compare and no
//! field name is ever copied or re-classified. Rendering resolves the text back through the run's
//! label interner ([`LabelInterner`](crate::machine::model::LabelInterner)).
//!
//! Identity is the key's [`Symbol`] bits alone — equality, hashing and the type digest all read
//! `key.symbol()` and never the variant tag, so a schema's class rides past the intern boundary
//! without widening what makes two records the same. Probe doors ([`Record::get`],
//! [`Record::get_index_of`], [`Record::remove`]) take a bare [`Symbol`] for the same reason; a
//! stored key's class comes back through [`Record::get_key_value`], witnessed because insertion
//! required a classified key.
//!
//! The backing is a plain `Vec<(BinderSymbol, V)>` — one allocation, no index table. At record
//! sizes a linear symbol compare beats hashing.
//!
//! Owned `Record`s exist only where content outlives every region — the type registry's nodes.
//! Everywhere transient the currency is a borrowed slice bumped in whichever region hosts it;
//! [`Record::from_slice`] is the one copy, paid at intern-boundary.
//!
//! Two invariants define it:
//!
//! - **Insertion order is preserved** for rendering and positional construction, but
//!   **equality ignores it**: `(x :Number, y :Str)` and `(y :Str, x :Number)` are the
//!   same record.
//! - **Hashing agrees with that order-blind equality**: a commutative fold
//!   (`wrapping_add`, not XOR — XOR cancels on a duplicate) over a per-field
//!   `mix(hash(symbol), hash(value))`. The `mix` binds name to value before the fold,
//!   so `{x: Number}` and `{y: Number}` hash apart.
//!
//! Names are unique within a record. The parser rejects duplicate fields upstream, in `STRUCT` /
//! `SIG` declarations and in record literals alike; if one ever reached [`Record::from_pairs`], the
//! last-wins insert still leaves keys unique, so `Hash`/`Eq` stay well-defined.
//!
//! See [design/label-interning.md](../../../../design/label-interning.md).

use std::hash::{Hash, Hasher};

use crate::machine::model::labels::{BinderSymbol, Symbol};

/// See the module-level documentation for the invariants.
#[derive(Clone, Debug, Default)]
pub struct Record<V> {
    fields: Vec<(BinderSymbol, V)>,
}

impl<V> Record<V> {
    pub fn new() -> Self {
        Record { fields: Vec::new() }
    }

    /// Build from `(name, value)` pairs in declaration order. Last-wins on a duplicate
    /// name — a defensive default; the parser rejects duplicates upstream.
    pub fn from_pairs(pairs: impl IntoIterator<Item = (BinderSymbol, V)>) -> Self {
        let mut record = Record::new();
        for (name, value) in pairs {
            record.insert(name, value);
        }
        record
    }

    /// The intern-boundary copy: a transient slice becomes owned content. The single allocation a
    /// type node's field record pays, amortized because equal content interns to one node per run.
    pub fn from_slice(pairs: &[(BinderSymbol, V)]) -> Self
    where
        V: Clone,
    {
        Record::from_pairs(pairs.iter().cloned())
    }

    /// Fields in insertion (declaration) order, as the slice a transient record travels as.
    pub fn as_slice(&self) -> &[(BinderSymbol, V)] {
        &self.fields
    }

    /// Fields in insertion (declaration) order.
    pub fn iter(&self) -> FieldIter<'_, V> {
        self.fields.iter().map(borrow_field as fn(_) -> _)
    }

    /// Consume into owned `(name, value)` pairs in insertion order.
    pub fn into_pairs(self) -> impl Iterator<Item = (BinderSymbol, V)> {
        self.fields.into_iter()
    }

    pub fn keys(&self) -> impl Iterator<Item = BinderSymbol> + '_ {
        self.fields.iter().map(|(name, _)| *name)
    }

    pub fn values(&self) -> impl DoubleEndedIterator<Item = &V> {
        self.fields.iter().map(|(_, value)| value)
    }

    pub fn get(&self, name: Symbol) -> Option<&V> {
        self.fields
            .iter()
            .find(|(key, _)| key.symbol() == name)
            .map(|(_, value)| value)
    }

    /// Recover a stored key's binding class alongside its value. Witnessed: insertion took a
    /// classified key, so a hit hands back the class its declaration established.
    pub fn get_key_value(&self, name: Symbol) -> Option<(BinderSymbol, &V)> {
        self.fields
            .iter()
            .find(|(key, _)| key.symbol() == name)
            .map(|(key, value)| (*key, value))
    }

    /// The field's position in insertion order — the index a positional view aligns against.
    pub fn get_index_of(&self, name: Symbol) -> Option<usize> {
        self.fields.iter().position(|(key, _)| key.symbol() == name)
    }

    /// A new name appends in insertion order; a replace keeps the existing position.
    pub fn insert(&mut self, name: BinderSymbol, value: V) -> Option<V> {
        match self.get_index_of(name.symbol()) {
            Some(index) => Some(std::mem::replace(&mut self.fields[index].1, value)),
            None => {
                self.fields.push((name, value));
                None
            }
        }
    }

    /// `swap_remove`: O(1) but does not preserve order.
    pub fn remove(&mut self, name: Symbol) -> Option<V> {
        self.get_index_of(name)
            .map(|index| self.fields.swap_remove(index).1)
    }

    /// Map each field's value through `f`, preserving names and declaration order.
    pub fn map<U>(&self, f: impl Fn(&V) -> U) -> Record<U> {
        Record {
            fields: self
                .fields
                .iter()
                .map(|(name, value)| (*name, f(value)))
                .collect(),
        }
    }

    pub fn len(&self) -> usize {
        self.fields.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
}

/// Order-blind: same set of `(symbol, value)` pairs, regardless of declaration order. Keys are
/// unique, so matching every field of `self` in `other` at equal length is set equality.
impl<V: PartialEq> PartialEq for Record<V> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len()
            && self
                .fields
                .iter()
                .all(|(key, value)| other.get(key.symbol()) == Some(value))
    }
}
impl<V: Eq> Eq for Record<V> {}

impl<V: Hash> Hash for Record<V> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Commutative fold so the hash is order-independent, matching the order-blind
        // `PartialEq`. Each field contributes `mix(hash(symbol), hash(value))`; the
        // wrapping-add accumulator is symmetric, so reordering fields can't change it.
        let mut acc: u64 = 0;
        for (key, value) in &self.fields {
            acc = acc.wrapping_add(field_hash(key.symbol(), value));
        }
        state.write_u64(acc);
    }
}

/// `mix(hash(symbol), hash(value))` — fold name and value into one hash so that
/// `{x: Number}` and `{y: Number}` (same value, different name) differ. The symbol bits alone
/// feed it: identity must not see the key's variant tag.
fn field_hash<V: Hash>(symbol: Symbol, value: &V) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    symbol.hash(&mut h);
    value.hash(&mut h);
    h.finish()
}

/// The pair-borrowing shape `iter` and `IntoIterator` share: the key is `Copy`, so a field reads as
/// `(BinderSymbol, &V)` rather than a reference pair.
type FieldIter<'a, V> = std::iter::Map<
    std::slice::Iter<'a, (BinderSymbol, V)>,
    fn(&'a (BinderSymbol, V)) -> (BinderSymbol, &'a V),
>;

fn borrow_field<V>(field: &(BinderSymbol, V)) -> (BinderSymbol, &V) {
    (field.0, &field.1)
}

impl<'a, V> IntoIterator for &'a Record<V> {
    type Item = (BinderSymbol, &'a V);
    type IntoIter = FieldIter<'a, V>;
    fn into_iter(self) -> Self::IntoIter {
        self.fields.iter().map(borrow_field as fn(_) -> _)
    }
}

impl<V> FromIterator<(BinderSymbol, V)> for Record<V> {
    fn from_iter<I: IntoIterator<Item = (BinderSymbol, V)>>(iter: I) -> Self {
        Record::from_pairs(iter)
    }
}

#[cfg(test)]
mod tests;
