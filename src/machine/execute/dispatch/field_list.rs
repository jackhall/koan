//! Shared deferral for typed field lists whose elaboration parked on a forward type or
//! scheduled sub-Dispatches for sigil field types — FN/FUNCTOR parameter lists, the
//! NEWTYPE record repr, the UNION schema, and the standalone record-type sigil.
//!
//! One dep-finish waits on `[park_producers ++ owned_subs]`; its finish re-walks the field
//! list through [`parse_typed_field_list_via_elaborator`], feeding the resolved
//! sub-Dispatch carriers back through that walker's `results` channel in DFS order, then
//! hands the sealed `(name, KType)` pairs to a caller-supplied `finalize` that folds them
//! into the right carrier (`KType::Record`, `KFunction`, union schema, …).

use std::rc::Rc;

use crate::machine::core::kfunction::action::DepPlacement;
use crate::machine::core::{LexicalFrame, PendingBinderGuard};
use crate::machine::model::ast::KExpression;
use crate::machine::model::types::{
    parse_typed_field_list_via_elaborator, Elaborator, FieldListOutcome, FieldNameKind, ResultFeed,
};
use crate::machine::model::values::Carried;
use crate::machine::model::{KType, Record};
use crate::machine::{KError, KErrorKind, NodeId, Scope, TraceFrame};

use super::super::outcome::{dep_error_frame, Continuation, Outcome};
use super::super::DepFinish;
use super::DepRequest;
use super::SchedulerView;

/// Folds the elaborated `(name, KType)` pairs into the caller's carrier on the dep-finish's
/// `Done` arm. The scheduler-currency variant, returning [`Outcome`] — used by
/// [`defer_field_list`].
pub(crate) type FieldListFinalize<'run> = Box<
    dyn for<'step> FnOnce(&'step Scope<'run>, Vec<(String, KType<'run>)>) -> Outcome<'run>
        + 'run,
>;

/// `Action`-path twin of [`FieldListFinalize`], returning `Result<Carried, KError>` — used by
/// [`defer_field_list_action`], whose finish wraps the result in `Action::Done`.
pub(crate) type FieldListFinalizeAction<'run> = Box<
    dyn for<'step> FnOnce(
            &'step Scope<'run>,
            Vec<(String, KType<'run>)>,
        ) -> Result<Carried<'run>, KError>
        + 'run,
>;

/// Declare the sigil sub-Dispatches (in DFS order) and the dep-finish that re-walks `expr` once they
/// and `park_producers` resolve, as a [`Outcome::ParkThenContinue`] — a pure decide, no write.
/// `threaded` / `chain` rebuild the elaborator for the re-walk; `pending_guard` (when present)
/// rides into the closure so its Drop fires on every finish arm; `error_frame` is attached to the
/// user-facing `Err` arm.
#[allow(clippy::too_many_arguments)]
pub(crate) fn defer_field_list<'run>(
    expr: KExpression<'run>,
    park_producers: Vec<NodeId>,
    sub_dispatches: Vec<KExpression<'run>>,
    context: &'static str,
    name_kind: FieldNameKind,
    threaded: Vec<String>,
    chain: Option<Rc<LexicalFrame>>,
    pending_guard: Option<PendingBinderGuard<'run>>,
    error_frame: Option<TraceFrame>,
    finalize: FieldListFinalize<'run>,
) -> Outcome<'run> {
    let park_count = park_producers.len();
    let finish: DepFinish<'run> = Box::new(move |view, results| {
        // The guard's Drop clears the in-flight `pending_types` entry on every arm.
        let _pending_guard = pending_guard;
        // `results` = `[park results.. , owned-sub results..]`; the re-walk consumes only
        // the owned-sub carriers, in the DFS order they were scheduled above.
        let mut feed = ResultFeed::new(&results[park_count..]);
        let mut elaborator = Elaborator::new(view.current_scope())
            .with_threaded(threaded.iter().cloned())
            .with_chain(chain.clone());
        match parse_typed_field_list_via_elaborator(
            &expr,
            context,
            name_kind,
            &mut elaborator,
            Some(&mut feed),
        ) {
            FieldListOutcome::Done(fields) => finalize(view.current_scope(), fields),
            FieldListOutcome::Err(msg) => {
                let error = KError::new(KErrorKind::ShapeError(msg));
                Outcome::Done(Err(match error_frame {
                    Some(frame) => error.with_frame(frame),
                    None => error,
                }))
            }
            // Every producer waited on is terminal by dep-finish invariant, so a second
            // park is a scheduling inconsistency rather than a recoverable forward ref.
            FieldListOutcome::Pending { .. } => {
                Outcome::Done(Err(KError::new(KErrorKind::ShapeError(format!(
                    "{context}: forward type reference still unresolved after dep-finish wake"
                )))))
            }
        }
    });
    // Deps `[park_producers (Existing) ..., sigil subs (Dispatch/OwnScope) ...]`; the harness owns
    // the `Dispatch` suffix and parks the `Existing` prefix, feeding results in that order.
    let mut deps: Vec<DepRequest<'run>> = park_producers
        .into_iter()
        .map(DepRequest::Existing)
        .collect();
    deps.extend(sub_dispatches.into_iter().map(|sub| DepRequest::Dispatch {
        expr: sub,
        placement: DepPlacement::OwnScope,
    }));
    Outcome::ParkThenContinue {
        deps,
        park_count,
        cont: Continuation::Finish(finish),
        dep_error_frame: Some(dep_error_frame()),
    }
}

