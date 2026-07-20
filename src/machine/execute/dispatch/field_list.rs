//! Shared deferral for typed field lists whose elaboration parked on a forward type or
//! scheduled sub-Dispatches for sigil field types — FN parameter lists, the
//! NEWTYPE record repr, the UNION schema, and the standalone record-type sigil.
//!
//! One dep-finish waits on `[park_producers ++ owned_subs]`; its finish re-walks the field
//! list through [`parse_typed_field_list_via_elaborator`], feeding the resolved
//! sub-Dispatch carriers back through that walker's `results` channel in DFS order. Two
//! composition surfaces consume the resulting `(name, KType)` pairs:
//!
//! - the record-type sigil and the FN carrier compose through [`compose_field_list`] and a
//!   [`BrandCompose`] closure, which assembles one owned `KType` and allocates it into the
//!   consumer's own region;
//! - the UNION schema and the NEWTYPE record repr hand the pairs to a caller-supplied
//!   [`FieldListFinalizeAction`], which seals them through the declaration window into interned
//!   member handles and crosses the nominal identity through
//!   [`seal_type_identity`](super::constructors::seal_type_identity).

use std::rc::Rc;

use crate::machine::core::{DepPlacement, FinishCtx};
use crate::machine::core::{LexicalFrame, PendingBinderGuard, StepAllocator};
use crate::machine::model::Carried;
use crate::machine::model::KExpression;
use crate::machine::model::{
    parse_typed_field_list_via_elaborator, Elaborator, FieldListContext, FieldListOutcome,
    FieldNameKind, ResultFeed,
};
use crate::machine::model::{KType, Record, TypeRegistry};
use crate::machine::{KError, KErrorKind, NodeId, Scope, TraceFrame};
use crate::scheduler::Deps;

use super::super::outcome::{dep_error_frame, Await, Outcome};
use super::super::StepCarried;
use super::super::TerminalDepFinish;
use super::DepRequest;
use super::SchedulerView;
use crate::machine::DeliveredCarried;

/// Composes the final `KType` from the elaborated pairs, plus whatever owned type content the
/// caller closed over (e.g. the FN return type). Runs in [`compose_field_list`], which allocates
/// the composed value into the consumer's own region through the single type door.
pub(crate) type BrandCompose<'step> = Box<
    dyn for<'r> FnOnce(Vec<(String, KType)>, &'r TypeRegistry) -> Result<KType, KError> + 'step,
>;

/// `Action`-path finalize, returning a witnessed carrier — used by
/// [`defer_field_list_action`], whose finish lifts the result straight into
/// [`Action::Done(Ok)`](crate::machine::core::Action::Done). Takes the
/// [`FinishCtx`] the `AwaitContinue` wrapper already holds, for the same reason.
pub(crate) type FieldListFinalizeAction<'a> = Box<
    dyn for<'r> FnOnce(
            &FinishCtx<'a, 'r>,
            Vec<(String, KType)>,
            &[&DeliveredCarried],
        ) -> Result<StepCarried<'a>, KError>
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
    context: FieldListContext,
    name_kind: FieldNameKind,
    threaded: Vec<String>,
    window: Option<std::rc::Rc<crate::machine::model::RecursiveGroupWindow>>,
    chain: Option<Rc<LexicalFrame>>,
    error_frame: Option<TraceFrame>,
}

impl<'step> FieldListRewalk<'step> {
    /// Re-walk the field list: the sub-Dispatch results arrive as `feed`, and each elaborated field
    /// type is cloned out as owned data. The expression stays at `'step` (only walked, never
    /// embedded), while the output pairs are owned `KType`s; the parser carries the two lifetimes
    /// separately so they can diverge. `ResultFeed` is always installed: a
    /// `Done`-shaped walk never pops it, and a popped-dry feed hits the loud "fewer resolved
    /// sub-dispatches" error inside the walker.
    fn run<'b>(
        self,
        scope: &Scope<'b>,
        feed: &[Carried<'b>],
        types: &TypeRegistry,
    ) -> Result<Vec<(String, KType)>, KError> {
        let mut result_feed = ResultFeed::new(feed);
        let mut elaborator = Elaborator::new(scope)
            .with_threaded(self.threaded.iter().cloned())
            .with_chain(self.chain.clone());
        if let Some(window) = self.window.clone() {
            elaborator = elaborator.with_window(window);
        }
        match parse_typed_field_list_via_elaborator(
            &self.expr,
            self.context,
            self.name_kind,
            &mut elaborator,
            Some(&mut result_feed),
            types,
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
                self.context.list
            )))),
        }
    }
}

