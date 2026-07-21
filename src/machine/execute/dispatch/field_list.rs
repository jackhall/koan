//! Shared deferral for typed field lists whose elaboration parked on a forward type or
//! scheduled sub-Dispatches for sigil field types — FN parameter lists, the
//! NEWTYPE record repr, the UNION schema, and the standalone record-type sigil.
//!
//! [`FieldListDeferral`] bundles the parked producers, the sigil sub-Dispatches, and the elaborator
//! state a re-walk needs; its three consuming finish methods each declare one dep-finish that waits
//! on `[park_producers ++ owned_subs]` and re-walks the field list through
//! [`parse_typed_field_list_via_elaborator`], feeding the resolved sub-Dispatch carriers back through
//! that walker's `results` channel in DFS order. Two composition surfaces consume the resulting
//! `(name, KType)` pairs:
//!
//! - [`FieldListDeferral::outcome`] (the record-type sigil) and [`FieldListDeferral::action_composed`]
//!   (the FN carrier) compose through a [`BrandCompose`] closure, which assembles one owned `KType`
//!   and allocates it into the consumer's own region;
//! - [`FieldListDeferral::action`] (the UNION schema and the NEWTYPE record repr) hands the pairs to a
//!   caller-supplied [`FieldListFinalizeAction`], which seals them through the declaration window into
//!   interned member handles and crosses the nominal identity through
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
use super::OwnedDispatch;
use super::SchedulerView;

/// Composes the final `KType` from the elaborated pairs, plus whatever owned type content the
/// caller closed over (e.g. the FN return type). Runs in [`compose_field_list`], which allocates
/// the composed value into the consumer's own region through the single type door.
pub(crate) type BrandCompose<'step> = Box<
    dyn for<'r> FnOnce(Vec<(String, KType)>, &'r TypeRegistry) -> Result<KType, KError> + 'step,
>;

/// `Action`-path finalize, returning a witnessed carrier — used by
/// [`FieldListDeferral::action`], whose finish lifts the result straight into
/// [`Action::Done(Ok)`](crate::machine::core::Action::Done). Takes the
/// [`FinishCtx`] the `AwaitContinue` wrapper already holds, for the same reason.
pub(crate) type FieldListFinalizeAction<'a> = Box<
    dyn for<'r> FnOnce(&FinishCtx<'a, 'r>, Vec<(String, KType)>) -> Result<StepCarried<'a>, KError>
        + 'a,
>;

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

/// The construction site the scheduler-currency finish calls: re-walk the field list against the
/// resolved sub-Dispatch results, compose the result `KType` from the interned pairs, and carry the
/// handle through [`StepAllocator::type_carried`]. The `Action`-currency finish composes through the
/// [`FieldListDeferral::action_composed`] adapter, which wraps this same step around a
/// [`FinishCtx`]'s allocator.
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
    Ok(step_ctx.type_carried(compose(fields, types)?))
}

/// One field-list deferral, ready to finish into either dispatch currency. Holds the parked
/// forward-ref producers, the sigil sub-Dispatches (DFS order), and the elaborator state a re-walk
/// rebuilds; the required fields are set at [`new`](Self::new) and the optionals thread in through the
/// `with_*` setters. The three consuming finish methods each assemble the shared
/// `[park_producers ++ owned_subs]` dep vector once through [`into_parts`](Self::into_parts).
pub(crate) struct FieldListDeferral<'a> {
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
}

impl<'a> FieldListDeferral<'a> {
    /// The five fields every deferral names: the parked field-list `expr`, its forward-ref
    /// `park_producers`, the sigil `sub_dispatches` (DFS order), and the `context` / `name_kind`
    /// diagnostic and field-name policy. The elaborator-rebuild optionals default empty/absent.
    pub(crate) fn new(
        expr: KExpression<'a>,
        park_producers: Vec<NodeId>,
        sub_dispatches: Vec<KExpression<'a>>,
        context: FieldListContext,
        name_kind: FieldNameKind,
    ) -> Self {
        Self {
            expr,
            park_producers,
            sub_dispatches,
            context,
            name_kind,
            threaded: Vec::new(),
            window: None,
            chain: None,
            pending_guard: None,
            error_frame: None,
        }
    }

    /// Seed the re-walk's threaded self-reference set (a declaration threads its own binder name so a
    /// self-recursive reference resolves through the window rather than parking).
    pub(crate) fn with_threaded(mut self, names: impl IntoIterator<Item = String>) -> Self {
        self.threaded = names.into_iter().collect();
        self
    }

    /// Set the declaration window the first walk minted its sibling handles against, so the re-walk
    /// mints the same indices.
    pub(crate) fn with_window(
        mut self,
        window: std::rc::Rc<crate::machine::model::RecursiveGroupWindow>,
    ) -> Self {
        self.window = Some(window);
        self
    }

    /// Set the lexical chain the re-walk resolves crossed-scope field names against.
    pub(crate) fn with_chain(mut self, chain: Option<Rc<LexicalFrame>>) -> Self {
        self.chain = chain;
        self
    }

