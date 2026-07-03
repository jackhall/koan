//! The resolve-or-await protocol combinator: a caller states the identifier, the scope/chain
//! (via a resolve closure), the slot name for diagnostics, and the on-resolved continuation.
//! Park-on-producer, re-resolve-on-wake, and the second-park protocol error live here, so every
//! routing site states its own carrier shape and slot name and nothing else.

use crate::machine::core::kfunction::action::{Action, AwaitContinue, DepPlacement, DepRequest};
use crate::machine::core::TypeHit;
use crate::machine::model::ast::KExpression;
use crate::machine::model::types::TypeResolution;
use crate::machine::model::{Carried, KType};
use crate::machine::{KError, KErrorKind, NameLookup, Scope};
use crate::scheduler::DepResults;

/// `{slot}: {detail}` — the unbound / hard-miss shape.
pub(crate) fn unbound_error(slot: &str, detail: &str) -> KError {
    KError::new(KErrorKind::ShapeError(format!("{slot}: {detail}")))
}

/// Every parked producer is terminal by the dep-finish invariant, so a second park after wake is
/// a protocol error, not a longer wait.
fn parked_after_wake_error(slot: &str) -> KError {
    KError::new(KErrorKind::ShapeError(format!(
        "{slot} parked after dep-finish wake"
    )))
}

fn non_type_result_error(slot: &str, got_kind: String) -> KError {
    KError::new(KErrorKind::ShapeError(format!(
        "{slot} sub-dispatch resolved to a non-type value of kind `{got_kind}`"
    )))
}

/// Classify a plain type-table lookup (`Scope::resolve_type_with_chain`).
pub(crate) fn classify_name_lookup<'a>(
    lookup: Option<NameLookup<&'a KType<'a>>>,
    name: &str,
) -> TypeResolution<KType<'a>> {
    match lookup {
        Some(NameLookup::Bound(kt)) => TypeResolution::Done(kt.clone()),
        Some(NameLookup::Parked(producer)) => TypeResolution::Park(vec![producer]),
        None => TypeResolution::Unbound(format!("unknown type name `{name}`")),
    }
}

/// Drop a `TypeHit`'s reach, keeping the resolved `KType` — adapts
/// `Scope::resolve_type_identifier`'s outcome into the combinator currency.
pub(crate) fn classify_type_hit<'a>(
    resolution: TypeResolution<TypeHit<'a>>,
) -> TypeResolution<KType<'a>> {
    resolution.and_then_done(|hit| TypeResolution::Done(hit.kt.clone()))
}

/// Re-run `resolve` after the parked producers finished. `Done` yields the type; `Park` is the
/// protocol error; `Unbound` is a hard miss.
pub(crate) fn resolve_at_wake<'a>(
    scope: &Scope<'a>,
    slot: &str,
    resolve: impl Fn(&Scope<'a>) -> TypeResolution<KType<'a>>,
) -> Result<KType<'a>, KError> {
    match resolve(scope) {
        TypeResolution::Done(kt) => Ok(kt),
        TypeResolution::Park(_) => Err(parked_after_wake_error(slot)),
        TypeResolution::Unbound(detail) => Err(unbound_error(slot, &detail)),
    }
}

/// Resolve now; park on the producers and re-resolve at wake when the name is still finalizing.
/// `resolve` runs once synchronously and (on the park arm) once more at dep-finish against the
/// wake-side scope.
pub(crate) fn resolve_or_await<'a>(
    scope: &'a Scope<'a>,
    slot: &'static str,
    resolve: impl Fn(&Scope<'a>) -> TypeResolution<KType<'a>> + 'a,
    on_resolved: impl FnOnce(&'a Scope<'a>, KType<'a>) -> Action<'a> + 'a,
) -> Action<'a> {
    match resolve(scope) {
        TypeResolution::Done(kt) => on_resolved(scope, kt),
        TypeResolution::Park(producers) => {
            let finish: AwaitContinue<'a> = Box::new(move |fctx, _results| {
                let kt = crate::try_action!(resolve_at_wake(fctx.scope, slot, resolve));
                on_resolved(fctx.scope, kt)
            });
            Action::AwaitDeps {
                deps: producers.into_iter().map(DepRequest::Existing).collect(),
                finish,
            }
        }
        TypeResolution::Unbound(detail) => Action::Done(Err(unbound_error(slot, &detail))),
    }
}

/// Read the type a sub-dispatch resolved to out of a dep-finish's owned results; a non-type
/// result is the slot's canonical shape error.
pub(crate) fn expect_type_result<'a>(
    results: &DepResults<'_, Carried<'a>>,
    owned_pos: usize,
    slot: &str,
) -> Result<KType<'a>, KError> {
    match *results.owned(owned_pos) {
        Carried::Type(kt) => Ok(kt.clone()),
        Carried::Object(other) => Err(non_type_result_error(slot, other.ktype().name())),
    }
}

/// Sub-dispatch `expr` in the slot's own scope and hand the resolved type to `on_resolved` at
/// dep-finish.
pub(crate) fn dispatch_type_then<'a>(
    expr: KExpression<'a>,
    slot: &'static str,
    on_resolved: impl FnOnce(&'a Scope<'a>, KType<'a>) -> Action<'a> + 'a,
) -> Action<'a> {
    let finish: AwaitContinue<'a> = Box::new(move |fctx, results| {
        let kt = crate::try_action!(expect_type_result(&results, 0, slot));
        on_resolved(fctx.scope, kt)
    });
    Action::AwaitDeps {
        deps: vec![DepRequest::Dispatch {
            expr,
            placement: DepPlacement::OwnScope,
        }],
        finish,
    }
}
