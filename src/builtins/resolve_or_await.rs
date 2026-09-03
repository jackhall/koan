//! The resolve-or-await protocol combinator: a caller states the identifier, the scope/chain
//! (via a resolve closure), the slot name for diagnostics, and the on-resolved continuation.
//! Park-on-producer, re-resolve-on-wake, and the second-park protocol error live here, so every
//! routing site states its own carrier shape and slot name and nothing else.
//!
//! The waiting this does is *not* the dispatch-time park. A declarator's reference to a
//! co-declared sibling is exempt from that one
//! ([`WorkingExpression::park_exempt_slot`](crate::machine::model::WorkingExpression::park_exempt_slot))
//! precisely because waiting there would deadlock the group the two names share; the token reaches
//! the body raw instead, and the body waits on the sibling's claim edge through `deps_on`.

use std::rc::Rc;

use crate::machine::LexicalFrame;
use crate::machine::execute::deps_on;
use crate::machine::model::RunRegistries;
use crate::machine::model::TypeResolution;
use crate::machine::model::type_name_miss;
use crate::machine::model::{Carried, KType};
use crate::machine::{Action, AwaitContinue, DepPlacement, DepTerminal, FinishCtx, SubDispatch};
use crate::machine::{KError, KErrorKind, NameLookup, Scope};
use crate::scheduler::Deps;

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
    name: crate::machine::model::TypeSymbol,
) -> TypeResolution<KType> {
    match lookup {
        Some(NameLookup::Bound(kt)) => TypeResolution::Done(kt),
        Some(NameLookup::Parked(producer)) => TypeResolution::Park(vec![producer]),
        None => TypeResolution::Unbound(name),
    }
}

/// Re-run `resolve` after the parked binders finished. `Done` yields the type; `Park` is the
/// protocol error; `Unbound` is a hard miss, classified against `chain` — the same lexical
/// position `resolve` gates its own lookup at, so the miss separates a forward reference from a
/// name declared nowhere.
pub(crate) fn resolve_at_wake<'a>(
    scope: &Scope<'a>,
    slot: &str,
    chain: Option<&LexicalFrame>,
    registries: &RunRegistries,
    resolve: impl Fn(&Scope<'a>, &RunRegistries) -> TypeResolution<KType>,
) -> Result<KType, KError> {
    match resolve(scope, registries) {
        TypeResolution::Done(kt) => Ok(kt),
        TypeResolution::Park(_) => Err(parked_after_wake_error(slot)),
        TypeResolution::Unbound(name) => {
            Err(type_name_miss(scope, name, chain, Some(slot), registries))
        }
    }
}

/// Resolve now; park on the binders' claim edges and re-resolve at wake when the name is still
/// finalizing.
/// `resolve` runs once synchronously and (on the park arm) once more at dep-finish against the
/// wake-side scope.
pub(crate) fn resolve_or_await<'a>(
    scope: &'a Scope<'a>,
    slot: &'static str,
    chain: Option<Rc<LexicalFrame>>,
    resolve: impl Fn(&Scope<'a>, &RunRegistries) -> TypeResolution<KType> + 'a,
    on_resolved: impl for<'r> FnOnce(&FinishCtx<'a, 'r>, KType) -> Action<'a> + 'a,
    registries: &RunRegistries,
) -> Action<'a> {
    match resolve(scope, registries) {
        // The synchronous arm hands the continuation the same `FinishCtx` a wake-time finish
        // receives: `FinishCtx::for_scope` reconstructs the step context over the scope's own frame,
        // matching the wake side's provenance, so both arms allocate in the same region.
        TypeResolution::Done(kt) => on_resolved(&FinishCtx::for_scope(scope, registries), kt),
        TypeResolution::Park(sources) => {
            let finish: AwaitContinue<'a> = Box::new(move |fctx, _results| {
                let kt = crate::try_action!(resolve_at_wake(
                    fctx.scope,
                    slot,
                    chain.as_deref(),
                    fctx.registries,
                    resolve
                ));
                on_resolved(fctx, kt)
            });
            Action::await_deps(deps_on(sources), finish)
        }
        TypeResolution::Unbound(name) => Action::done(Err(type_name_miss(
            scope,
            name,
            chain.as_deref(),
            Some(slot),
            registries,
        ))),
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

/// Sub-dispatch a node already in working form and hand the resolved type to `on_resolved` at
/// dep-finish — what a declarator calls once its co-declared references are threaded in as
/// resolved cells. The resolved `KType` is owned data, so the dep carrier stays behind.
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