/// The ONE construction site both deferral currencies call: re-walk the field list against the
/// resolved sub-Dispatch results, compose the result `KType` from the owned pairs, and allocate it
/// into the consumer's own region through [`StepAllocator::alloc_type`].
///
/// `feed` is the owned suffix of the dep terminals in DFS order — the parks are notify-only waits
/// on a forward reference, so they never reach the walk. Every field type the walk produces is
/// owned data, so the composed type embeds no borrow of a producer region.
fn compose_field_list<'step>(
    step_ctx: &StepAllocator<'step>,
    scope: &'step Scope<'step>,
    rewalk: FieldListRewalk<'step>,
    feed: &[Carried<'step>],
    compose: BrandCompose<'step>,
    types: &TypeRegistry,
) -> Result<StepCarried<'step>, KError> {
    let fields = rewalk.run(scope, feed, types)?;
    Ok(step_ctx.alloc_type(compose(fields, types)?))
}

/// Declare the sigil sub-Dispatches (in DFS order) and the dep-finish that re-walks `expr` once they
/// and `park_producers` resolve, as a [`Outcome::ParkThenContinue`] — a pure decide, no write.
/// `threaded` / `window` / `chain` rebuild the elaborator for the re-walk — the window is the
/// declaration window the first walk minted its sibling handles against, so the re-walk mints the
/// same indices; `pending_guard` (when present)
/// rides into the closure so its Drop fires on every finish arm; `error_frame` is attached to the
/// user-facing `Err` arm.
#[allow(clippy::too_many_arguments)]
pub(crate) fn defer_field_list<'step>(
    expr: KExpression<'step>,
    park_producers: Vec<NodeId>,
    sub_dispatches: Vec<KExpression<'step>>,
    context: FieldListContext,
    name_kind: FieldNameKind,
    threaded: Vec<String>,
    window: Option<std::rc::Rc<crate::machine::model::RecursiveGroupWindow>>,
    chain: Option<Rc<LexicalFrame>>,
    pending_guard: Option<PendingBinderGuard>,
    error_frame: Option<TraceFrame>,
    compose: BrandCompose<'step>,
) -> Outcome<'step> {
    let rewalk = FieldListRewalk {
        expr,
        context,
        name_kind,
        threaded,
        window,
        chain,
        error_frame,
    };
    let finish: TerminalDepFinish<'step> = Box::new(move |view, terminals| {
        // The guard's Drop clears the in-flight `pending_types` entry on every arm.
        let _pending_guard = pending_guard;
        // The owned suffix — each sub-Dispatch's terminal read live at the step brand — is the
        // walk's feed; the parks are notify-only waits on a forward reference. Each field type the
        // walk yields is cloned out as owned data, so the composed type needs no operand fold.
        let owned: Vec<Carried<'step>> = terminals.owned_slice().iter().map(|t| t.value).collect();
        match compose_field_list(
            &view.step_ctx(),
            view.current_scope(),
            rewalk,
            &owned,
            compose,
            view.types(),
        ) {
            Ok(sealed) => Outcome::Done(Ok(sealed)),
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
/// but wraps the dep-finish as an [`Action`](crate::machine::core::Action) — its
/// re-walk of `expr` lifts the `finalize` result into `Action::Done`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn defer_field_list_action<'a>(
    expr: KExpression<'a>,
    park_producers: Vec<NodeId>,
    sub_dispatches: Vec<KExpression<'a>>,
    context: FieldListContext,
    name_kind: FieldNameKind,
    threaded: Vec<String>,
    window: Option<std::rc::Rc<crate::machine::model::RecursiveGroupWindow>>,
    chain: Option<Rc<LexicalFrame>>,
    pending_guard: Option<PendingBinderGuard>,
    error_frame: Option<TraceFrame>,
    finalize: FieldListFinalizeAction<'a>,
) -> crate::machine::core::Action<'a> {
    use crate::machine::core::{Action, AwaitContinue};
    // `deps` order [park ++ subs] makes the harness split owned = subs (DFS order), park =
    // park_producers; the scheduler feeds results as [park.. , owned..] — so the re-walk consumes
    // the owned suffix, exactly as the scheduler-side twin does.
    let deps = field_list_deps(park_producers, sub_dispatches);
    let rewalk = FieldListRewalk {
        expr,
        context,
        name_kind,
        threaded,
        window,
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
        let carriers: Vec<&DeliveredCarried> = results.all().iter().map(|t| &t.delivered).collect();
        let owned: Vec<Carried<'a>> = results.owned_slice().iter().map(|t| t.value).collect();
        Action::Done(
            rewalk
                .run(fctx.scope, &owned, fctx.types)
                .and_then(|fields| finalize(fctx, fields, &carriers)),
        )
    });
    Action::AwaitDeps { deps, finish }
}

