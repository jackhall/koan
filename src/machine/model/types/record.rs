//! `Record<V>` — an ordered identifier-keyed map: the single shape behind a struct
//! schema's `(name, type)` fields, and (later) the FN/FUNCTOR parameter list and the
//! runtime binding carriers. Generic over the value so the type level stores
//! `Record<KType>` and the value level can store `Record<KObject>`.
//!
//! Two properties define it (see the [record-substrate] roadmap item):
//!
//! - **Insertion order is preserved** for rendering and positional construction
//!   ([`iter`](Record::iter) walks declaration order), but **equality ignores it**:
//!   `(x :Number, y :Str)` and `(y :Str, x :Number)` are the same record. The
//!   order-blind `PartialEq` is `IndexMap`'s, forwarded for free.
//! - **Hashing agrees with that order-blind equality**: a commutative fold
//!   (`wrapping_add`, not XOR — XOR cancels on a duplicate) over a per-field
//!   `mix(hash(name), hash(value))`. The `mix` binds name to value before the fold,
//!   so `{x: Number}` and `{y: Number}` hash apart.
//!
//! Names are unique within a record — the structural invariant `IndexMap` keys carry
//! for free. The `STRUCT` / `SIG` parser ([`parse_pair_list`](crate::parse)) already
//! rejects a duplicate field before one reaches [`from_pairs`]; if one ever arrived,
//! `IndexMap`'s last-wins insert would still leave the keys unique, so `Hash`/`Eq`
//! stay well-defined.
//!
//! [record-substrate]: ../../../../roadmap/type_language/record-substrate.md

use indexmap::IndexMap;
use std::hash::{Hash, Hasher};

/// An ordered identifier-keyed map with order-blind equality and a commutative
/// name+value hash. See the module-level documentation for the invariants.
#[derive(Clone, Debug, Default)]
pub struct Record<V> {
    fields: IndexMap<String, V>,
}

impl<V> Record<V> {
    /// Empty record. Equivalent to `from_pairs([])`.
    pub fn new() -> Self {
        Record {
            fields: IndexMap::new(),
        }
    }

    /// Build from `(name, value)` pairs in declaration order. Last-wins on a duplicate
    /// name (the upstream `parse_pair_list` rejects duplicates before this point, so the
    /// last-wins arm is a defensive default, not a routine path).
    pub fn from_pairs(pairs: impl IntoIterator<Item = (String, V)>) -> Self {
        Record {
            fields: pairs.into_iter().collect(),
        }
    }

    /// Fields in insertion (declaration) order.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &V)> {
        self.fields.iter()
    }

    /// Field names in insertion order.
    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.fields.keys()
    }

    /// Field values in insertion order.
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.fields.values()
    }

    /// Look up a field's value by name. O(1).
    pub fn get(&self, name: &str) -> Option<&V> {
        self.fields.get(name)
    }

    /// Number of fields.
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    /// Whether the record has no fields.
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
}

/// Order-blind: same set of `(name, value)` pairs, regardless of declaration order.
/// Forwarded from `IndexMap`, whose `PartialEq` already compares entries unordered.
impl<V: PartialEq> PartialEq for Record<V> {
    fn eq(&self, other: &Self) -> bool {
        self.fields == other.fields
    }
}
impl<V: Eq> Eq for Record<V> {}

impl<V: Hash> Hash for Record<V> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Commutative fold so the hash is order-independent, matching the order-blind
        // `PartialEq`. Each field contributes `mix(hash(name), hash(value))`; the
        // wrapping-add accumulator is symmetric, so reordering fields can't change it.
        let mut acc: u64 = 0;
        for (name, value) in &self.fields {
            acc = acc.wrapping_add(field_hash(name, value));
        }
        state.write_u64(acc);
    }
}

/// `mix(hash(name), hash(value))` — fold name and value into one hash so that
/// `{x: Number}` and `{y: Number}` (same value, different name) differ.
fn field_hash<V: Hash>(name: &str, value: &V) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut h);
    value.hash(&mut h);
    h.finish()
}

impl<'a, V> IntoIterator for &'a Record<V> {
    type Item = (&'a String, &'a V);
    type IntoIter = indexmap::map::Iter<'a, String, V>;
    fn into_iter(self) -> Self::IntoIter {
        self.fields.iter()
    }
}

impl<V> FromIterator<(String, V)> for Record<V> {
    fn from_iter<I: IntoIterator<Item = (String, V)>>(iter: I) -> Self {
        Record::from_pairs(iter)
    }
}

#[cfg(test)]
mod tests;
