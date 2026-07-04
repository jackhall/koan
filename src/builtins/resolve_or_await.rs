//! The resolve-or-await protocol combinator: a caller states the identifier, the scope/chain
//! (via a resolve closure), the slot name for diagnostics, and the on-resolved continuation.
//! Park-on-producer, re-resolve-on-wake, and the second-park protocol error live here, so every
//! routing site states its own carrier shape and slot name and nothing else.

use crate::machine::core::kfunction::action::{
    scope_frame, Action, AwaitContinue, DepPlacement, DepRequest, DepTerminal, FinishCtx,
};
use crate::machine::core::TypeHit;
use crate::machine::model::ast::KExpression;
use crate::machine::model::types::TypeResolution;
use crate::machine::model::values::CarriedFamily;
use crate::machine::model::{Carried, KType};
use crate::machine::{FrameSet, KError, KErrorKind, NameLookup, Scope};
use crate::scheduler::DepResults;
use crate::witnessed::{Sealed, StepContext};

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
    on_resolved: impl FnOnce(&FinishCtx<'a>, KType<'a>) -> Action<'a> + 'a,
) -> Action<'a> {
    match resolve(scope) {
        // The synchronous arm hands the continuation the same `FinishCtx` shape a wake-time finish
        // receives, built over the caller's own scope and its frame's step context. `scope_frame(scope)`
        // matches the wake side's provenance — the harness `StepContext` also wraps the scope-derived
        // dest frame — so both arms allocate in the same region, USING windows included.
        TypeResolution::Done(kt) => {
            let fctx = FinishCtx {
                scope,
                ctx: StepContext::new(scope_frame(scope)),
            };
            on_resolved(&fctx, kt)
        }
        TypeResolution::Park(producers) => {
            let finish: AwaitContinue<'a> = Box::new(move |fctx, _results| {
                let kt = crate::try_action!(resolve_at_wake(fctx.scope, slot, resolve));
                on_resolved(fctx, kt)
            });
            Action::AwaitDeps {
                deps: producers.into_iter().map(DepRequest::Existing).collect(),
                finish,
            }
        }
        TypeResolution::Unbound(detail) => Action::Done(Err(unbound_error(slot, &detail))),
    }
}

/// Read the type a sub-dispatch resolved to out of a dep-finish's owned results, paired with the
/// terminal's own dep carrier — a non-type result is the slot's canonical shape error. The
/// resolved `KType` can embed a borrow into the terminal's producer region (a bound `KFunctor`,
/// a nominal `SetRef`, ...), so a caller that seals the type into a result must fold the carrier
/// in (`StepContext::alloc_type_with`) or fold it into a scope's reach-set (`Scope::fold_reach`)
/// before the type crosses into stored state.
pub(crate) fn expect_type_terminal<'a, 'd>(
    results: &DepResults<'_, &'d DepTerminal<'a>>,
    owned_pos: usize,
    slot: &str,
) -> Result<(KType<'a>, &'d Sealed<CarriedFamily, FrameSet>), KError> {
    // The sub-dispatch's resolved type read live at the step brand (un-relocated); the caller
    // re-allocates it into the destination region when it constructs, folding `carrier` in.
    let terminal: &'d DepTerminal<'a> = results.owned(owned_pos);
    match terminal.value {
        Carried::Type(kt) => Ok((kt.clone(), &terminal.carrier)),
        Carried::Object(other) => Err(non_type_result_error(slot, other.ktype().name())),
    }
}

/// Sub-dispatch `expr` in the slot's own scope and hand the resolved type, plus its dep carrier,
/// to `on_resolved` at dep-finish — `on_resolved` folds the carrier into whatever it seals so the
/// type's own reach travels with it.
pub(crate) fn dispatch_type_then<'a>(
    expr: KExpression<'a>,
    slot: &'static str,
    on_resolved: impl FnOnce(&FinishCtx<'a>, KType<'a>, &Sealed<CarriedFamily, FrameSet>) -> Action<'a>
        + 'a,
) -> Action<'a> {
    let finish: AwaitContinue<'a> = Box::new(move |fctx, results| {
        let (kt, carrier) = crate::try_action!(expect_type_terminal(&results, 0, slot));
        on_resolved(fctx, kt, carrier)
    });
    Action::AwaitDeps {
        deps: vec![DepRequest::Dispatch {
            expr,
            placement: DepPlacement::OwnScope,
        }],
        finish,
    }
}