/// Composed twin of [`defer_field_list_action`]: declares the identical [`field_list_deps`] vector,
/// but its finish runs the re-walk through [`compose_field_list`] rather than a caller-supplied
/// `finalize` over the dep carriers. `compose` assembles the elaborated pairs — plus whatever owned
/// type content it closed over, such as the FN return type — into the result `KType`. Used by
/// `build_carrier` (`src/builtins/parameterized_types.rs`); `nominal_schema` keeps the
/// `finalize` twin.
#[allow(clippy::too_many_arguments)]
pub(crate) fn defer_field_list_action_composed<'a>(
    expr: KExpression<'a>,
    park_producers: Vec<NodeId>,
    sub_dispatches: Vec<KExpression<'a>>,
    context: FieldListContext,
    name_kind: FieldNameKind,
    threaded: Vec<String>,
    window: Option<std::rc::Rc<crate::machine::model::RecursiveGroupWindow>>,
    chain: Option<Rc<LexicalFrame>>,
    pending_guard: Option<PendingBinderGuard>,
    error_frame: Option<TraceFrame>,
    compose: BrandCompose<'a>,
) -> crate::machine::core::Action<'a> {
    use crate::machine::core::{Action, AwaitContinue};
    // `deps` order [park ++ subs] makes the harness split owned = subs (DFS order), park =
    // park_producers; the scheduler feeds results as [park.. , owned..].
    let deps = field_list_deps(park_producers, sub_dispatches);
    let rewalk = FieldListRewalk {
        expr,
        context,
        name_kind,
        threaded,
        window,
        chain,
        error_frame,
    };
    let finish: AwaitContinue<'a> = Box::new(move |fctx, results| {
        // The guard's Drop clears the in-flight `pending_types` entry on every arm.
        let _pending_guard = pending_guard;
        // The owned suffix — each sub-Dispatch's terminal read live at the step brand — is the
        // walk feed; the parks are notify-only waits on a forward reference. Each field type the
        // walk yields is cloned out as owned data, so the composed type needs no operand fold.
        let owned: Vec<Carried<'a>> = results.owned_slice().iter().map(|t| t.value).collect();
        Action::Done(compose_field_list(
            &fctx.ctx, fctx.scope, rewalk, &owned, compose, fctx.types,
        ))
    });
    Action::AwaitDeps { deps, finish }
}

/// Elaborate a standalone `:{…}` record type to `Carried::Type(KType::Record { .. })`.
/// The `fields` expression is the record's `(name :Type, …)` field list. A record type at a
/// value/type position declares no binder, so the elaborator threads no self-reference; a
/// field naming a forward type parks and a sigil field type sub-dispatches, both deferred
/// through one dep-finish (the field walker's own re-walk handles nested records).
pub(crate) fn elaborate_record_value<'step, 'view>(
    view: &SchedulerView<'step, 'view>,
    fields: KExpression<'step>,
    chain: Option<Rc<LexicalFrame>>,
) -> Outcome<'step> {
    let mut elaborator = Elaborator::new(view.current_scope()).with_chain(chain.clone());
    match parse_typed_field_list_via_elaborator(
        &fields,
        FieldListContext::RECORD_TYPE,
        FieldNameKind::Identifier,
        &mut elaborator,
        None,
        view.types(),
    ) {
        FieldListOutcome::Done(pairs) => {
            let kt = view.types().record(Record::from_pairs(pairs));
            Outcome::Done(Ok(view.step_ctx().alloc_type(kt)))
        }
        FieldListOutcome::Err(msg) => Outcome::Done(Err(KError::new(KErrorKind::ShapeError(msg)))),
        FieldListOutcome::Pending {
            park_producers,
            sub_dispatches,
        } => defer_field_list(
            fields,
            park_producers,
            sub_dispatches,
            FieldListContext::RECORD_TYPE,
            FieldNameKind::Identifier,
            Vec::new(),
            None,
            chain,
            None,
            None,
            Box::new(|pairs, types| Ok(types.record(Record::from_pairs(pairs)))),
        ),
    }
}
