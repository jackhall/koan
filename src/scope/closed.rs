//! A layer's **table of closed types**: the distinct parametric types the unit and closing records
//! it absorbs name over its chain's binding, each beside the closed type it takes under the chain's
//! binding ([build/README.md § The store](../build/README.md#the-store)). A unit record that only
//! names a run-bound name is laid once, at the load, and every binding reads it through its own
//! layer's table: a run reader closes each rigid type a fact names by a probe of the tables its
//! chain's layers hold ([`records`](super::records)), and the walk closes as it absorbs.
//!
//! The keys do not depend on the binding: they are the types the shared records name. So a
//! parametric layer may lay a second slice of values for another binding without walking
//! ([`closed_under`]).
//!
//! A key is a type alone, with no segment, and a lexical variable's handle holds none either. It
//! still names one unknown, because every name a chain binds lies on one lexical chain of bodies,
//! whose levels are disjoint: a layer's parent is the slot its callee was born under.

use crate::memory::{BumpAllocator, BumpVec, Lays, collect};
use crate::type_lattice::{DeclaredType, KType, Parametric};

/// The closed types a layer's binding gives the parametric types its absorbed unit and closing
/// records name: `keys` sorted by digest, which no binding changes, and `values` aligned with them.
#[derive(Clone, Copy, Debug)]
pub struct ClosedTypes<'a> {
    keys: &'a [DeclaredType<Parametric>],
    values: &'a [DeclaredType<KType>],
}

impl<'a> ClosedTypes<'a> {
    /// The table that closes nothing.
    pub const EMPTY: ClosedTypes<'static> = ClosedTypes {
        keys: &[],
        values: &[],
    };

    /// The closed type `ty` takes, where the table holds it.
    pub fn probe(&self, ty: DeclaredType<Parametric>) -> Option<DeclaredType<KType>> {
        let digest = ty.digest();
        let at = (self.keys)
            .binary_search_by_key(&digest, |key| key.digest())
            .ok()?;
        Some(self.values[at])
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Its keys, sorted by digest: what a binding over the same records closes again.
    pub fn keys(&self) -> &'a [DeclaredType<Parametric>] {
        self.keys
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Each key beside its value.
    pub fn entries(&self) -> impl Iterator<Item = (DeclaredType<Parametric>, DeclaredType<KType>)> {
        self.keys.iter().copied().zip(self.values.iter().copied())
    }

    /// Hand `each` every type the table names, keys and values both.
    pub fn each_type(&self, each: &mut dyn FnMut(DeclaredType<Parametric>)) {
        for (key, value) in self.entries() {
            each(key);
            each(match value {
                DeclaredType::Type(closed) => DeclaredType::Type(closed.into()),
                DeclaredType::Scheme(scheme) => DeclaredType::Scheme(scheme),
            });
        }
    }

    /// This table laid again through `lay`.
    pub fn copied<'to>(&self, lay: impl Lays<'to>) -> ClosedTypes<'to> {
        ClosedTypes {
            keys: collect(lay, self.keys.iter().copied()),
            values: collect(lay, self.values.iter().copied()),
        }
    }
}

#[cfg(test)]
thread_local! {
    /// How many closed types tables have laid: the report's count. Tests reset it.
    pub(crate) static CLOSED_LAID: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The table over `keys` under one binding, laid through `lay`: each key `close` closes, sorted
/// by digest and held once. A key `close` leaves open is dropped. The one door a parametric layer
/// reuses to lay values for another binding.
pub fn closed_under<'a>(
    lay: impl Lays<'a>,
    scratch: BumpAllocator<'_>,
    keys: &[DeclaredType<Parametric>],
    close: &mut dyn FnMut(DeclaredType<Parametric>) -> Option<DeclaredType<KType>>,
) -> ClosedTypes<'a> {
    let mut entries = BumpVec::with_capacity_in(keys.len(), scratch);
    entries.extend(
        keys.iter()
            .filter_map(|key| close(*key).map(|value| (*key, value))),
    );
    entries.sort_unstable_by_key(|(key, _)| key.digest());
    entries.dedup_by(|later, first| {
        let same = later.0.digest() == first.0.digest();
        debug_assert!(!same || later.1 == first.1, "a key closes to one type");
        same
    });
    if entries.is_empty() {
        return ClosedTypes::EMPTY;
    }
    #[cfg(test)]
    CLOSED_LAID.with(|laid| laid.set(laid.get() + entries.len()));
    ClosedTypes {
        keys: collect(lay, entries.iter().map(|(key, _)| *key)),
        values: collect(lay, entries.iter().map(|(_, value)| *value)),
    }
}