/// `Action`-harness twin of [`defer_field_list`]: build the same dep-finish as an
/// [`Action`](crate::machine::core::kfunction::action::Action) — park producers become
/// `Dep::Existing`, sigil sub-Dispatches `Dep::Dispatch { OwnScope }`, and the finish re-walks
/// `expr` then wraps the `finalize` result in `Action::Done`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn defer_field_list_action<'a>(
    expr: KExpression<'a>,
    park_producers: Vec<NodeId>,
    sub_dispatches: Vec<KExpression<'a>>,
    context: &'static str,
    name_kind: FieldNameKind,
    threaded: Vec<String>,
    chain: Option<Rc<LexicalFrame>>,
    pending_guard: Option<PendingBinderGuard<'a>>,
    error_frame: Option<TraceFrame>,
    finalize: FieldListFinalizeAction<'a>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::machine::core::kfunction::action::{Action, AwaitContinue, Dep, DepPlacement};
    // `deps` order [park ++ subs] makes the harness split owned = subs (DFS order), park =
    // park_producers, and the scheduler feeds results as [park.. , owned..] — so the re-walk
    // consumes `results[park_count..]`, exactly as the scheduler-side twin does.
    let park_count = park_producers.len();
    let mut deps: Vec<Dep<'a>> = park_producers.into_iter().map(Dep::Existing).collect();
    deps.extend(sub_dispatches.into_iter().map(|sub| Dep::Dispatch {
        expr: sub,
        placement: DepPlacement::OwnScope,
    }));
    let finish: AwaitContinue<'a> = Box::new(move |fctx, results| {
        // The guard's Drop clears the in-flight `pending_types` entry on every arm.
        let _pending_guard = pending_guard;
        let mut feed = ResultFeed::new(&results[park_count..]);
        let mut elaborator = Elaborator::new(fctx.scope)
            .with_threaded(threaded.iter().cloned())
            .with_chain(chain.clone());
        match parse_typed_field_list_via_elaborator(
            &expr,
            context,
            name_kind,
            &mut elaborator,
            Some(&mut feed),
        ) {
            FieldListOutcome::Done(fields) => Action::Done(finalize(fctx.scope, fields)),
            FieldListOutcome::Err(msg) => {
                let error = KError::new(KErrorKind::ShapeError(msg));
                Action::Done(Err(match error_frame {
                    Some(frame) => error.with_frame(frame),
                    None => error,
                }))
            }
            FieldListOutcome::Pending { .. } => {
                Action::Done(Err(KError::new(KErrorKind::ShapeError(format!(
                    "{context}: forward type reference still unresolved after dep-finish wake"
                )))))
            }
        }
    });
    Action::AwaitDeps { deps, finish }
}

/// Elaborate a standalone `:{…}` record type to `KObject::KTypeValue(KType::Record(_))`.
/// The `fields` expression is the record's `(name :Type, …)` field list. A record type at a
/// value/type position declares no binder, so the elaborator threads no self-reference; a
/// field naming a forward type parks and a sigil field type sub-dispatches, both deferred
/// through one dep-finish (the field walker's own re-walk handles nested records).
pub(crate) fn elaborate_record_value<'run, 'view>(
    view: &SchedulerView<'run, 'view>,
    fields: KExpression<'run>,
    chain: Option<Rc<LexicalFrame>>,
) -> Outcome<'run> {
    fn fold<'run>(scope: &Scope<'run>, pairs: Vec<(String, KType<'run>)>) -> Outcome<'run> {
        let record = Record::from_pairs(pairs);
        let kt = scope.arena.alloc_ktype(KType::Record(Box::new(record)));
        Outcome::Done(Ok(Carried::Type(kt)))
    }
    let mut elaborator = Elaborator::new(view.current_scope()).with_chain(chain.clone());
    match parse_typed_field_list_via_elaborator(
        &fields,
        "record fields",
        FieldNameKind::Identifier,
        &mut elaborator,
        None,
    ) {
        FieldListOutcome::Done(pairs) => fold(view.current_scope(), pairs),
        FieldListOutcome::Err(msg) => Outcome::Done(Err(KError::new(KErrorKind::ShapeError(msg)))),
        FieldListOutcome::Pending {
            park_producers,
            sub_dispatches,
        } => defer_field_list(
            fields,
            park_producers,
            sub_dispatches,
            "record fields",
            FieldNameKind::Identifier,
            Vec::new(),
            chain,
            None,
            None,
            Box::new(fold),
        ),
    }
}
