//! The decide-side bare-name resolution surface.
//!
//! One enum, one ladder. [`resolve_name`] resolves a bare-name part (`Identifier` or leaf `Type`)
//! to a [`Resolution`] — exactly the states a *lookup* observes: resolved to a delivered carrier,
//! parked on a claim's edge, or unbound. It asks nothing about a parked producer's standing — a
//! decide holds no `&mut Scheduler`, so it cannot wire the edge that would make such a read sound;
//! the harness rules on the park when it installs it
//! ([`InstalledEdge`](crate::scheduler::InstalledEdge)). The one variation between the dispatch
//! ladder and the admission cache — which channel a leaf `Type` token consults first — rides as the
//! [`TypeLeafChannels`] parameter, not a second function.
//!
//! The ladder is total: every part lands on one of the three rungs, because a *producer* error
//! reaches the consumer through the park the harness installs, never through a probe here. And it
//! is non-recursive by the exclusive visibility cutoff (`index < cutoff` in the binding tables): a
//! statement's own claim is invisible to its own subtree, so `LET x = (f x)` resolves `x` in the
//! outer scope or lands `Unbound` — it never sees itself and never parks on itself.

use std::rc::Rc;

use crate::machine::ProducerId;
use crate::machine::model::TypeResolution;
use crate::machine::model::{ExpressionPart, KType, TypeIdentifier, TypeRegistry};
use crate::machine::{DeliveredCarried, LexicalFrame, NameLookup, Scope};

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

/// The one bare-name resolution result. Lifetime-free ([`DeliveredCarried`] is lifetime-free), so
/// it crosses branded-scope closure boundaries without ceremony. Consumed by the dispatch walk,
/// the admission cache, and the aggregate-literal cell classifier alike.
pub(crate) enum Resolution {
    /// The bound value lifted into a delivery envelope pinned by its binding scope — a consumer
    /// opens it under those pins to classify the value, so a speculative probe re-anchors nothing
    /// and retains nothing.
    Resolved(DeliveredCarried),
    Parked(ProducerId),
    Unbound(String),
}

/// Which channel a leaf `Type` token consults first — the one variation between [`resolve_name`]'s
/// two callers. (`Identifier` parts read the value channel alone either way.)
pub(in crate::machine::execute) enum TypeLeafChannels {
    /// Straight to the type channel — the dispatch ladder's read of a type operand.
    TypeChannel,
    /// The value channel first — honoring a still-finalizing claim stamped under the type's name —
    /// then the type channel. The admission cache's read, where a park under either channel must
    /// surface before overload selection commits.
    ValueChannelFirst,
}

/// THE bare-name ladder. `part` is a bare-name part (`Identifier` or leaf `Type`); anything else is
/// unreachable.
///
/// An `Identifier` reads the value channel: a bound name seals its binding-scope carrier (value and
/// reach as one unit), a still-finalizing name yields its claim's edge, a miss is `Unbound`. A
/// `Type` reads the type channel — under [`TypeLeafChannels::ValueChannelFirst`] after honoring a
/// value-channel claim under the same name: a resolved leaf seals its resident type carrier, a
/// still-finalizing referent yields its edge, a miss forwards.
pub(in crate::machine::execute) fn resolve_name(
    scope: &Scope<'_>,
    part: &ExpressionPart<'_>,
    chain: Option<&Rc<LexicalFrame>>,
    types: &TypeRegistry,
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
            match type_channel(scope, t, chain.cloned(), types) {
                // A `KType` is a `Copy` registry handle — no foreign reach — so the resident type
                // carrier seals under an empty foreign bundle.
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

/// Best-effort name extraction for a bare-name `ExpressionPart`, used to render
/// the `cycle in type alias <name>` deadlock sample.
pub(in crate::machine::execute) fn bare_name_of(part: &ExpressionPart<'_>) -> Option<String> {
    match part {
        ExpressionPart::Identifier(n) => Some((*n).to_string()),
        ExpressionPart::Type(t) => Some(t.render()),
        _ => None,
    }
}
