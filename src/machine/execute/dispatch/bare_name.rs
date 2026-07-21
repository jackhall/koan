//! The dispatch-side bare-name resolution surface.
//!
//! One ladder — bare name → value channel / type channel → producer screening →
//! seal — owned by [`resolve_bare_carrier`]. Its [`BareCarrier`] result carries
//! exactly the states a consumer observes (sealed / parked / unbound); a producer
//! error is absorbed at the resolution surface as `Err`, so no consumer carries an
//! arm for a pre-excluded state. [`resolve_name_part`] is the admission-cache twin:
//! it keeps the raw-`&KObject` value read [`resolve_dispatch`](super::resolve_dispatch)
//! needs for `accepts_carried`, sharing the type channel and screening with the ladder.

use std::rc::Rc;

use crate::machine::model::Carried;
use crate::machine::model::TypeResolution;
use crate::machine::model::{ExpressionPart, KType, TypeIdentifier, TypeRegistry};
use crate::machine::{
    DeliveredCarried, KError, LexicalFrame, NameLookup, NameOutcome, NodeId, Scope,
};

use super::super::runtime::KoanWorkload;
use super::{producer_standing, ProducerStanding};
use crate::scheduler::Scheduler;

/// Type-channel resolution with the first-producer fold applied once. Folds
/// [`resolve_type_identifier`](Scope::resolve_type_identifier)'s [`TypeResolution`]:
/// a `Done` carries the sealed handle, a `Park` narrows to its first producer (an
/// empty producer list is a miss, so it renders `Unbound`), and an `Unbound` forwards.
pub(in crate::machine::execute) enum TypeChannel {
    Done(KType),
    Parked(NodeId),
    Unbound(String),
}

/// Resolve the type channel for `t`, folding the park-producer list to its first
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
        TypeResolution::Park(producers) => match producers.first() {
            Some(producer) => TypeChannel::Parked(*producer),
            None => TypeChannel::Unbound(t.render()),
        },
    }
}

/// The bare-name ladder result. Lifetime-free ([`DeliveredCarried`] is
/// lifetime-free), so it crosses branded-scope closure boundaries
/// (`literal.rs`) without ceremony.
pub(in crate::machine::execute) enum BareCarrier {
    Sealed(DeliveredCarried),
    Parked(NodeId),
    Unbound(String),
}

/// THE bare-name → sealed-carrier ladder. `part` is a bare-name part (`Identifier`
/// or leaf `Type`); anything else is unreachable.
///
/// An `Identifier` reads the value channel: a bound name seals its binding-scope
/// carrier (value and reach as one unit), a still-finalizing name screens on its
/// producer, a miss is `Unbound`. A `Type` reads the type channel: a resolved leaf
/// seals its resident type carrier, a still-finalizing referent screens, a miss
/// forwards. [`screen`] is the one place producer standing folds into the ladder.
pub(in crate::machine::execute) fn resolve_bare_carrier(
    scope: &Scope<'_>,
    part: &ExpressionPart<'_>,
    chain: Option<&Rc<LexicalFrame>>,
    scheduler: &Scheduler<KoanWorkload>,
    types: &TypeRegistry,
) -> Result<BareCarrier, KError> {
    match part {
        ExpressionPart::Identifier(name) => {
            match scope.resolve_value_carrier(name, chain.map(|c| &**c)) {
                Some(NameLookup::Bound(carrier)) => {
                    Ok(BareCarrier::Sealed(scope.seal_resident_delivered(carrier)))
                }
                Some(NameLookup::Parked(producer)) => screen(scheduler, producer, name.clone()),
                None => Ok(BareCarrier::Unbound(name.clone())),
            }
        }
        ExpressionPart::Type(t) => match type_channel(scope, t, chain.cloned(), types) {
            TypeChannel::Done(kt) => Ok(BareCarrier::Sealed(
                scope.seal_resident_delivered(scope.resident_type_carrier(kt)),
            )),
            TypeChannel::Parked(producer) => screen(scheduler, producer, t.render()),
            TypeChannel::Unbound(n) => Ok(BareCarrier::Unbound(n)),
        },
        _ => unreachable!("resolve_bare_carrier only called on bare-name parts"),
    }
}

