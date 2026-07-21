//! The resolve-or-await protocol combinator: a caller states the identifier, the scope/chain
//! (via a resolve closure), the slot name for diagnostics, and the on-resolved continuation.
//! Park-on-producer, re-resolve-on-wake, and the second-park protocol error live here, so every
//! routing site states its own carrier shape and slot name and nothing else.

use crate::machine::model::KExpression;
use crate::machine::model::TypeRegistry;
use crate::machine::model::TypeResolution;
use crate::machine::model::{Carried, KType};
use crate::machine::{Action, AwaitContinue, DepPlacement, DepTerminal, FinishCtx, OwnedDispatch};
use crate::machine::{KError, KErrorKind, NameLookup, Scope};
use crate::scheduler::{DepResults, Deps};

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
pub(crate) fn classify_name_lookup(
    lookup: Option<NameLookup<KType>>,
    name: &str,
) -> TypeResolution<KType> {
    match lookup {
        Some(NameLookup::Bound(kt)) => TypeResolution::Done(kt),
        Some(NameLookup::Parked(producer)) => TypeResolution::Park(vec![producer]),
        None => TypeResolution::Unbound(format!("unknown type name `{name}`")),
    }
}

/// Re-run `resolve` after the parked producers finished. `Done` yields the type; `Park` is the
/// protocol error; `Unbound` is a hard miss.
pub(crate) fn resolve_at_wake<'a>(
    scope: &Scope<'a>,
    slot: &str,
    resolve: impl Fn(&Scope<'a>) -> TypeResolution<KType>,
) -> Result<KType, KError> {
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
    resolve: impl Fn(&Scope<'a>) -> TypeResolution<KType> + 'a,
    on_resolved: impl for<'r> FnOnce(&FinishCtx<'a, 'r>, KType) -> Action<'a> + 'a,
    types: &TypeRegistry,
) -> Action<'a> {
    match resolve(scope) {
        // The synchronous arm hands the continuation the same `FinishCtx` a wake-time finish
        // receives: `FinishCtx::for_scope` reconstructs the step context over the scope's own frame,
        // matching the wake side's provenance, so both arms allocate in the same region.
        TypeResolution::Done(kt) => on_resolved(&FinishCtx::for_scope(scope, types), kt),
        TypeResolution::Park(producers) => {
            let finish: AwaitContinue<'a> = Box::new(move |fctx, _results| {
                let kt = crate::try_action!(resolve_at_wake(fctx.scope, slot, resolve));
                on_resolved(fctx, kt)
            });
            Action::AwaitDeps {
                deps: Deps::from_parks(producers),
                finish,
            }
        }
        TypeResolution::Unbound(detail) => Action::Done(Err(unbound_error(slot, &detail))),
    }
}

/// Read the type a sub-dispatch resolved to out of a dep-finish's owned results — a non-type
/// result is the slot's canonical shape error. The resolved `KType` is a `Copy` handle read out of
/// the terminal, so a caller that seals it into a result carries it by value.
pub(crate) fn expect_type_terminal<'a, 'd>(
    results: &DepResults<'_, &'d DepTerminal<'a>>,
    owned_pos: usize,
    slot: &str,
    types: &TypeRegistry,
) -> Result<KType, KError> {
    let terminal: &'d DepTerminal<'a> = results.owned(owned_pos);
    match terminal.value {
        Carried::Type(kt) => Ok(kt),
        Carried::Object(other) => Err(non_type_result_error(slot, other.ktype().name(types))),
        Carried::UnresolvedType(ti) => Err(non_type_result_error(slot, ti.render())),
    }
}

/// Sub-dispatch `expr` in the slot's own scope and hand the resolved type to `on_resolved` at
/// dep-finish. The resolved `KType` is owned data, so the dep carrier stays behind.
pub(crate) fn dispatch_type_then<'a>(
    expr: KExpression<'a>,
    slot: &'static str,
    on_resolved: impl for<'r> FnOnce(&FinishCtx<'a, 'r>, KType) -> Action<'a> + 'a,
) -> Action<'a> {
    let finish: AwaitContinue<'a> = Box::new(move |fctx, results| {
        let kt = crate::try_action!(expect_type_terminal(&results, 0, slot, fctx.types));
        on_resolved(fctx, kt)
    });
    Action::AwaitDeps {
        deps: Deps::from_owned([OwnedDispatch {
            expr,
            placement: DepPlacement::OwnScope,
        }]),
        finish,
    }
}
