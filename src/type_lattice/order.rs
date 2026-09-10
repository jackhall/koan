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

use super::handle::KType;
use super::node::TypeNode;
use super::registry::{Relation, TypeRegistry};
use super::schema::{shape_quantifiers, shape_return, shape_slots};
use super::sig_relations::sig_subtype;
use super::unify::{Collector, admits_with};
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
pub fn is_subtype_of(types: &TypeRegistry, a: KType, b: KType) -> bool {
    if a == b || b == KType::ANY || a == KType::NEVER {
        return true;
    }
    if a == KType::ANY || b == KType::NEVER {
        return false;
    }
    if let Some(known) = types.verdict(a.digest(), b.digest(), Relation::Subtype) {
        return known;
    }
    let verdict = lockstep(types, a, b, Variance::Co, &mut Order { root: true });
    types.record_verdict(a.digest(), b.digest(), Relation::Subtype, verdict);
    verdict
}

/// The strict order: unequal handles and a subtype.
pub fn is_more_specific_than(types: &TypeRegistry, a: KType, b: KType) -> bool {
    a != b && is_subtype_of(types, a, b)
}

/// Whether the type a position carries fills the slot declared there — the order, read from the
/// slot's side.
pub fn satisfied_by(types: &TypeRegistry, slot: KType, carried: KType) -> bool {
    is_subtype_of(types, carried, slot)
}

/// The order as a [`Lockstep`] instance. `root` is what routes every nested pair back through
/// [`is_subtype_of`], so each one is memoized rather than only the pair the caller asked about.
struct Order {
    root: bool,
}

impl Lockstep for Order {
    type Out = bool;

    fn enter(&mut self, types: &TypeRegistry, a: KType, b: KType, v: Variance) -> Option<bool> {
        if !self.root {
            // A contravariant position asks the reverse question, which is the same relation with
            // the operands swapped.
            return Some(match v {
                Variance::Co => is_subtype_of(types, a, b),
                Variance::Contra => is_subtype_of(types, b, a),
            });
        }
        self.root = false;
        None
    }

    fn leaf(&mut self, types: &TypeRegistry, a: KType, b: KType, _v: Variance) -> bool {
        types.with_node(a, |na| {
            types.with_node(b, |nb| {
                // A rigid variable's down-set is checked first: below one are only itself — which
                // the caller's equality guard already answered — and `Never`.
                if is_rigid(nb) {
                    return false;
                }
                match (na, nb) {
                    (TypeNode::OfKind(x), TypeNode::OfKind(y)) => y.admits(*x),
                    (
                        TypeNode::Signature { schema: sub, .. },
                        TypeNode::Signature { schema: sup, .. },
                    ) => {
                        if let Some(known) =
                            types.verdict(a.digest(), b.digest(), Relation::SigSatisfies)
                        {
                            return known;
                        }
                        let verdict = sig_subtype(types, sub, sup).is_ok();
                        types.record_verdict(
                            a.digest(),
                            b.digest(),
                            Relation::SigSatisfies,
                            verdict,
                        );
                        verdict
                    }
                    // Above a rigid variable is everything above its bound.
                    (TypeNode::Quantified { bound, .. }, _)
                    | (TypeNode::AbstractType { bound, .. }, _) => is_subtype_of(types, *bound, b),
                    (TypeNode::ExpressionShape { .. }, TypeNode::ExpressionShape { .. }) => {
                        instantiates_below(types, a, b)
                    }
                    _ => false,
                }
            })
        })
    }

    fn set_wise(
        &mut self,
        _types: &TypeRegistry,
        a: &[KType],
        b: &[KType],
        v: Variance,
        recurse: &mut dyn FnMut(&mut Self, KType, KType, Variance) -> bool,
    ) -> bool {
        // A union is below `b` when every member is; a non-union is below a union when it is below
        // some member. One rule covers both, since a non-union side arrives as a one-element slice.
        a.iter().all(|x| b.iter().any(|y| recurse(self, *x, *y, v)))
    }

    fn structural(&mut self, _types: &TypeRegistry, paired: &[bool], arm: Arm<'_>) -> bool {
        paired.iter().all(|verdict| *verdict) && arm.width.permits(arm.only_a, arm.only_b)
    }

    fn short_circuits(&self, out: &bool) -> bool {
        !*out
    }
}

fn is_rigid(node: &TypeNode) -> bool {
    matches!(
        node,
        TypeNode::Quantified { .. } | TypeNode::AbstractType { .. }
    )
}

/// Whether some instantiation of `a`'s quantifier group, each variable under its bound, puts the
/// instance below `b` with `b`'s variables rigid.
///
/// Prenex instantiation through the collector: each slot pair asks `b`'s slot to lie under `a`'s
/// (covariant for the collector, since a slot's own polarity is contravariant), the return pair
/// asks `a`'s return to lie under `b`'s, and `solve` decides. `b`'s `Quantified` nodes fall to the
/// rigid rule automatically, because the collector only ever solves declared-side variables and the
/// carried side is never substituted.
fn instantiates_below(types: &TypeRegistry, a: KType, b: KType) -> bool {
    let (a_slots, b_slots) = (shape_slots(a, types), shape_slots(b, types));
    if a_slots.len() != b_slots.len() || !keywords_agree(types, a, b) {
        return false;
    }
    let (Some(a_ret), Some(b_ret)) = (shape_return(a, types), shape_return(b, types)) else {
        return false;
    };
    let mut collector = Collector::new(shape_quantifiers(a, types).len());
    for (declared, carried) in a_slots.iter().zip(b_slots.iter()) {
        if admits_with(types, *declared, *carried, Variance::Co, &mut collector).is_err() {
            return false;
        }
    }
    if admits_with(types, a_ret, b_ret, Variance::Contra, &mut collector).is_err() {
        return false;
    }
    collector.solve(types).is_ok()
}

/// Whether two shapes key the same bucket: equal element runs, keyword for keyword, position for
/// position.
fn keywords_agree(types: &TypeRegistry, a: KType, b: KType) -> bool {
    super::schema::shape_keys_equal(a, b, types)
}
