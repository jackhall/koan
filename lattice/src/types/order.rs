//! `is_subtype_of` — the one order — and *fits*, the relation a question reads, over raw handles.
//!
//! Both are reflexive, memoized through the registry's verdict edges under their own
//! [`Relation`], and share one descent, [`Order`], which differs between them at its leaf alone.
//! The **order** never solves: two signature types compare by their applications' pins (R-5). It
//! is what every construction reads — a join, a meet, a union's or an overload set's subsumption —
//! and it relates concrete types: its public door ([`typed`](super::typed)) takes `KType`s, so no
//! rigid variable reaches the rigid clause below through it. ***Fits*** solves: a quantified binder
//! fits another when some instantiation of its group puts the instance under the other, and a
//! module's signature fits a declared one when its members do. It contains the order, relates a
//! variable by the rigid rule, and it is what every question reads — whether a value fills a slot,
//! a return lies within its contract, a bound holds.
//!
//! There are no tie-break tiers: nothing ranks a token leaf against `Str`, a nominal slot against
//! a kind slot, or a constrained slot against an unconstrained one. Those are not subtype facts,
//! and a dispatch that needs to choose between two unrelated slot types has an ambiguity, not a
//! verdict.

use crate::bump::{BumpAllocator, BumpVec};
use crate::symbols::BinderSymbol;

use super::handle::{Handle, KType, TypeHandle, wrap};
use super::node::TypeNode;
use super::registry::TypeRegistry;
use super::sig_relations::{admits_function, admits_shape, sig_fits};
use super::signatures::{applications, applications_under, is_signature_type};
use super::verdicts::Relation;
use super::walk::Variance;
use super::walk::binary::{Arm, Lockstep, lockstep};

/// Whether `a` is below `b` in the one order.
///
/// - `Never` is the bottom and `Any` the top, above the three family tops.
/// - A family top — `Value` or `Code` — is above every type whose own family it is
///   ([`family_top`]); `Type` is the kind order's top.
/// - `OfKind(x) ≤ OfKind(y)` when `y` admits `x`; the kind lattice is the whole story on the type
///   channel.
/// - Lists, dicts and constructor applications are covariant in every child. An application lies
///   under the family it applies, and not the reverse. Records are covariant
///   and width-superset. Functions are contravariant in their parameters and width-subset there,
///   and covariant in the return. A monomorphic expression shape pairs positionally under equal
///   keywords, contravariant in its slots and covariant in its return.
/// - A union is below `b` when every member is; a non-union is below a union when it is below some
///   member.
/// - A signature type is below another when each application of the other lies above one of its
///   own: the same signature, pinning at least the other's pins, each at an equal type.
/// - A **rigid variable** — `Quantified`, `Lexical` or `Parameter` — is a nominal identity
///   between its lower end (`Never` but for a lexical variable's) and its bound: below it lie
///   itself and whatever lies under its lower end, above it itself and everything above its bound,
///   a union included. The clauses agree because both ends are variable-free types.
/// - A quantified shape or function type lies under only itself.
/// - A pre-seal `Sibling` and a sealed member are atoms, with the same profile as each other. An
///   opaque carrier is an atom under no family top: it reveals no bound outside its view, so above
///   it lie only itself, a union holding it and `Any`, and below it only itself and `Never`.
/// - Every other pair is unrelated.
pub(super) fn is_subtype_of(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    a: Handle,
    b: Handle,
) -> bool {
    related(types, scratch, a, b, Relation::Subtype)
}

/// Whether `a` fits `b`: the order, but for two clauses that solve. A quantified shape fits another
/// shape, and a quantified function another function, when some instantiation of its variables,
/// each under its bound, puts the instance under the other with the other's variables rigid. A
/// signature type fits another when [`sig_fits`] accepts the pair. Everything the order relates
/// fits.
pub(super) fn fits(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    a: Handle,
    b: Handle,
) -> bool {
    related(types, scratch, a, b, Relation::Fits)
}

/// Whether the type a position carries fills the slot declared there — *fits*, read from the
/// slot's side.
pub(super) fn satisfied_by(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    slot: Handle,
    carried: Handle,
) -> bool {
    fits(types, scratch, carried, slot)
}

