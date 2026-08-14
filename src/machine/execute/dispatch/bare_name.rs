//! The dispatch-side bare-name resolution surface.
//!
//! One ladder — bare name → value channel / type channel → seal — owned by
//! [`resolve_bare_carrier`]. Its [`BareCarrier`] result carries exactly the states a *lookup*
//! observes: sealed, parked on the claim's edge, or unbound. It asks nothing about the parked
//! binder's standing — a decide holds no `&mut Scheduler`, so it cannot wire the edge that would
//! make such a read sound; the harness rules on the park when it installs it
//! ([`InstalledEdge`](crate::scheduler::InstalledEdge)). [`resolve_name_part`] is the
//! admission-cache twin: it caches the same delivered carrier, which
//! [`resolve_dispatch`](super::resolve_dispatch) opens under the envelope's own pins for
//! `accepts_carried`, sharing the type channel with the ladder.

use std::rc::Rc;

use crate::machine::core::ProducerId;
use crate::machine::model::TypeResolution;
use crate::machine::model::{ExpressionPart, KType, TypeIdentifier, TypeRegistry};
use crate::machine::{DeliveredCarried, LexicalFrame, NameLookup, NameOutcome, Scope};

use crate::machine::model::Carried;

/// Type-channel resolution with the first-source fold applied once. Folds
/// [`resolve_type_identifier`](Scope::resolve_type_identifier)'s [`TypeResolution`]:
/// a `Done` carries the sealed handle, a `Park` narrows to its first source edge (an
/// empty park list is a miss, so it renders `Unbound`), and an `Unbound` forwards.
pub(in crate::machine::execute) enum TypeChannel {
    Done(KType),
    Parked(ProducerId),
    Unbound(String),
}

/// Resolve the type channel for `t`, folding the park-source list to its first
/// element. A visible type alias has already resolved its RHS, so a leaf parks on
/// at most one binder; an empty list renders the miss diagnostic.
pub(in crate::machine::execute) fn type_channel(
    scope: &Scope<'_>,
    t: &TypeIdentifier,
    chain: Option<Rc<LexicalFrame>>,
    types: &TypeRegistry,
) -> TypeChannel {
    match scope.resolve_type_identifier(t, chain, types) {
        TypeResolution::Done(kt) => TypeChannel::Done(kt),
        TypeResolution::Unbound(n) => TypeChannel::Unbound(n),
        TypeResolution::Park(sources) => match sources.first() {
            Some(source) => TypeChannel::Parked(*source),
            None => TypeChannel::Unbound(t.render()),
        },
    }
}

/// The bare-name ladder result. Lifetime-free ([`DeliveredCarried`] is
/// lifetime-free), so it crosses branded-scope closure boundaries
/// (`literal.rs`) without ceremony.
pub(in crate::machine::execute) enum BareCarrier {
    Sealed(DeliveredCarried),
    Parked(ProducerId),
    Unbound(String),
}

/// THE bare-name → sealed-carrier ladder. `part` is a bare-name part (`Identifier`
/// or leaf `Type`); anything else is unreachable.
///
/// An `Identifier` reads the value channel: a bound name seals its binding-scope
/// carrier (value and reach as one unit), a still-finalizing name yields its claim's edge, a miss
/// is `Unbound`. A `Type` reads the type channel: a resolved leaf seals its resident type carrier,
/// a still-finalizing referent yields its edge, a miss forwards. The ladder is total: every part
/// lands on one of the three rungs, because a *producer* error reaches the consumer through the
/// park the harness installs, never through a probe here.
pub(in crate::machine::execute) fn resolve_bare_carrier(
    scope: &Scope<'_>,
    part: &ExpressionPart<'_>,
    chain: Option<&Rc<LexicalFrame>>,
    types: &TypeRegistry,
) -> BareCarrier {
    match part {
        ExpressionPart::Identifier(name) => {
            match scope.resolve_value_delivered(name, chain.map(|c| &**c)) {
                Some(NameLookup::Bound(delivered)) => BareCarrier::Sealed(delivered),
                Some(NameLookup::Parked(source)) => BareCarrier::Parked(source),
                None => BareCarrier::Unbound((*name).to_string()),
            }
        }
        ExpressionPart::Type(t) => match type_channel(scope, t, chain.cloned(), types) {
            // A `KType` is a `Copy` registry handle — no foreign reach — so the resident type
            // carrier seals under an empty foreign bundle.
            TypeChannel::Done(kt) => BareCarrier::Sealed(scope.deliver_resident(Carried::Type(kt))),
            TypeChannel::Parked(source) => BareCarrier::Parked(source),
            TypeChannel::Unbound(n) => BareCarrier::Unbound(n),
        },
        _ => unreachable!("resolve_bare_carrier only called on bare-name parts"),
    }
}

/// Resolve a bare-name `ExpressionPart` (`Identifier` or leaf `Type`) into the
/// admission-cache currency. The value channel lifts the binding into a delivery envelope, which
/// admission opens under its own pins to call `accepts_carried`; the type channel is shared with
/// the ladder. A still-finalizing name yields its claim's edge, which the harness rules on when it
/// installs the park.
pub(in crate::machine::execute) fn resolve_name_part(
    scope: &Scope<'_>,
    part: &ExpressionPart<'_>,
    active_chain: Option<&Rc<LexicalFrame>>,
    types: &TypeRegistry,
) -> NameOutcome {
    let (name, is_type) = match part {
        ExpressionPart::Identifier(n) => (*n, None),
        ExpressionPart::Type(t) => (t.as_str(), Some(t)),
        _ => unreachable!("resolve_name_part only called on bare-name parts"),
    };
    let chain = active_chain.map(|c| &**c);
    match scope.resolve_value_delivered(name, chain) {
        Some(NameLookup::Parked(source)) => return NameOutcome::Parked(source),
        // An Identifier part reads the value channel; a Type part takes the type ladder below.
        Some(NameLookup::Bound(delivered)) if is_type.is_none() => {
            return NameOutcome::Resolved(delivered);
        }
        Some(NameLookup::Bound(_)) | None => {}
    }
    match is_type {
        // The bare-leaf type token routes through the memoized, park-capable bridge, reusing the
        // same first-source fold the value-side placeholder arm applies.
        Some(t) => match type_channel(scope, t, active_chain.cloned(), types) {
            // A `KType` is a `Copy` registry handle with no reach, so the admission cache carries
            // it in the same envelope currency under an empty foreign bundle.
            TypeChannel::Done(kt) => {
                NameOutcome::Resolved(scope.deliver_resident(Carried::Type(kt)))
            }
            TypeChannel::Unbound(n) => NameOutcome::Unbound(n),
            TypeChannel::Parked(source) => NameOutcome::Parked(source),
        },
        None => NameOutcome::Unbound(name.to_string()),
    }
}

/// Best-effort name extraction for a bare-name `ExpressionPart`, used to render
/// the `cycle in type alias <name>` deadlock sample.
pub(in crate::machine::execute) fn bare_name_of(part: &ExpressionPart<'_>) -> Option<String> {
    match part {
        ExpressionPart::Identifier(n) => Some((*n).to_string()),
        ExpressionPart::Type(t) => Some(t.render()),
        _ => None,
    }
}
