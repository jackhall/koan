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

use crate::machine::core::kfunction::action::{DepPlacement, FinishCtx};
use crate::machine::core::{KoanStepContextExt, LexicalFrame, PendingBinderGuard};
use crate::machine::model::ast::KExpression;
use crate::machine::model::types::{
    parse_typed_field_list_via_elaborator, Elaborator, FieldListOutcome, FieldNameKind, ResultFeed,
};
use crate::machine::model::values::{Carried, CarriedFamily};
use crate::machine::model::{KType, Record};
use crate::machine::{CarrierWitness, KError, KErrorKind, NodeId, Scope, TraceFrame};
use crate::scheduler::Deps;
use crate::witnessed::Witnessed;

use super::super::outcome::{dep_error_frame, Await, Outcome};
use super::super::TerminalDepFinish;
use super::DepRequest;
use super::SchedulerView;
use crate::machine::DeliveredCarried;

/// Folds the elaborated `(name, KType)` pairs into the caller's carrier on the dep-finish's
/// `Done` arm. The scheduler-currency variant, returning [`Outcome`] — used by
/// [`defer_field_list`]. Takes the [`SchedulerView`] (not a bare `Scope`) so a `finalize` that
/// builds a born-pure carrier can construct it through `view.step_ctx().alloc_carried(…)`.
pub(crate) type FieldListFinalize<'step> = Box<
    dyn for<'view> FnOnce(
            &SchedulerView<'step, 'view>,
            Vec<(String, KType<'step>)>,
            &[&DeliveredCarried],
        ) -> Outcome<'step>
        + 'step,
>;

/// `Action`-path twin of [`FieldListFinalize`], returning a witnessed carrier — used by
/// [`defer_field_list_action`], whose finish lifts the result straight into
/// [`Action::Done(Ok)`](crate::machine::core::kfunction::action::Action::Done). Takes the
/// [`FinishCtx`] the `AwaitContinue` wrapper already holds, for the same reason.
pub(crate) type FieldListFinalizeAction<'a> = Box<
    dyn FnOnce(
            &FinishCtx<'a>,
            Vec<(String, KType<'a>)>,
            &[&DeliveredCarried],
        ) -> Result<Witnessed<CarriedFamily, CarrierWitness>, KError>
        + 'a,
>;

/// The `[park_producers (Existing) ..., sigil subs (Dispatch/OwnScope) ...]` dep vector the
/// `Action` deferral twin declares — `run_action` parks the `Existing` prefix and owns the
/// `Dispatch` suffix, so the re-walk consumes the owned suffix in DFS order. The scheduler twin
/// builds its `Deps` directly.
fn field_list_deps<'step>(
    park_producers: Vec<NodeId>,
    sub_dispatches: Vec<KExpression<'step>>,
) -> Vec<DepRequest<'step>> {
    let mut deps: Vec<DepRequest<'step>> = park_producers
        .into_iter()
        .map(DepRequest::Existing)
        .collect();
    deps.extend(sub_dispatches.into_iter().map(|sub| DepRequest::Dispatch {
        expr: sub,
        placement: DepPlacement::OwnScope,
    }));
    deps
}

/// The deferred re-walk both currencies run once their deps resolve: rebuild the elaborator, feed the
/// owned sub-Dispatch carriers back through the field walker in DFS order, and produce the
/// `(name, KType)` pairs. The re-walk consumes only the owned suffix (`park_producers` are notify-only
/// forward-ref waits). The `Err` arm labels a shape error with `error_frame`; a still-`Pending` walk is
/// a scheduling inconsistency (every producer waited on is terminal by the dep-finish invariant, so a
/// second park is not a recoverable forward ref) and errors loudly.
struct FieldListRewalk<'step> {
    expr: KExpression<'step>,
    context: &'static str,
    name_kind: FieldNameKind,
    threaded: Vec<String>,
    chain: Option<Rc<LexicalFrame>>,
    error_frame: Option<TraceFrame>,
}

