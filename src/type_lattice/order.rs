//! `is_subtype_of` — the one order, and the two readings written over it.
//!
//! One reflexive partial order, memoized through the registry's verdict edges. There are no
//! tie-break tiers: nothing ranks a token leaf against `Str`, a nominal slot against a kind slot,
//! or a constrained slot against an unconstrained one. Those are not subtype facts, and a dispatch
//! that needs to choose between two unrelated slot types has an ambiguity, not a verdict.
//!
//! **The guard set is here and only here.** Every relation that reads the order — the three
//! `slot_*` compositions, the unifier's leaf, the specificity verdict, the signature relation's
//! value-slot rule — reaches it through [`is_subtype_of`], so there is no second descent to keep in
//! step with this one.

use crate::memory::{BumpAllocator, BumpVec};

use super::handle::KType;
use super::node::TypeNode;
use super::registry::{Relation, TypeRegistry};
use super::sig_relations::{Returns, admits_shape, sig_subtype};
use super::walk::Variance;
use super::walk::binary::{Arm, Lockstep, lockstep};

/// Whether `a` is below `b` in the one order.
///
/// - `Never` is the bottom and `Any` the top.
/// - `OfKind(x) ≤ OfKind(y)` when `y` admits `x`; the kind lattice is the whole story on the type
///   channel.
/// - Lists, dicts and constructor applications are covariant in every child. Records are covariant
///   and width-superset. Functions are contravariant in their parameters and width-subset there,
///   and covariant in the return. A monomorphic expression shape pairs positionally under equal
///   keywords, contravariant in its slots and covariant in its return.
/// - A union is below `b` when every member is; a non-union is below a union when it is below some
///   member.
/// - A signature is below another when [`sig_subtype`] accepts the pair.
/// - A **rigid variable** — `Quantified` or `AbstractType` — is a nominal identity over its bound:
///   below it are only itself and `Never`, above it only itself and everything above its bound.
///   The two clauses agree because a bound is a variable-free type, so nothing above a bound is
///   itself rigid.
/// - A quantified shape is below another shape when some instantiation of its variables, each under
///   its bound, puts the instance below the other with the other's variables rigid.
/// - A pre-seal `Sibling` and a sealed member are atoms, with the same profile as each other.
/// - Every other pair is unrelated.
pub fn is_subtype_of(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    a: KType,
    b: KType,
) -> bool {
    if a == b || b == KType::ANY || a == KType::NEVER {
        return true;
    }
    if a == KType::ANY || b == KType::NEVER {
        return false;
    }
    if let Some(known) = types.verdict(a.digest(), b.digest(), Relation::Subtype) {
        return known;
    }
    let verdict = lockstep(
        types,
        scratch,
        a,
        b,
        Variance::Co,
        &mut Order { root: true },
    );
    types.record_verdict(a.digest(), b.digest(), Relation::Subtype, verdict);
    verdict
}

/// The strict order: unequal handles and a subtype.
pub fn is_more_specific_than(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    a: KType,
    b: KType,
) -> bool {
    a != b && is_subtype_of(types, scratch, a, b)
}

/// Whether the type a position carries fills the slot declared there — the order, read from the
/// slot's side.
pub fn satisfied_by(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    slot: KType,
    carried: KType,
) -> bool {
    is_subtype_of(types, scratch, carried, slot)
}

/// The order as a [`Lockstep`] instance. `root` is what routes every nested pair back through
/// [`is_subtype_of`], so each one is memoized rather than only the pair the caller asked about.
struct Order {
    root: bool,
}

impl Lockstep for Order {
    type Out = bool;

    fn enter(
        &mut self,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        a: KType,
        b: KType,
        v: Variance,
    ) -> Option<bool> {
        if !self.root {
            // A contravariant position asks the reverse question, which is the same relation with
            // the operands swapped.
            return Some(match v {
                Variance::Co => is_subtype_of(types, scratch, a, b),
                Variance::Contra => is_subtype_of(types, scratch, b, a),
            });
        }
        self.root = false;
        None
    }

