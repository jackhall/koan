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
//! Beside the types, the table holds the content trees the absorbed records hold that the binding
//! changes: each open tree's identity beside the tree the binding gives it, and each closed subtree
//! of those by its digest, so a read of the module's content by its digest finds it. The walk
//! builds each such tree as a fresh load reads it, since a view's fit or a retype's collapse is
//! the walk's to redo ([`Tree`]).
//!
//! A key is a type alone, with no segment, and a lexical variable's handle holds none either. It
//! still names one unknown, because every name a chain binds lies on one lexical chain of bodies,
//! whose levels are disjoint: a layer's parent is the slot its callee was born under.

use crate::memory::{BumpAllocator, BumpVec, Lays, collect};
use crate::type_lattice::{DeclaredType, Parametric};

use super::record_types::Closed;
use super::tree::{Identity, Tree};

/// The closed types a layer's binding gives the parametric types its absorbed unit and closing
/// records name: `keys` sorted by digest, which no binding changes, and `values` aligned with them,
/// each the type with every name the binding binds written in — closed, or partly where the chain
/// leaves a name it holds open, which a later layer binding the rest closes whole.
#[derive(Clone, Copy)]
pub struct ClosedTypes<'a> {
    keys: &'a [DeclaredType<Parametric>],
    values: &'a [Closed],
    /// Each tree the binding changes, by identity, sorted: an open tree's beside its substitution,
    /// a closed tree's beside itself.
    trees: &'a [(Identity, &'a Tree<'a>)],
}

impl std::fmt::Debug for ClosedTypes<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClosedTypes")
            .field("keys", &self.keys)
            .field("values", &self.values)
            .field("trees", &self.trees.len())
            .finish()
    }
}

impl<'a> ClosedTypes<'a> {
    /// The table that closes nothing.
    pub const EMPTY: ClosedTypes<'static> = ClosedTypes {
        keys: &[],
        values: &[],
        trees: &[],
    };

    /// What `ty` takes, where the table holds it: never [`Closed::Same`].
    pub fn probe(&self, ty: DeclaredType<Parametric>) -> Option<Closed> {
        let digest = ty.digest();
        let at = (self.keys)
            .binary_search_by_key(&digest, |key| key.digest())
            .ok()?;
        Some(self.values[at])
    }

    /// The tree the binding gives the tree of `identity`, where the table holds it.
    pub fn tree(&self, identity: Identity) -> Option<&'a Tree<'a>> {
        let at = (self.trees)
            .binary_search_by_key(&identity, |(held, _)| *held)
            .ok()?;
        Some(self.trees[at].1)
    }

    /// This table beside `trees`, each open tree's identity beside the tree the binding gives it,
    /// laid through `lay` with each closed subtree of those by its digest.
    pub fn with_trees(
        self,
        lay: impl Lays<'a>,
        scratch: BumpAllocator<'_>,
        trees: &[(Identity, &'a Tree<'a>)],
    ) -> ClosedTypes<'a> {
        let mut entries = BumpVec::with_capacity_in(trees.len(), scratch);
        entries.extend(trees.iter().copied());
        let mut pending: BumpVec<'_, &'a Tree<'a>> = BumpVec::new_in(scratch);
        pending.extend(trees.iter().map(|(_, tree)| *tree));
        while let Some(tree) = pending.pop() {
            if let Identity::Closed(_) = tree.identity() {
                entries.push((tree.identity(), tree));
            }
            tree.subtrees(&mut |subtree| pending.push(subtree));
        }
        entries.sort_unstable_by_key(|(identity, _)| *identity);
        entries.dedup_by_key(|(identity, _)| *identity);
        ClosedTypes {
            trees: collect(lay, entries.iter().copied()),
            ..self
        }
    }

    /// Whether the binding gives some open tree a tree: what only a walk substitutes.
    pub fn substitutes_trees(&self) -> bool {
        (self.trees.iter()).any(|(identity, _)| matches!(identity, Identity::Open(_)))
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.trees.is_empty()
    }

    /// Its keys, sorted by digest: what a binding over the same records closes again.
    pub fn keys(&self) -> &'a [DeclaredType<Parametric>] {
        self.keys
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Each key beside its value.
    pub fn entries(&self) -> impl Iterator<Item = (DeclaredType<Parametric>, Closed)> {
        self.keys.iter().copied().zip(self.values.iter().copied())
    }

    /// Hand `each` every type the table names, keys and values both.
    pub fn each_type(&self, each: &mut dyn FnMut(DeclaredType<Parametric>)) {
        for (key, value) in self.entries() {
            each(key);
            match value {
                Closed::Type(DeclaredType::Type(closed)) => each(DeclaredType::Type(closed.into())),
                Closed::Type(DeclaredType::Scheme(scheme)) => each(DeclaredType::Scheme(scheme)),
                Closed::Partial(partial) => each(partial),
                Closed::Same => {}
            }
        }
    }

    /// This table laid again through `lay`, each tree through `tree`.
    pub fn copied<'to>(
        &self,
        lay: impl Lays<'to>,
        mut tree: impl FnMut(&'a Tree<'a>) -> &'to Tree<'to>,
    ) -> ClosedTypes<'to> {
        ClosedTypes {
            keys: collect(lay, self.keys.iter().copied()),
            values: collect(lay, self.values.iter().copied()),
            trees: collect(
                lay,
                (self.trees.iter()).map(|(identity, held)| (*identity, tree(held))),
            ),
        }
    }
}

#[cfg(test)]
thread_local! {
    /// How many closed types tables have laid: the report's count. Tests reset it.
    pub(crate) static CLOSED_LAID: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The table over `keys` under one binding, laid through `lay`: each key `close` closes, whole
/// or in part, sorted by digest and held once. A key `close` leaves as it is is dropped. The one
/// door a parametric layer reuses to lay values for another binding.
pub fn closed_under<'a>(
    lay: impl Lays<'a>,
    scratch: BumpAllocator<'_>,
    keys: &[DeclaredType<Parametric>],
    close: &mut dyn FnMut(DeclaredType<Parametric>) -> Closed,
) -> ClosedTypes<'a> {
    let mut entries = BumpVec::with_capacity_in(keys.len(), scratch);
    entries.extend(keys.iter().filter_map(|key| match close(*key) {
        Closed::Same => None,
        value => Some((*key, value)),
    }));
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
        trees: &[],
    }
}
