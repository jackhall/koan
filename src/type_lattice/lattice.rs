//! `join` and `meet` — the least upper bound and the greatest lower bound over the whole node
//! vocabulary.
//!
//! `join` is not a walk. It is the larger operand when the two are ordered and their canonical
//! union otherwise, which is what makes it associative: a structural join stops being associative
//! the moment `a ≤ a | b`. `meet` is the binary driver's rebuilding instance, and is total — a pair
//! with no common refinement meets at `Never`, which is always a sound lower bound.
//!
//! The four laws — commutativity, associativity, idempotence and absorption — hold over every node
//! kind, and that is what fixes both operations.

use crate::memory::{BumpAllocator, BumpVec};

use super::handle::KType;
use super::node::TypeNode;
use super::order::is_subtype_of;
use super::registry::TypeRegistry;
use super::sig_relations::meet_schemas;
use super::walk::Variance;
use super::walk::binary::{Arm, Lockstep, Width, lockstep};

/// The least upper bound: the larger operand when the two are ordered, otherwise their union.
pub fn join(types: &TypeRegistry<'_>, scratch: BumpAllocator<'_>, a: KType, b: KType) -> KType {
    if is_subtype_of(types, scratch, a, b) {
        return b;
    }
    if is_subtype_of(types, scratch, b, a) {
        return a;
    }
    types.union_of(scratch, &[a, b])
}

/// Reduce an iterator of types to their least upper bound. An empty iterator is `Never`, the join's
/// identity element: an empty container carries the bottom element type, which every typed element
/// slot admits and which absorbs the first element joined against it.
pub fn join_iter<I: IntoIterator<Item = KType>>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    iter: I,
) -> KType {
    iter.into_iter()
        .reduce(|a, b| join(types, scratch, a, b))
        .unwrap_or(KType::NEVER)
}

/// The greatest lower bound.
///
/// Pointwise through lists, dicts and constructor arguments; the union of both field sets with
/// shared fields met for records; the intersection of parameter names with shared parameters joined
/// and returns met for functions; distribution through a union; [`meet_schemas`] for two
/// signatures; `Never` where no common shape exists.
pub fn meet(types: &TypeRegistry<'_>, scratch: BumpAllocator<'_>, a: KType, b: KType) -> KType {
    lockstep(types, scratch, a, b, Variance::Co, &mut Meet)
}

/// The rebuilding [`Lockstep`] instance. Every pair it reaches structurally is at
/// [`Variance::Co`]: a contravariant position under a meet is a join, which [`enter`] answers
/// outright rather than descending into.
///
/// [`enter`]: Lockstep::enter
struct Meet;

impl Lockstep for Meet {
    type Out = KType;

    fn enter(
        &mut self,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        a: KType,
        b: KType,
        v: Variance,
    ) -> Option<KType> {
        if v == Variance::Contra {
            return Some(join(types, scratch, a, b));
        }
        if a == b {
            return Some(a);
        }
        if is_subtype_of(types, scratch, a, b) {
            return Some(a);
        }
        if is_subtype_of(types, scratch, b, a) {
            return Some(b);
        }
        None
    }

    fn leaf(
        &mut self,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        a: KType,
        b: KType,
        _v: Variance,
    ) -> KType {
        // Two interfaces meet at their strongest common refinement; a conflict — two manifest
        // bindings for one name, two modes for one operator run — is the absence of a meet.
        match (types.node(a), types.node(b)) {
            (TypeNode::Signature { schema: x, .. }, TypeNode::Signature { schema: y, .. }) => {
                meet_schemas(types, scratch, x, y).unwrap_or(KType::NEVER)
            }
            _ => KType::NEVER,
        }
    }

    fn set_wise(
        &mut self,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        a: &[KType],
        b: &[KType],
        v: Variance,
        recurse: &mut dyn FnMut(&mut Self, KType, KType, Variance) -> KType,
    ) -> KType {
        // A value in both operands is in some member of each, so the meet is the union of the
        // per-pair meets. Pairs that meet at `Never` contribute nothing, and `union_of` drops them.
        let mut met = BumpVec::with_capacity_in(a.len() * b.len(), scratch);
        for x in a {
            for y in b {
                met.push(recurse(self, *x, *y, v));
            }
        }
        types.union_of(scratch, &met)
    }

    fn structural(
        &mut self,
        _types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        paired: &[KType],
        arm: Arm<'_, '_>,
    ) -> KType {
        let leftovers = !(arm.only_a.is_empty() && arm.only_b.is_empty());
        if arm.width == Width::Exact && leftovers {
            // Two applications naming different parameters have no common application below them.
            return KType::NEVER;
        }
        if arm.width.bound_keeps_leftovers(false) {
            let mut extra = BumpVec::with_capacity_in(arm.only_a.len() + arm.only_b.len(), scratch);
            extra.extend_from_slice(arm.only_a);
            extra.extend_from_slice(arm.only_b);
            return arm.rebuild.compose(paired, &extra);
        }
        arm.rebuild.compose(paired, &[])
    }
}