/// `relation` — the order or *fits* — over `a` and `b`, through the verdict cache.
fn related(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    a: Handle,
    b: Handle,
    relation: Relation,
) -> bool {
    if a == b || b == Handle::ANY || a == Handle::NEVER {
        return true;
    }
    if a == Handle::ANY || b == Handle::NEVER {
        return false;
    }
    if let Some(known) = types.verdict(a.digest(), b.digest(), relation) {
        return known;
    }
    let verdict = lockstep(
        types,
        scratch,
        a,
        b,
        Variance::Co,
        &mut Order {
            root: true,
            relation,
        },
    );
    types.record_verdict(a.digest(), b.digest(), relation, verdict);
    verdict
}

/// The order or *fits* as a [`Lockstep`] instance. `root` is what routes every nested pair back
/// through [`related`] under the same `relation`, so each one is memoized rather than only the
/// pair the caller asked about.
struct Order {
    root: bool,
    relation: Relation,
}

impl Lockstep for Order {
    type Out = bool;

    fn enter(
        &mut self,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        a: Handle,
        b: Handle,
        v: Variance,
    ) -> Option<bool> {
        if !self.root {
            // A contravariant position asks the reverse question, which is the same relation with
            // the operands swapped.
            return Some(match v {
                Variance::Co => related(types, scratch, a, b, self.relation),
                Variance::Contra => related(types, scratch, b, a, self.relation),
            });
        }
        self.root = false;
        let na = types.node(a);
        // Above a rigid variable is itself and everything above its bound — a union included, which
        // a member-by-member reading would miss when the bound spans several members.
        if let Some(bound) = na.rigid_bound() {
            return Some(
                matches!(types.node(b), TypeNode::Union { members } if members.contains(&a))
                    || related(types, scratch, bound.raw(), b, self.relation),
            );
        }
        // A union is below `b` when every member is below `b` whole, for the same reason.
        match na {
            TypeNode::Union { members } => Some(
                members
                    .iter()
                    .all(|x| related(types, scratch, *x, b, self.relation)),
            ),
            _ => None,
        }
    }

    fn leaf(
        &mut self,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        a: Handle,
        b: Handle,
        _v: Variance,
    ) -> bool {
        let (na, nb) = (types.node(a), types.node(b));
        // A rigid variable's down-set is checked first: below one lie only itself — which the
        // caller's equality guard already answered — and whatever lies under its lower end.
        if let Some(lower) = nb.rigid_lower() {
            return related(types, scratch, a, lower.raw(), self.relation);
        }
        let solves = self.relation == Relation::Fits;
        if is_signature_type(types, a) && is_signature_type(types, b) {
            return if solves {
                sig_fits(types, scratch, a, b).is_ok()
            } else {
                let lower = applications(types, scratch, a).expect("a signature type");
                let upper = applications(types, scratch, b).expect("a signature type");
                applications_under(&lower, &upper)
            };
        }
        match (na, nb) {
            (TypeNode::OfKind(x), TypeNode::OfKind(y)) => y.admits(x),
            // A quantified shape fits another when some instantiation of its group puts every
            // slot and the return under the other's, with the other's rigid. In the order it lies
            // under only itself, which the caller's equality guard already answered.
            (TypeNode::ExpressionShape { .. }, TypeNode::ExpressionShape { .. }) => {
                solves && admits_shape(types, scratch, a, b)
            }
            // The same clause for a pair of function types, related name by name. Only a pair
            // where one quantifies reaches here — two monomorphic ones pair structurally.
            (TypeNode::KFunction { .. }, TypeNode::KFunction { .. }) => {
                solves && admits_function(types, scratch, a, b)
            }
            // An application lies under the family it applies: a bare family stands for every
            // application of it. A pre-seal sibling has the profile of the member it becomes.
            (
                TypeNode::ConstructorApply { constructor, .. },
                TypeNode::SetMember { .. } | TypeNode::Sibling(_),
            ) => constructor == b,
            // A code kind needing names is above a kind under its own needing no more of them;
            // a bare kind needs none.
            (_, TypeNode::CodeNeeding { kind, names }) => {
                code_needs(types, a).is_some_and(|(sub, needs)| {
                    sub.within_code(kind) && needs.iter().all(|name| names.contains(name))
                })
            }
            // A code kind below `Code` is above exactly the code kinds under it in the code
            // family's tree.
            _ if b.code_parent().is_some() => a.within_code(b),
            // A family top is above every type whose own family it is.
            (_, TypeNode::AnyValue | TypeNode::AnyCode) => family_top(&na) == Some(b),
            _ => false,
        }
    }