    fn leaf(
        &mut self,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        a: KType,
        b: KType,
        _v: Variance,
    ) -> bool {
        let (na, nb) = (types.node(a), types.node(b));
        // A rigid variable's down-set is checked first: below one are only itself — which the
        // caller's equality guard already answered — and `Never`.
        if is_rigid(&nb) {
            return false;
        }
        match (na, nb) {
            (TypeNode::OfKind(x), TypeNode::OfKind(y)) => y.admits(x),
            (TypeNode::Signature { schema: sub, .. }, TypeNode::Signature { schema: sup, .. }) => {
                if let Some(known) = types.verdict(a.digest(), b.digest(), Relation::SigSatisfies) {
                    return known;
                }
                let verdict = sig_subtype(types, scratch, sub, sup).is_ok();
                types.record_verdict(a.digest(), b.digest(), Relation::SigSatisfies, verdict);
                verdict
            }
            // Above a rigid variable is everything above its bound.
            (TypeNode::Quantified { bound, .. }, _) | (TypeNode::AbstractType { bound, .. }, _) => {
                is_subtype_of(types, scratch, bound, b)
            }
            // A quantified shape is below another when some instantiation of its group puts every
            // slot and the return under the other's, with the other's rigid.
            (TypeNode::ExpressionShape { .. }, TypeNode::ExpressionShape { .. }) => {
                admits_shape(types, scratch, a, b, Returns::Checked)
            }
            _ => false,
        }
    }

    fn set_wise(
        &mut self,
        _types: &TypeRegistry<'_>,
        _scratch: BumpAllocator<'_>,
        a: &[KType],
        b: &[KType],
        v: Variance,
        recurse: &mut dyn FnMut(&mut Self, KType, KType, Variance) -> bool,
    ) -> bool {
        // A union is below `b` when every member is; a non-union is below a union when it is below
        // some member. One rule covers both, since a non-union side arrives as a one-element slice.
        a.iter().all(|x| b.iter().any(|y| recurse(self, *x, *y, v)))
    }

    fn structural(
        &mut self,
        _types: &TypeRegistry<'_>,
        _scratch: BumpAllocator<'_>,
        paired: &[bool],
        arm: Arm<'_, '_>,
    ) -> bool {
        paired.iter().all(|verdict| *verdict) && arm.width.permits(arm.only_a, arm.only_b)
    }

    fn short_circuits(&self, out: &bool) -> bool {
        !*out
    }
}

fn is_rigid(node: &TypeNode<'_>) -> bool {
    matches!(
        node,
        TypeNode::Quantified { .. } | TypeNode::AbstractType { .. }
    )
}

/// Which member of an ordered pair a canonicalization drops.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Dropped {
    /// The lower one — a union's rule: a member below another admits nothing the other does not.
    Below,
    /// The upper one — an overload set's rule: a member above another promises nothing the lower
    /// one does not, since whatever satisfies the lower satisfies it too.
    Above,
}

/// One flag per member: `false` where the member lies on the `dropped` side of some *other*
/// member, so the survivors form an antichain — the subsumption rule a union and an overload set
/// canonicalize by, each from its own side. Two mutually ordered members are one handle, so a
/// caller dedups first and no pair drops both sides.
pub(super) fn unsubsumed<'s>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'s>,
    members: &[KType],
    dropped: Dropped,
) -> BumpVec<'s, bool> {
    let mut keep = BumpVec::with_capacity_in(members.len(), scratch);
    keep.extend(members.iter().map(|member| {
        !members.iter().any(|peer| {
            peer != member
                && match dropped {
                    Dropped::Below => is_subtype_of(types, scratch, *member, *peer),
                    Dropped::Above => is_subtype_of(types, scratch, *peer, *member),
                }
        })
    }));
    keep
}

/// The index of the one element of `0..count` that `dominates` every other, if there is one — the
/// tournament a most-specific overload, a keyworded selection and a contribution set's extremum
/// all run.
pub(super) fn dominant(
    count: usize,
    mut dominates: impl FnMut(usize, usize) -> bool,
) -> Option<usize> {
    (0..count).find(|&i| (0..count).all(|j| i == j || dominates(i, j)))
}