    /// Move the in-flight binder guard into the deferral so its `Drop` fires on every finish arm,
    /// clearing the `pending_types` entry once the deferred walk resolves.
    pub(crate) fn with_pending_guard(mut self, guard: PendingBinderGuard) -> Self {
        self.pending_guard = Some(guard);
        self
    }

    /// Attach the trace frame the user-facing `Err` arm labels a shape error with.
    pub(crate) fn with_error_frame(mut self, frame: TraceFrame) -> Self {
        self.error_frame = Some(frame);
        self
    }

    /// Split the deferral into the deferred re-walk, the shared `[park_producers ++ owned_subs]` dep
    /// vector (parks first, then each sub-Dispatch owned in DFS order), and the pending guard the
    /// finish closure carries. The one place the dep vector is assembled.
    fn into_parts(
        self,
    ) -> (
        FieldListRewalk<'a>,
        Deps<OwnedDispatch<'a>>,
        Option<PendingBinderGuard>,
    ) {
        let rewalk = FieldListRewalk {
            expr: self.expr,
            context: self.context,
            name_kind: self.name_kind,
            threaded: self.threaded,
            window: self.window,
            chain: self.chain,
            error_frame: self.error_frame,
        };
        let mut deps = Deps::from_parks(self.park_producers);
        for expr in self.sub_dispatches {
            deps.own(OwnedDispatch {
                expr,
                placement: DepPlacement::OwnScope,
            });
        }
        (rewalk, deps, self.pending_guard)
    }

    /// Finish into the scheduler currency: a [`Outcome::ParkThenContinue`] whose dep-finish re-walks
    /// `expr` once the parks and owned sub-Dispatches resolve, then composes the pairs through
    /// `compose`. A pure decide, no write.
    pub(in crate::machine::execute) fn outcome(self, compose: BrandCompose<'a>) -> Outcome<'a> {
        let (rewalk, deps, pending_guard) = self.into_parts();
        let finish: TerminalDepFinish<'a> = Box::new(move |view, terminals| {
            // The guard's Drop clears the in-flight `pending_types` entry on every arm.
            let _pending_guard = pending_guard;
            // The owned suffix — each sub-Dispatch's terminal read live at the step brand — is the
            // walk's feed; the parks are notify-only waits on a forward reference. Each field type the
            // walk yields is cloned out as owned data, so the composed type needs no operand fold.
            let owned: Vec<Carried<'a>> = terminals.owned_slice().iter().map(|t| t.value).collect();
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
        // Lower each owned sub-Dispatch into the library dep currency `Await::on` consumes; the finish
        // reads only the owned suffix through the view.
        let (parks, owned) = deps.into_parts();
        let mut lowered: Deps<DepRequest<'a>> = Deps::from_parks(parks);
        for sub in owned {
            lowered.own(sub.into_request());
        }
        Await::on(lowered)
            .error_frame(dep_error_frame())
            .finish_terminal(finish)
    }

    /// Finish into the `Action` currency: an [`Action::AwaitDeps`](crate::machine::core::Action) whose
    /// re-walk of `expr` lifts the `finalize` result into `Action::Done`.
    pub(crate) fn action(
        self,
        finalize: FieldListFinalizeAction<'a>,
    ) -> crate::machine::core::Action<'a> {
        use crate::machine::core::{Action, AwaitContinue};
        let (rewalk, deps, pending_guard) = self.into_parts();
        let finish: AwaitContinue<'a> = Box::new(move |fctx, results| {
            // The guard's Drop clears the in-flight `pending_types` entry on every arm.
            let _pending_guard = pending_guard;
            // The owned suffix — each sub-Dispatch's terminal read live at the step brand — feeds the
            // re-walk; the parks are notify-only waits on a forward reference. Each field type the
            // walk yields is cloned out as owned data, so the composed type needs no operand fold.
            let owned: Vec<Carried<'a>> = results.owned_slice().iter().map(|t| t.value).collect();
            Action::Done(
                rewalk
                    .run(fctx.scope, &owned, fctx.types)
                    .and_then(|fields| finalize(fctx, fields)),
            )
        });
        Action::AwaitDeps { deps, finish }
    }

    /// Finish into the `Action` currency through a [`BrandCompose`], adapting `compose` into a
    /// [`FieldListFinalizeAction`] that carries the composed `KType` through the finish's allocator.
    pub(crate) fn action_composed(
        self,
        compose: BrandCompose<'a>,
    ) -> crate::machine::core::Action<'a> {
        self.action(Box::new(move |fctx, fields| {
            Ok(fctx.ctx.type_carried(compose(fields, fctx.types)?))
        }))
    }
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
            Outcome::Done(Ok(view.step_ctx().type_carried(kt)))
        }
        FieldListOutcome::Err(msg) => Outcome::Done(Err(KError::new(KErrorKind::ShapeError(msg)))),
        FieldListOutcome::Pending {
            park_producers,
            sub_dispatches,
        } => FieldListDeferral::new(
            fields,
            park_producers,
            sub_dispatches,
            FieldListContext::RECORD_TYPE,
            FieldNameKind::Identifier,
        )
        .with_chain(chain)
        .outcome(Box::new(|pairs, types| {
            Ok(types.record(Record::from_pairs(pairs)))
        })),
    }
}
