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
use crate::machine::core::{
    FoldingBrand, FrameStorage, KoanStepContextExt, LexicalFrame, PendingBinderGuard,
};
use crate::machine::model::ast::KExpression;
use crate::machine::model::types::{
    parse_typed_field_list_via_elaborator, Elaborator, FieldListOutcome, FieldNameKind, ResultFeed,
};
use crate::machine::model::values::{Carried, CarriedFamily};
use crate::machine::model::{KType, Record};
use crate::machine::{CarrierWitness, KError, KErrorKind, NodeId, Scope, TraceFrame};
use crate::scheduler::Deps;
use crate::witnessed::{StepContext, Witnessed};

use super::super::outcome::{dep_error_frame, Await, Outcome};
use super::super::TerminalDepFinish;
use super::DepRequest;
use super::SchedulerView;
use crate::machine::DeliveredCarried;

/// Composes the final `KType` at the fold brand from the elaborated pairs and any extra operand
/// views (e.g. the FN/FUNCTOR return type's carrier view). Runs inside the fold closure of
/// [`fold_fields_at_brand`]; everything it builds from is a declared operand of that fold — the
/// pairs cloned out of brand-delivered views, plus the `extras` views.
pub(crate) type BrandCompose<'step> = Box<
    dyn for<'b> FnOnce(
            FoldingBrand<'b>,
            Vec<(String, KType<'b>)>,
            &[Carried<'b>],
        ) -> Result<KType<'b>, KError>
        + 'step,
>;

/// `Action`-path finalize, returning a witnessed carrier — used by
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
    /// Re-walk the field list at the fold brand `'b`: the sub-Dispatch carriers arrive as brand
    /// views in `feed`, and each elaborated field type is cloned out at `'b`. The expression stays
    /// at `'step` (only walked, never embedded), while the output pairs are `KType<'b>` — phase 1's
    /// lifetime split is what lets these two diverge. `ResultFeed` is always installed: a
    /// `Done`-shaped walk never pops it, and a popped-dry feed hits the loud "fewer resolved
    /// sub-dispatches" error inside the walker.
    fn run<'b>(
        self,
        scope: &Scope<'b>,
        feed: &[Carried<'b>],
    ) -> Result<Vec<(String, KType<'b>)>, KError> {
        let mut result_feed = ResultFeed::new(feed);
        let mut elaborator = Elaborator::new(scope)
            .with_threaded(self.threaded.iter().cloned())
            .with_chain(self.chain.clone());
        match parse_typed_field_list_via_elaborator(
            &self.expr,
            self.context,
            self.name_kind,
            &mut elaborator,
            Some(&mut result_feed),
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

/// The ONE at-brand construction site both deferral currencies call: fold `[carriers ++ extras]`
/// and the consumer scope through the phase-2 door, then — inside the fold closure, at the store's
/// own brand `'b` — re-walk the field list against the brand-delivered feed views, compose the
/// result `KType`, and store it folded.
///
/// Dep ORDER contract: `deps` is `[parks.., owned.., extras..]`. `carriers` is `[parks.., owned..]`
/// in terminal order, `park_count` splits its park prefix from its owned suffix, and `extras` are
/// compose-only operands (e.g. the FN/FUNCTOR return type's carrier). Inside the fold the walk feed
/// is `views[park_count..carriers.len()]` (the owned suffix), and the extras are
/// `views[carriers.len()..]`. Every operand's reach and residence host fold into the result's
/// witness, so a field or return type reaching a producer region carries that reach forward.
fn fold_fields_at_brand<'step>(
    step_ctx: &StepContext<FrameStorage>,
    scope: &'step Scope<'step>,
    rewalk: FieldListRewalk<'step>,
    carriers: &[&DeliveredCarried],
    park_count: usize,
    extras: &[&DeliveredCarried],
    compose: BrandCompose<'step>,
) -> Result<Witnessed<CarriedFamily, CarrierWitness>, KError> {
    let deps: Vec<&DeliveredCarried> = carriers.iter().chain(extras).copied().collect();
    let walk_len = carriers.len();
    // The fold closure must return a `Carried`, so a walk/compose error is stashed here and
    // surfaced after the alloc, storing a throwaway placeholder in its place.
    let mut error: Option<KError> = None;
    let sealed = step_ctx.alloc_carried_with_scope(&deps, scope, |brand, views, scope| {
        let feed_views = &views[park_count..walk_len];
        let extra_views = &views[walk_len..];
        match rewalk
            .run(scope, feed_views)
            .and_then(|fields| compose(brand, fields, extra_views))
        {
            Ok(kt) => Carried::Type(brand.alloc_ktype_folded(kt)),
            Err(e) => {
                error = Some(e);
                Carried::Type(brand.alloc_ktype_folded(KType::Any))
            }
        }
    });
    match error {
        Some(e) => Err(e),
        None => Ok(sealed),
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
    extras: Vec<DeliveredCarried>,
    compose: BrandCompose<'step>,
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
        // Every terminal's carrier — parks then owned — folds into the result at the store's own
        // brand, so a field type that embeds a park's forward-referenced type or an owned
        // sub-Dispatch's type carries that producer's reach forward. The owned suffix is the walk's
        // feed; the parks are notify-only. `fold_fields_at_brand` re-walks against the brand views
        // rather than the ambient step lifetime.
        let carriers: Vec<&DeliveredCarried> =
            terminals.all().iter().map(|t| &t.delivered).collect();
        let park_count = carriers.len() - terminals.owned_slice().len();
        let extra_refs: Vec<&DeliveredCarried> = extras.iter().collect();
        match fold_fields_at_brand(
            &view.step_ctx(),
            view.current_scope(),
            rewalk,
            &carriers,
            park_count,
            &extra_refs,
            compose,
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
        let carriers: Vec<&DeliveredCarried> = results.all().iter().map(|t| &t.delivered).collect();
        let owned: Vec<Carried<'a>> = results.owned_slice().iter().map(|t| t.value).collect();
        Action::Done(
            rewalk
                .run(fctx.scope, &owned)
                .and_then(|fields| finalize(fctx, fields, &carriers)),
        )
    });
    Action::AwaitDeps { deps, finish }
}

/// Composed twin of [`defer_field_list_action`]: declares the identical [`field_list_deps`] vector,
/// but its finish runs the re-walk at the store's own fold brand through [`fold_fields_at_brand`]
/// rather than folding an ambient `finalize`. `extras` are compose-only operand carriers (e.g. the
/// FN/FUNCTOR return type's carrier), and `compose` folds the elaborated pairs plus those extra
/// brand views into the result `KType` inside the fold closure. Used by `build_carrier`
/// (`src/builtins/parameterized_types.rs`); `nominal_schema` keeps the ambient-`finalize` twin.
#[allow(clippy::too_many_arguments)]
pub(crate) fn defer_field_list_action_composed<'a>(
    expr: KExpression<'a>,
    park_producers: Vec<NodeId>,
    sub_dispatches: Vec<KExpression<'a>>,
    context: &'static str,
    name_kind: FieldNameKind,
    threaded: Vec<String>,
    chain: Option<Rc<LexicalFrame>>,
    pending_guard: Option<PendingBinderGuard<'a>>,
    error_frame: Option<TraceFrame>,
    extras: Vec<DeliveredCarried>,
    compose: BrandCompose<'a>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::machine::core::kfunction::action::{Action, AwaitContinue};
    // `deps` order [park ++ subs] makes the harness split owned = subs (DFS order), park =
    // park_producers; the scheduler feeds results as [park.. , owned..].
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
        // Every terminal's carrier — parks then owned — folds into the result at the store's own
        // brand; the owned suffix is the walk feed, the parks are notify-only, and `extras` are
        // compose-only operands. `fold_fields_at_brand` re-walks against the brand views.
        let carriers: Vec<&DeliveredCarried> = results.all().iter().map(|t| &t.delivered).collect();
        let park_count = carriers.len() - results.owned_slice().len();
        let extra_refs: Vec<&DeliveredCarried> = extras.iter().collect();
        Action::Done(fold_fields_at_brand(
            &fctx.ctx,
            fctx.scope,
            rewalk,
            &carriers,
            park_count,
            &extra_refs,
            compose,
        ))
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
    let mut elaborator = Elaborator::new(view.current_scope()).with_chain(chain.clone());
    match parse_typed_field_list_via_elaborator(
        &fields,
        "record fields",
        FieldNameKind::Identifier,
        &mut elaborator,
        None,
    ) {
        // Sync `Done`: the single ambient walk resolved everything, so store the composed record
        // through the audited non-fold tier (`alloc_type_pure`) — no folded placement. Migrating
        // synchronous composition onto a brand is `checked-tier-confinement`'s job, not this one's.
        FieldListOutcome::Done(pairs) => Outcome::Done(
            view.step_ctx()
                .alloc_type_pure(KType::Record(Box::new(Record::from_pairs(pairs)))),
        ),
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
            Vec::new(),
            Box::new(|_brand, pairs, _extras| Ok(KType::Record(Box::new(Record::from_pairs(pairs))))),
        ),
    }
}
