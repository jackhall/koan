//! The decide-side bare-name resolution surface.
//!
//! One enum, one ladder. [`resolve_name`] resolves a bare-name part (`Identifier` or leaf `Type`)
//! to a [`Resolution`] — exactly the states a *lookup* observes: resolved to a delivered carrier,
//! parked on a claim's edge, or unbound. It asks nothing about a parked producer's standing —
//! resolution holds no scheduler, so it cannot wire the edge that would make such a read sound; the
//! harness rules on the park when it installs it
//! ([`InstalledEdge`](crate::scheduler::InstalledEdge)). Which channel a leaf `Type` token consults
//! first rides as the [`TypeLeafChannels`] parameter, not a second function.
//!
//! The ladder is total: every part lands on one of the three rungs, because a *producer* error
//! reaches the consumer through the park the harness installs, never through a probe here. And it
//! is non-recursive by the exclusive visibility cutoff (`index < cutoff` in the binding tables): a
//! statement's own claim is invisible to its own subtree, so `LET x = (f x)` resolves `x` in the
//! outer scope or lands `Unbound` — it never sees itself and never parks on itself.

use std::rc::Rc;

use crate::machine::ProducerId;
use crate::machine::model::TypeResolution;
use crate::machine::model::{ExpressionPart, KType, RunRegistries, TypeIdentifier};
use crate::machine::{DeliveredCarried, LexicalFrame, NameLookup, Scope};

use crate::machine::model::Carried;

/// Type-channel resolution with the park-source list already folded to a single edge, so no
/// consumer has to choose among sources.
pub(in crate::machine::execute) enum TypeChannel {
    Done(KType),
    Parked(ProducerId),
    Unbound(String),
}

/// Narrow [`resolve_type_identifier`](Scope::resolve_type_identifier)'s park-source list to its
/// first element, since a [`Resolution`] parks on exactly one edge. An empty list has no edge to
/// park on, so it renders the miss diagnostic instead.
pub(in crate::machine::execute) fn type_channel(
    scope: &Scope<'_>,
    t: &TypeIdentifier,
    chain: Option<Rc<LexicalFrame>>,
    registries: &RunRegistries,
) -> TypeChannel {
    match scope.resolve_type_identifier(t, chain, registries) {
        TypeResolution::Done(kt) => TypeChannel::Done(kt),
        TypeResolution::Unbound(n) => TypeChannel::Unbound(n),
        TypeResolution::Park(sources) => match sources.first() {
            Some(source) => TypeChannel::Parked(*source),
            None => TypeChannel::Unbound(t.render()),
        },
    }
}

/// The one bare-name resolution result. Lifetime-free ([`DeliveredCarried`] is lifetime-free), so
/// it crosses branded-scope closure boundaries without ceremony.
pub(crate) enum Resolution {
    /// The bound value lifted into a delivery envelope pinned by its binding scope — a consumer
    /// opens it under those pins to classify the value, so a speculative probe re-anchors nothing
    /// and retains nothing.
    Resolved(DeliveredCarried),
    Parked(ProducerId),
    Unbound(String),
}

/// Which channel a leaf `Type` token consults first. (`Identifier` parts read the value channel
/// alone either way.)
pub(in crate::machine::execute) enum TypeLeafChannels {
    TypeChannel,
    /// Honor a still-finalizing claim stamped under the type's name before consulting the type
    /// channel, so a park under either channel surfaces before overload selection commits.
    ValueChannelFirst,
}

/// `part` must be a bare-name part (`Identifier` or leaf `Type`); anything else is unreachable.
///
/// A bound `Identifier` seals its binding-scope carrier — value and reach as one unit — so the
/// result stands on its own once the scope borrow closes.
pub(in crate::machine::execute) fn resolve_name(
    scope: &Scope<'_>,
    part: &ExpressionPart<'_>,
    chain: Option<&Rc<LexicalFrame>>,
    registries: &RunRegistries,
    type_leaf: TypeLeafChannels,
) -> Resolution {
    match part {
        ExpressionPart::Identifier(name) => {
            match scope.resolve_value_delivered(name, chain.map(|c| &**c)) {
                Some(NameLookup::Bound(delivered)) => Resolution::Resolved(delivered),
                Some(NameLookup::Parked(source)) => Resolution::Parked(source),
                None => Resolution::Unbound((*name).to_string()),
            }
        }
        ExpressionPart::Type(t) => {
            if matches!(type_leaf, TypeLeafChannels::ValueChannelFirst)
                && let Some(NameLookup::Parked(source)) =
                    scope.resolve_value_delivered(t.as_str(), chain.map(|c| &**c))
            {
                return Resolution::Parked(source);
            }
            match type_channel(scope, t, chain.cloned(), registries) {
                // A `KType` is a `Copy` registry handle with no foreign reach, so it delivers
                // resident with no coverage to assemble.
                TypeChannel::Done(kt) => {
                    Resolution::Resolved(scope.deliver_resident(Carried::Type(kt)))
                }
                TypeChannel::Parked(source) => Resolution::Parked(source),
                TypeChannel::Unbound(n) => Resolution::Unbound(n),
            }
        }
        _ => unreachable!("resolve_name only called on bare-name parts"),
    }
}