/// Fold a parked name's producer standing into the ladder result. A ready-errored
/// producer absorbs into `Err`; a ready-Ok producer means the name finalized to a
/// non-shadowing value, so it is `Unbound`; a still-finalizing one parks.
fn screen(
    scheduler: &Scheduler<KoanWorkload>,
    producer: NodeId,
    name: String,
) -> Result<BareCarrier, KError> {
    match producer_standing(scheduler, producer) {
        ProducerStanding::Errored(e) => Err(e.clone_for_propagation()),
        ProducerStanding::Ready => Ok(BareCarrier::Unbound(name)),
        ProducerStanding::Park => Ok(BareCarrier::Parked(producer)),
    }
}

/// Resolve a bare-name `ExpressionPart` (`Identifier` or leaf `Type`) into the
/// admission-cache currency. The value channel reads the raw `&KObject`
/// ([`resolve_with_chain`](Scope::resolve_with_chain)) so admission can call
/// `accepts_carried`; the type channel and screening are shared with the ladder.
/// A producer error absorbs into `Err`, surfacing before `resolve_dispatch` is
/// consulted.
pub(in crate::machine::execute) fn resolve_name_part<'step>(
    scope: &Scope<'step>,
    part: &ExpressionPart<'step>,
    scheduler: &Scheduler<KoanWorkload>,
    active_chain: Option<&Rc<LexicalFrame>>,
    types: &TypeRegistry,
) -> Result<NameOutcome<'step>, KError> {
    let (name, is_type) = match part {
        ExpressionPart::Identifier(n) => (n.as_str(), None),
        ExpressionPart::Type(t) => (t.as_str(), Some(t)),
        _ => unreachable!("resolve_name_part only called on bare-name parts"),
    };
    let chain = active_chain.map(|c| &**c);
    match scope.resolve_with_chain(name, chain) {
        Some(NameLookup::Parked(producer)) => return screen_outcome(scheduler, producer, name),
        // An Identifier part reads the value channel; a Type part takes the type ladder below.
        Some(NameLookup::Bound(obj)) if is_type.is_none() => {
            return Ok(NameOutcome::Resolved(Carried::Object(obj)));
        }
        Some(NameLookup::Bound(_)) | None => {}
    }
    match is_type {
        // The bare-leaf type token routes through the memoized, park-capable bridge, reusing the
        // same first-producer fold and ready/errored/park screen the value-side placeholder arm
        // applies.
        Some(t) => match type_channel(scope, t, active_chain.cloned(), types) {
            TypeChannel::Done(kt) => Ok(NameOutcome::Resolved(Carried::Type(kt))),
            TypeChannel::Unbound(n) => Ok(NameOutcome::Unbound(n)),
            TypeChannel::Parked(producer) => screen_outcome(scheduler, producer, name),
        },
        None => Ok(NameOutcome::Unbound(name.to_string())),
    }
}

/// Fold a parked name's producer standing into a [`NameOutcome`] — the
/// admission-cache twin of [`screen`].
fn screen_outcome<'step>(
    scheduler: &Scheduler<KoanWorkload>,
    producer: NodeId,
    name: &str,
) -> Result<NameOutcome<'step>, KError> {
    match producer_standing(scheduler, producer) {
        ProducerStanding::Errored(e) => Err(e.clone_for_propagation()),
        ProducerStanding::Ready => Ok(NameOutcome::Unbound(name.to_string())),
        ProducerStanding::Park => Ok(NameOutcome::Parked(producer)),
    }
}

/// Best-effort name extraction for a bare-name `ExpressionPart`, used to render
/// the `cycle in type alias <name>` deadlock sample.
pub(in crate::machine::execute) fn bare_name_of(part: &ExpressionPart<'_>) -> Option<String> {
    match part {
        ExpressionPart::Identifier(n) => Some(n.clone()),
        ExpressionPart::Type(t) => Some(t.render()),
        _ => None,
    }
}
