//! `Record<V>` — an ordered identifier-keyed map: the shape behind a struct schema's
//! `(name, type)` fields, the FN parameter list, and the runtime binding
//! carriers. Generic over the value, so the type level stores `Record<KType>` and the
//! value level stores `Record<KObject>`.
//!
//! Two invariants define it:
//!
//! - **Insertion order is preserved** for rendering and positional construction, but
//!   **equality ignores it**: `(x :Number, y :Str)` and `(y :Str, x :Number)` are the
//!   same record. The order-blind `PartialEq` is `IndexMap`'s, forwarded for free.
//! - **Hashing agrees with that order-blind equality**: a commutative fold
//!   (`wrapping_add`, not XOR — XOR cancels on a duplicate) over a per-field
//!   `mix(hash(name), hash(value))`. The `mix` binds name to value before the fold,
//!   so `{x: Number}` and `{y: Number}` hash apart.
//!
//! Names are unique within a record — an `IndexMap` key invariant. The `STRUCT` / `SIG`
//! parser rejects duplicate fields upstream; if one ever reached [`from_pairs`], the
//! last-wins insert still leaves keys unique, so `Hash`/`Eq` stay well-defined.

use indexmap::IndexMap;
use std::hash::{Hash, Hasher};

/// See the module-level documentation for the invariants.
#[derive(Clone, Debug, Default)]
pub struct Record<V> {
    fields: IndexMap<String, V>,
}

impl<V> Record<V> {
    pub fn new() -> Self {
        Record {
            fields: IndexMap::new(),
        }
    }

    /// Build from `(name, value)` pairs in declaration order. Last-wins on a duplicate
    /// name — a defensive default; the parser rejects duplicates upstream.
    pub fn from_pairs(pairs: impl IntoIterator<Item = (String, V)>) -> Self {
        Record {
            fields: pairs.into_iter().collect(),
        }
    }

    /// Fields in insertion (declaration) order.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &V)> {
        self.fields.iter()
    }

    /// Consume into owned `(name, value)` pairs in insertion order.
    pub fn into_pairs(self) -> impl Iterator<Item = (String, V)> {
        self.fields.into_iter()
    }

    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.fields.keys()
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.fields.values()
    }

    pub fn get(&self, name: &str) -> Option<&V> {
        self.fields.get(name)
    }

    /// A new name appends in insertion order; a replace keeps the existing position.
    pub fn insert(&mut self, name: String, value: V) -> Option<V> {
        self.fields.insert(name, value)
    }

    /// `swap_remove`: O(1) but does not preserve order.
    pub fn remove(&mut self, name: &str) -> Option<V> {
        self.fields.swap_remove(name)
    }

    /// Map each field's value through `f`, preserving names and declaration order.
    pub fn map<U>(&self, f: impl Fn(&V) -> U) -> Record<U> {
        Record {
            fields: self
                .fields
                .iter()
                .map(|(name, value)| (name.clone(), f(value)))
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
