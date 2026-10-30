//! `join` and the meet — the least upper bound and the greatest lower bound — over nodes.
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

use super::chain::TypeChain;
use super::handle::Handle;
use super::node::TypeNode;
use super::order::{code_kind_of, code_needs, is_subtype_of};
use super::signatures::is_signature_type;
use super::stored::{Content, Linked};
use super::tiers::Habitat;
use super::walk::Variance;
use super::walk::binary::{Arm, Lockstep, Width, lockstep};

/// The least upper bound: the larger operand when the two are ordered, otherwise their union. A
/// parametric operand is related by no order, so it joins to the union, which keeps a variable
/// beside every concrete member.
pub(super) fn join<'run>(
    types: &TypeChain<'run, impl Habitat<'run>>,
    scratch: BumpAllocator<'_>,
    a: Linked<'run>,
    b: Linked<'run>,
) -> Linked<'run> {
    if !a.is_concrete() || !b.is_concrete() {
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
pub(super) fn join_iter<'run, I: IntoIterator<Item = Linked<'run>>>(
    types: &TypeChain<'run, impl Habitat<'run>>,
    scratch: BumpAllocator<'_>,
    iter: I,
) -> Linked<'run> {
    iter.into_iter()
        .reduce(|a, b| join(types, scratch, a, b))
        .unwrap_or_else(|| types.never())
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
pub(super) fn meet_through_variables<'run>(
    types: &TypeChain<'run, impl Habitat<'run>>,
    scratch: BumpAllocator<'_>,
    a: Linked<'run>,
    b: Linked<'run>,
) -> Linked<'run> {
    lockstep(types, scratch, a, b, Variance::Co, &mut Meet)
}

/// The rebuilding [`Lockstep`] instance. Every pair it reaches structurally is at
/// [`Variance::Co`]: a contravariant position under a meet is a join, which [`enter`] answers
/// outright rather than descending into.
///
/// [`enter`]: Lockstep::enter
struct Meet;

impl<'run> Lockstep<'run> for Meet {
    type Out = Linked<'run>;

    fn enter(
        &mut self,
        types: &TypeChain<'run, impl Habitat<'run>>,
        scratch: BumpAllocator<'_>,
        a: Linked<'run>,
        b: Linked<'run>,
        v: Variance,
    ) -> Option<Linked<'run>> {
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
        let (na, nb) = (a.view(), b.view());
        if !matches!(na, TypeNode::Union { .. }) && !matches!(nb, TypeNode::Union { .. }) {
            return None;
        }
        let mut met = BumpVec::new_in(scratch);
        if let TypeNode::Union { members } = na {
            met.extend(
                members
                    .iter()
                    .map(|x| meet_through_variables(types, scratch, x, b)),
            );
        }
        if let TypeNode::Union { members } = nb {
            met.extend(
                members
                    .iter()
                    .map(|y| meet_through_variables(types, scratch, a, y)),
            );
        }
        Some(types.union_of(scratch, &met))
    }

    fn leaf(
        &mut self,
        types: &TypeChain<'run, impl Habitat<'run>>,
        scratch: BumpAllocator<'_>,
        a: Linked<'run>,
        b: Linked<'run>,
        _v: Variance,
    ) -> Linked<'run> {
        // Two signature types meet at the set of both operands' applications, which is exact and
        // never `Never`.
        if is_signature_type(a) && is_signature_type(b) {
            return types.signature_meet(scratch, &[a, b]);
        }
        // Two code kinds, one needing names, meet at the kinds' meet needing the names both need.
        // Two bare kinds unordered in the code tree have no meet.
        // An unbuilt kind meets a code type at the kinds' meet, needing what the other side needs:
        // unbuilt again when both are.
        let unbuilt = |kt: Linked<'run>| matches!(kt.content(), Content::CodeUnbuilt { .. });
        if unbuilt(a) || unbuilt(b) {
            let (Some(x), Some(y)) = (code_kind_of(a), code_kind_of(b)) else {
                return types.never();
            };
            let kind = meet_through_variables(types, scratch, x, y);
            if kind == Handle::NEVER {
                return types.never();
            }
            return match (unbuilt(a), unbuilt(b)) {
                (true, true) => types.code_unbuilt(kind),
                (true, false) => {
                    types.code_needing(scratch, kind, code_needs(b).map_or(&[], |n| n.1))
                }
                _ => types.code_needing(scratch, kind, code_needs(a).map_or(&[], |n| n.1)),
            };
        }
        let needing = |kt: Linked<'run>| matches!(kt.content(), Content::CodeNeeding { .. });
        match (code_needs(a), code_needs(b)) {
            (Some((x, xs)), Some((y, ys))) if needing(a) || needing(b) => {
                let kind = meet_through_variables(types, scratch, x, y);
                if kind == Handle::NEVER {
                    return types.never();
                }
                let mut both = BumpVec::new_in(scratch);
                both.extend(xs.iter().filter(|name| ys.contains(name)).copied());
                types.code_needing(scratch, kind, &both)
            }
            _ => types.never(),
        }
    }

    fn set_wise(
        &mut self,
        _types: &TypeChain<'run, impl Habitat<'run>>,
        _scratch: BumpAllocator<'_>,
        _a: &[Linked<'run>],
        _b: &[Linked<'run>],
        _v: Variance,
        _recurse: &mut dyn FnMut(&mut Self, Linked<'run>, Linked<'run>, Variance) -> Linked<'run>,
    ) -> Linked<'run> {
        // `enter` answers every pair with a union on either side.
        unreachable!("Meet::enter distributes every union")
    }

    fn structural<H: Habitat<'run>>(
        &mut self,
        types: &TypeChain<'run, H>,
        scratch: BumpAllocator<'_>,
        paired: &[Linked<'run>],
        arm: Arm<'_, 'run, H>,
    ) -> Linked<'run> {
        let leftovers = !(arm.only_a.is_empty() && arm.only_b.is_empty());
        if arm.width == Width::Exact && leftovers {
            // Two applications naming different parameters have no common application below them.
            return types.never();
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