    fn set_wise(
        &mut self,
        _types: &TypeRegistry<'_>,
        _scratch: BumpAllocator<'_>,
        a: &[Handle],
        b: &[Handle],
        v: Variance,
        recurse: &mut dyn FnMut(&mut Self, Handle, Handle, Variance) -> bool,
    ) -> bool {
        // `enter` takes every union on the left whole, so `a` is one non-rigid type here: it is
        // below a union when it is below some member.
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

/// A code kind below `Code` as its kind and the names it needs: a bare kind needs none. `None` for
/// every other type.
pub(super) fn code_needs<'run>(
    types: &TypeRegistry<'run>,
    ktype: Handle,
) -> Option<(KType, &'run [BinderSymbol])> {
    match types.node(ktype) {
        TypeNode::CodeNeeding { kind, names } => Some((kind, names)),
        _ if ktype.code_parent().is_some() => Some((wrap(ktype), &[])),
        _ => None,
    }
}

/// The family top `node` lies under by its own shape — `Value`, `Type` or `Code` — or `None` for a
/// node whose family is decided elsewhere: the lattice's top and bottom, a union by its members, a
/// type variable by its bound, and a deferred return by the return it defers; or for an opaque
/// carrier, which reveals no family outside its view. No arm is a wildcard, so a new variant does
/// not compile until it is given a family.
fn family_top(node: &TypeNode<'_>) -> Option<Handle> {
    match node {
        TypeNode::Number
        | TypeNode::Str
        | TypeNode::Bool
        | TypeNode::Null
        | TypeNode::List { .. }
        | TypeNode::Dict { .. }
        | TypeNode::Record { .. }
        | TypeNode::KFunction { .. }
        | TypeNode::ExpressionShape { .. }
        | TypeNode::ConstructorApply { .. }
        | TypeNode::Signature { .. }
        | TypeNode::SignatureApply { .. }
        | TypeNode::SignatureMeet { .. }
        | TypeNode::SetMember { .. }
        | TypeNode::Sibling(_)
        | TypeNode::AnyValue => Some(Handle::ANY_VALUE),
        TypeNode::Identifier
        | TypeNode::Symbol
        | TypeNode::TypeNameToken
        | TypeNode::Expression
        | TypeNode::SigiledTypeExpr
        | TypeNode::RecordType
        | TypeNode::Literal
        | TypeNode::Block
        | TypeNode::Declaration
        | TypeNode::Binder
        | TypeNode::Name
        | TypeNode::Keyword
        | TypeNode::CodeNeeding { .. }
        | TypeNode::AnyCode => Some(Handle::ANY_CODE),
        TypeNode::OfKind(_) => Some(Handle::ANY_TYPE),
        TypeNode::Any
        | TypeNode::Never
        | TypeNode::Union { .. }
        | TypeNode::Quantified { .. }
        | TypeNode::Lexical { .. }
        | TypeNode::Parameter { .. }
        | TypeNode::Carrier { .. }
        | TypeNode::DeferredReturn(_) => None,
    }
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
/// member, so the concrete survivors form an antichain — the subsumption rule a union and an
/// overload set canonicalize by, each from its own side.
///
/// The order relates concrete types only, so a parametric member is kept as it is and holds no
/// other member up: a variable stands beside every concrete member, even one its bound lies under.
///
/// Two distinct handles can subsume each other — a union spelled two ways, two shapes whose slots
/// admit each other — and they stand for one promise, so the run's **first** of them survives for
/// both. Dropping each because of the other would drop the promise itself, leaving the canonical
/// set admitting less than the run it came from.
pub(super) fn unsubsumed<'s>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'s>,
    members: &[Handle],
    dropped: Dropped,
) -> BumpVec<'s, bool> {
    let subsumes = |member: Handle, peer: Handle| match dropped {
        Dropped::Below => is_subtype_of(types, scratch, member, peer),
        Dropped::Above => is_subtype_of(types, scratch, peer, member),
    };
    let mut concrete = BumpVec::with_capacity_in(members.len(), scratch);
    concrete.extend(members.iter().map(|member| types.is_concrete(*member)));
    let mut keep = BumpVec::with_capacity_in(members.len(), scratch);
    keep.extend(members.iter().enumerate().map(|(index, member)| {
        !concrete[index]
            || !members.iter().enumerate().any(|(other, peer)| {
                other != index
                    && concrete[other]
                    && subsumes(*member, *peer)
                    && (other < index || !subsumes(*peer, *member))
            })
    }));
    keep
}