impl<'step> FieldListRewalk<'step> {
    fn run(
        self,
        scope: &Scope<'step>,
        owned: &[Carried<'step>],
    ) -> Result<Vec<(String, KType<'step>)>, KError> {
        let mut feed = ResultFeed::new(owned);
        let mut elaborator = Elaborator::new(scope)
            .with_threaded(self.threaded.iter().cloned())
            .with_chain(self.chain.clone());
        match parse_typed_field_list_via_elaborator(
            &self.expr,
            self.context,
            self.name_kind,
            &mut elaborator,
            Some(&mut feed),
        ) {
            FieldListOutcome::Done(fields) => Ok(fields),
            FieldListOutcome::Err(msg) => {
                let error = KError::new(KErrorKind::ShapeError(msg));
                Err(match self.error_frame {
                    Some(frame) => error.with_frame(frame),
                    None => error,
                })
            }
            FieldListOutcome::Pending { .. } => Err(KError::new(KErrorKind::ShapeError(format!(
                "{}: forward type reference still unresolved after dep-finish wake",
                self.context
            )))),
        }
    }
}

/// Declare the sigil sub-Dispatches (in DFS order) and the dep-finish that re-walks `expr` once they
/// and `park_producers` resolve, as a [`Outcome::ParkThenContinue`] — a pure decide, no write.
/// `threaded` / `chain` rebuild the elaborator for the re-walk; `pending_guard` (when present)
/// rides into the closure so its Drop fires on every finish arm; `error_frame` is attached to the
/// user-facing `Err` arm.
#[allow(clippy::too_many_arguments)]
pub(crate) fn defer_field_list<'step>(
    expr: KExpression<'step>,
    park_producers: Vec<NodeId>,
    sub_dispatches: Vec<KExpression<'step>>,
    context: &'static str,
    name_kind: FieldNameKind,
    threaded: Vec<String>,
    chain: Option<Rc<LexicalFrame>>,
    pending_guard: Option<PendingBinderGuard<'step>>,
    error_frame: Option<TraceFrame>,
    finalize: FieldListFinalize<'step>,
) -> Outcome<'step> {
    let rewalk = FieldListRewalk {
        expr,
        context,
        name_kind,
        threaded,
        chain,
        error_frame,
    };
    let finish: TerminalDepFinish<'step> = Box::new(move |view, terminals| {
        // The guard's Drop clears the in-flight `pending_types` entry on every arm.
        let _pending_guard = pending_guard;
        // Every terminal's carrier — parks then owned — folds into the result so a field type
        // that embeds a park's forward-referenced type or an owned sub-Dispatch's type carries
        // that producer's reach forward; the owned values, read live at the step brand
        // (un-relocated), feed the re-walk, which clones each type into the folded field list.
        let carriers: Vec<&DeliveredCarried> =
            terminals.all().iter().map(|t| &t.delivered).collect();
        let owned: Vec<Carried<'step>> = terminals.owned_slice().iter().map(|t| t.value).collect();
        match rewalk.run(view.current_scope(), &owned) {
            Ok(fields) => finalize(view, fields, &carriers),
            Err(e) => Outcome::Done(Err(e)),
        }
    });
    // Parks the forward-ref producers; owns each sigil sub-Dispatch (in DFS order). The finish reads
    // only the owned suffix through the view.
    let mut deps = Deps::from_parks(park_producers);
    for sub in sub_dispatches {
        deps.own(DepRequest::Dispatch {
            expr: sub,
            placement: DepPlacement::OwnScope,
        });
    }
    Await::on(deps)
        .error_frame(dep_error_frame())
        .finish_terminal(finish)
}

/// `Action`-harness twin of [`defer_field_list`]: declares the identical [`field_list_deps`] vector
/// but wraps the dep-finish as an [`Action`](crate::machine::core::kfunction::action::Action) — its
/// re-walk of `expr` lifts the `finalize` result into `Action::Done`.
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
    use crate::machine::core::kfunction::action::{Action, AwaitContinue};
    // `deps` order [park ++ subs] makes the harness split owned = subs (DFS order), park =
    // park_producers; the scheduler feeds results as [park.. , owned..] — so the re-walk consumes
    // the owned suffix, exactly as the scheduler-side twin does.
    let deps = field_list_deps(park_producers, sub_dispatches);
    let rewalk = FieldListRewalk {
        expr,
        context,
        name_kind,
        threaded,
        chain,
        error_frame,
    };
    let finish: AwaitContinue<'a> = Box::new(move |fctx, results| {
        // The guard's Drop clears the in-flight `pending_types` entry on every arm.
        let _pending_guard = pending_guard;
        // Every terminal's carrier — parks then owned — folds into the result so a field type
        // that embeds a park's forward-referenced type or an owned sub-Dispatch's type carries
        // that producer's reach forward; the owned values, read live at the step brand
        // (un-relocated), feed the re-walk, which clones each type into the folded field list.
        let carriers: Vec<&DeliveredCarried> =
            results.all().iter().map(|t| &t.delivered).collect();
        let owned: Vec<Carried<'a>> = results.owned_slice().iter().map(|t| t.value).collect();
        Action::Done(
            rewalk
                .run(fctx.scope, &owned)
                .and_then(|fields| finalize(fctx, fields, &carriers)),
        )
    });
    Action::AwaitDeps { deps, finish }
}

/// Elaborate a standalone `:{…}` record type to `Carried::Type(KType::Record(_))`.
/// The `fields` expression is the record's `(name :Type, …)` field list. A record type at a
/// value/type position declares no binder, so the elaborator threads no self-reference; a
/// field naming a forward type parks and a sigil field type sub-dispatches, both deferred
/// through one dep-finish (the field walker's own re-walk handles nested records).
pub(crate) fn elaborate_record_value<'step, 'view>(
    view: &SchedulerView<'step, 'view>,
    fields: KExpression<'step>,
    chain: Option<Rc<LexicalFrame>>,
) -> Outcome<'step> {
    fn fold<'step>(
        view: &SchedulerView<'step, '_>,
        pairs: Vec<(String, KType<'step>)>,
        carriers: &[&DeliveredCarried],
    ) -> Outcome<'step> {
        let record = Record::from_pairs(pairs);
        Outcome::Done(Ok(view
            .step_ctx()
            .alloc_type_with(carriers, KType::Record(Box::new(record)))))
    }
    let mut elaborator = Elaborator::new(view.current_scope()).with_chain(chain.clone());
    match parse_typed_field_list_via_elaborator(
        &fields,
        "record fields",
        FieldNameKind::Identifier,
        &mut elaborator,
        None,
    ) {
        FieldListOutcome::Done(pairs) => fold(view, pairs, &[]),
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
