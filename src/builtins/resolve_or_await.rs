//! Shared pieces of the resolve-or-await protocol: the sub-dispatch-then-continue combinator, the
//! re-resolve-on-wake read (with its second-park protocol error), the type-terminal read a
//! dep-finish makes of its result, and the diagnostics they raise. A routing site states its own
//! carrier shape and slot name and nothing else.

use crate::machine::model::RunRegistries;
use crate::machine::model::TypeResolution;
use crate::machine::model::{Carried, KType};
use crate::machine::{Action, AwaitContinue, DepPlacement, DepTerminal, FinishCtx, SubDispatch};
use crate::machine::{KError, KErrorKind, Scope};
use crate::scheduler::Deps;

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

/// Re-run `resolve` after the parked binders finished. `Done` yields the type; `Park` is the
/// protocol error; `Unbound` is a hard miss.
pub(crate) fn resolve_at_wake<'a>(
    scope: &Scope<'a>,
    slot: &str,
    registries: &RunRegistries,
    resolve: impl Fn(&Scope<'a>, &RunRegistries) -> TypeResolution<KType>,
) -> Result<KType, KError> {
    match resolve(scope, registries) {
        TypeResolution::Done(kt) => Ok(kt),
        TypeResolution::Park(_) => Err(parked_after_wake_error(slot)),
        TypeResolution::Unbound(name) => Err(unbound_error(
            slot,
            &crate::machine::model::unknown_type_name(name, registries),
        )),
    }
}

/// Read the type a sub-dispatch resolved to out of a dep-finish's results — a non-type
/// result is the slot's canonical shape error. The value is read at the borrow of the guard bound
/// here, over a dep resident in a region this step already covers, at the step's own brand; the
/// resolved `KType` is a `Copy` handle that escapes that borrow, so a caller that seals it into a
/// result carries it by value.
pub(crate) fn expect_type_terminal(
    results: &[DepTerminal<'_>],
    dep_index: usize,
    slot: &str,
    registries: &RunRegistries,
) -> Result<KType, KError> {
    let terminal: &DepTerminal = &results[dep_index];
    let opened = terminal.cell.open_at();
    match opened.value() {
        Carried::Type(kt) => Ok(kt),
        Carried::Object(other) => Err(non_type_result_error(slot, other.ktype().name(registries))),
        Carried::UnresolvedType(ti) => Err(non_type_result_error(
            slot,
            crate::machine::model::render_label(ti.symbol(), registries),
        )),
    }
}

/// [`dispatch_type_then`] over a node already in working form — what a declarator hands it once its
/// co-declared references are threaded in as resolved cells.
pub(crate) fn dispatch_working_type_then<'a>(
    expr: crate::machine::model::WorkingExpression<'a>,
    slot: &'static str,
    on_resolved: impl for<'r> FnOnce(&FinishCtx<'a, 'r>, KType) -> Action<'a> + 'a,
) -> Action<'a> {
    let finish: AwaitContinue<'a> = Box::new(move |fctx, results| {
        let kt = crate::try_action!(expect_type_terminal(results, 0, slot, fctx.registries));
        on_resolved(fctx, kt)
    });
    Action::await_deps(
        Deps::from_requests([SubDispatch {
            expr,
            placement: DepPlacement::OwnScope,
        }]),
        finish,
    )
}
