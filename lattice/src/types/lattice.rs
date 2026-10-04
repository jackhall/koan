//! `join` and the meet — the least upper bound and the greatest lower bound — over raw handles.
//!
//! `join` is not a walk. It is the larger operand when the two are ordered and their canonical
//! union otherwise, which is what makes it associative: a structural join stops being associative
//! the moment `a ≤ a | b`. A parametric operand is ordered with nothing, so it joins to the union.
//! The meet is the binary driver's rebuilding instance, and is total — a pair with no common
//! refinement meets at `Never`, which is always a sound lower bound. Over a variable it relates by
//! the rigid rule, which the solver's meet needs ([`meet_through_variables`]); the public meet
//! takes concrete types ([`typed`](super::typed)).
//!
//! The four laws — commutativity, associativity, idempotence and absorption — hold over every
//! concrete type, and that is what fixes both operations.

use crate::bump::{BumpAllocator, BumpVec};

use super::handle::{Handle, TypeHandle, wrap};
use super::node::TypeNode;
use super::order::{code_needs, is_subtype_of};
use super::registry::TypeRegistry;
use super::signatures::is_signature_type;
use super::walk::Variance;
use super::walk::binary::{Arm, Lockstep, Width, lockstep};

/// The least upper bound: the larger operand when the two are ordered, otherwise their union. A
/// parametric operand is related by no order, so it joins to the union, which keeps a variable
/// beside every concrete member.
pub(super) fn join(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    a: Handle,
    b: Handle,
) -> Handle {
    if !types.is_concrete(a) || !types.is_concrete(b) {
        return types.union_of(scratch, &[a, b]);
    }
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
pub(super) fn join_iter<I: IntoIterator<Item = Handle>>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    iter: I,
) -> Handle {
    iter.into_iter()
        .reduce(|a, b| join(types, scratch, a, b))
        .unwrap_or(Handle::NEVER)
}

/// The greatest lower bound, relating a variable by the rigid rule — the solver's meet, which a
/// variable's least instance over upper contributions and *fits*' pooled value slots read. Over
/// concrete operands it is the lattice's meet.
///
/// Pointwise through lists, dicts and constructor arguments; the union of both field sets with
/// shared fields met for records; the intersection of parameter names with shared parameters joined
/// and returns met for functions; distribution through a union; the set of both operands'
/// applications for two signature types; the kinds' meet needing the names both need for two code
/// kinds; `Never` where no common shape exists.
pub(super) fn meet_through_variables(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    a: Handle,
    b: Handle,
) -> Handle {
    lockstep(types, scratch, a, b, Variance::Co, &mut Meet)
}

/// The rebuilding [`Lockstep`] instance. Every pair it reaches structurally is at
/// [`Variance::Co`]: a contravariant position under a meet is a join, which [`enter`] answers
/// outright rather than descending into.
///
/// [`enter`]: Lockstep::enter
struct Meet;

impl Lockstep for Meet {
    type Out = Handle;

    fn enter(
        &mut self,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        a: Handle,
        b: Handle,
        v: Variance,
    ) -> Option<Handle> {
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
        // A union meets the other side member by member, each member against the other side
        // *whole*: pairing members one at a time would lose a variable whose bound spans several
        // of them. With a union on both sides both distributions are taken, since either may hold
        // such a variable. Members that meet at `Never` contribute nothing, and `union_of` drops them.
        let (na, nb) = (types.node(a), types.node(b));
        if !matches!(na, TypeNode::Union { .. }) && !matches!(nb, TypeNode::Union { .. }) {
            return None;
        }
        let mut met = BumpVec::new_in(scratch);
        if let TypeNode::Union { members } = na {
            met.extend(
                members
                    .iter()
                    .map(|x| meet_through_variables(types, scratch, *x, b)),
            );
        }
        if let TypeNode::Union { members } = nb {
            met.extend(
                members
                    .iter()
                    .map(|y| meet_through_variables(types, scratch, a, *y)),
            );
        }
        Some(types.union_of(scratch, &met))
    }

    fn leaf(
        &mut self,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        a: Handle,
        b: Handle,
        _v: Variance,
    ) -> Handle {
        // Two signature types meet at the set of both operands' applications, which is exact and
        // never `Never`.
        if is_signature_type(types, a) && is_signature_type(types, b) {
            return types.signature_meet(scratch, &[a, b]);
        }
        // Two code kinds, one needing names, meet at the kinds' meet needing the names both need.
        // Two bare kinds unordered in the code tree have no meet.
        let needing = |kt| matches!(types.node(kt), TypeNode::CodeNeeding { .. });
        match (code_needs(types, a), code_needs(types, b)) {
            (Some((x, xs)), Some((y, ys))) if needing(a) || needing(b) => {
                let kind = meet_through_variables(types, scratch, x.raw(), y.raw());
                if kind == Handle::NEVER {
                    return Handle::NEVER;
                }
                let mut both = BumpVec::new_in(scratch);
                both.extend(xs.iter().filter(|name| ys.contains(name)).copied());
                types.code_needing(scratch, wrap(kind), &both).raw()
            }
            _ => Handle::NEVER,
        }
    }

    fn set_wise(
        &mut self,
        _types: &TypeRegistry<'_>,
        _scratch: BumpAllocator<'_>,
        _a: &[Handle],
        _b: &[Handle],
        _v: Variance,
        _recurse: &mut dyn FnMut(&mut Self, Handle, Handle, Variance) -> Handle,
    ) -> Handle {
        // `enter` answers every pair with a union on either side.
        unreachable!("Meet::enter distributes every union")
    }

    fn structural(
        &mut self,
        _types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        paired: &[Handle],
        arm: Arm<'_, '_>,
    ) -> Handle {
        let leftovers = !(arm.only_a.is_empty() && arm.only_b.is_empty());
        if arm.width == Width::Exact && leftovers {
            // Two applications naming different parameters have no common application below them.
            return Handle::NEVER;
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
