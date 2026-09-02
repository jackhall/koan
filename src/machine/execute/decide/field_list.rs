//! Shared deferral for typed field lists whose elaboration parked on a forward type or
//! scheduled sub-Dispatches for sigil field types — the NEWTYPE record repr, the UNION schema,
//! and the standalone record-type sigil (which is also how an `:(FN :{…} -> …)` parameter list
//! and an anonymous FN's record schema elaborate).
//!
//! [`FieldListDeferral`] bundles the forward-ref producers, the sigil sub-Dispatches, and the
//! elaborator state a re-walk needs. Its finish methods declare a dep-finish that waits on
//! `[awaited_producers ++ sub_dispatches]` and re-walks the field list through
//! [`parse_typed_field_list_via_elaborator`], feeding the resolved sub-Dispatch carriers back through
//! that walker's `results` channel in DFS order. Two composition surfaces consume the resulting
//! `(name, KType)` pairs:
//!
//! - [`FieldListDeferral::outcome`] (the record-type sigil) composes through a caller-supplied
//!   closure, which assembles one owned `KType` and allocates it into the consumer's own region —
//!   folded in generically, so the finish crosses the park as one `Copy` closure on the bumped
//!   tier;
//! - [`FieldListDeferral::action`] (the UNION schema and the NEWTYPE record repr) hands the pairs to a
//!   caller-supplied [`FieldListFinalizeAction`], which seals them through the declaration window into
//!   interned member handles and crosses the nominal identity through
//!   [`seal_type_identity`](super::constructors::seal_type_identity).

use std::rc::Rc;

use crate::machine::ProducerId;
use crate::machine::core::LexicalFrame;
use crate::machine::core::bindings::WriteOp;
use crate::machine::core::{DepPlacement, FinishCtx};
use crate::machine::model::Carried;
use crate::machine::model::WorkingExpression;
use crate::machine::model::{
    DeclWindow, Elaborator, FieldListContext, FieldListOutcome, FieldNameKind, FieldParts,
    ResultFeed, parse_typed_field_list_via_elaborator,
};
use crate::machine::model::{KType, Record};
use crate::machine::{KError, KErrorKind, Scope, TraceFrame};
use crate::scheduler::{Dep, Deps};
use crate::witnessed::BumpVec;

use super::super::StepCarried;
use super::super::outcome::DepTerminal;
use super::super::outcome::{Await, Outcome, StepDeps, dep_error_frame};
use super::SubDispatch;
use super::ctx::DecideCtx;
use crate::machine::model::RunRegistries;
use crate::machine::model::{BinderSymbol, TypeSymbol};

/// `Action`-path finalize, returning a witnessed carrier beside the binding writes the declarator
/// decided; the pair lifts straight into
/// [`Action::done_writing`](crate::machine::core::Action::done_writing).
pub(crate) type FieldListFinalizeAction<'a> = Box<
    dyn for<'r, 'w> FnOnce(
            &FinishCtx<'a, 'r>,
            Option<&'w DeclWindow<'a>>,
            Vec<(BinderSymbol, KType)>,
        ) -> Result<(StepCarried<'a>, Vec<WriteOp<'a>>), KError>
        + 'a,
>;

/// The deferred re-walk both currencies run once their deps resolve. It consumes only the
/// sub-Dispatch tail; `awaited_producers` are notify-only forward-ref waits. A still-`Pending` walk
/// is a scheduling inconsistency — every producer waited on is terminal by the dep-finish
/// invariant, so a second park is not a recoverable forward ref — and so it errors loudly.
struct FieldListRewalk<'step> {
    parts: FieldParts<'step>,
    context: FieldListContext,
    name_kind: FieldNameKind,
    threaded: Vec<TypeSymbol>,
    window: Option<DeclWindow<'step>>,
    chain: Option<Rc<LexicalFrame>>,
    error_frame: Option<TraceFrame>,
}

impl<'step> FieldListRewalk<'step> {
    /// The owned-bundle door onto [`rewalk_fields`], for the `Action` paths that carry threaded
    /// names, a declaration window, and an error frame.
    fn run<'f>(
        &self,
        scope: &Scope<'step>,
        feed: &[Carried<'f>],
        registries: &RunRegistries,
    ) -> Result<Vec<(BinderSymbol, KType)>, KError> {
        rewalk_fields(
            scope,
            self.parts,
            self.context,
            self.name_kind,
            self.threaded.iter().copied(),
            self.window.as_ref(),
            self.chain.clone(),
            self.error_frame.clone(),
            feed,
            registries,
        )
    }
}

/// Re-walk a parked field list against the resolved sub-Dispatch feed, in the elaborator state the
/// first walk ran under. Every parameter the deferral bundles is explicit here, so the bumped
/// `outcome` finish reaches the walk carrying only the `Copy` state its path actually uses.
///
/// `'f` is independent of `'step`: the walk reads a fed value only to extract its `KType` or render
/// it into an error string, so the feed may arrive at the short borrow of a dep envelope's open
/// guard, while the output pairs are owned `KType`s. `ResultFeed` is always installed — a
/// `Done`-shaped walk never pops it, and a popped-dry feed hits the loud "fewer resolved
/// sub-dispatches" error inside the walker.
#[allow(clippy::too_many_arguments)]
fn rewalk_fields<'step, 'f>(
    scope: &Scope<'step>,
    parts: FieldParts<'step>,
    context: FieldListContext,
    name_kind: FieldNameKind,
    threaded: impl IntoIterator<Item = TypeSymbol>,
    window: Option<&DeclWindow<'step>>,
    chain: Option<Rc<LexicalFrame>>,
    error_frame: Option<TraceFrame>,
    feed: &[Carried<'f>],
    registries: &RunRegistries,
) -> Result<Vec<(BinderSymbol, KType)>, KError> {
    let mut result_feed = ResultFeed::new(feed);
    let mut elaborator = Elaborator::new(scope)
        .with_threaded(threaded)
        .with_chain(chain);
    if let Some(window) = window {
        elaborator = elaborator.with_window(window.view());
    }
    match parse_typed_field_list_via_elaborator(
        parts,
        context,
        name_kind,
        &mut elaborator,
        Some(&mut result_feed),
        registries,
    ) {
        FieldListOutcome::Done(fields) => Ok(fields),
        FieldListOutcome::Err(msg) => {
            let error = KError::new(KErrorKind::ShapeError(msg));
            Err(match error_frame {
                Some(frame) => error.with_frame(frame),
                None => error,
            })
        }
        FieldListOutcome::Pending { .. } => Err(KError::new(KErrorKind::ShapeError(format!(
            "{}: forward type reference still unresolved after dep-finish wake",
            context.list
        )))),
    }
}

/// One field-list deferral, ready to finish into either dispatch currency. Holds the forward-ref
/// producers, the sigil sub-Dispatches (DFS order), and the elaborator state a re-walk rebuilds.
pub(crate) struct FieldListDeferral<'a> {
    parts: FieldParts<'a>,
    awaited_producers: Vec<ProducerId>,
    sub_dispatches: Vec<WorkingExpression<'a>>,
    context: FieldListContext,
    name_kind: FieldNameKind,
    threaded: Vec<TypeSymbol>,
    window: Option<DeclWindow<'a>>,
    chain: Option<Rc<LexicalFrame>>,
    error_frame: Option<TraceFrame>,
}

impl<'a> FieldListDeferral<'a> {
    /// The elaborator-rebuild optionals default empty/absent; the `with_*` setters thread them in.
    pub(crate) fn new(
        parts: FieldParts<'a>,
        awaited_producers: Vec<ProducerId>,
        sub_dispatches: Vec<WorkingExpression<'a>>,
        context: FieldListContext,
        name_kind: FieldNameKind,
    ) -> Self {
        Self {
            parts,
            awaited_producers,
            sub_dispatches,
            context,
            name_kind,
            threaded: Vec::new(),
            window: None,
            chain: None,
            error_frame: None,
        }
    }

    /// Seed the re-walk's threaded self-reference set (a declaration threads its own binder name so a
    /// self-recursive reference resolves through the window rather than parking).
    pub(crate) fn with_threaded(mut self, names: impl IntoIterator<Item = TypeSymbol>) -> Self {
        self.threaded = names.into_iter().collect();
        self
    }

    /// Set the declaration window the first walk minted its sibling handles against, so the re-walk
    /// mints the same indices.
    pub(crate) fn with_window(mut self, window: DeclWindow<'a>) -> Self {
        self.window = Some(window);
        self
    }

    /// Set the lexical chain the re-walk resolves crossed-scope field names against.
    pub(crate) fn with_chain(mut self, chain: Option<Rc<LexicalFrame>>) -> Self {
        self.chain = chain;
        self
    }

    /// Attach the trace frame the user-facing `Err` arm labels a shape error with.
    pub(crate) fn with_error_frame(mut self, frame: TraceFrame) -> Self {
        self.error_frame = Some(frame);
        self
    }

    /// Splits into the re-walk, the dep vector (forward-ref producers first, then each sub-Dispatch
    /// in DFS order), and the index the sub-Dispatch results start at.
    ///
    /// A field-list finish cannot read its results in order: it waits on forward-ref producers it
    /// never reads, while the re-walk resolves those names from the now-populated scope. So it
    /// slices its own deps out rather than consuming the list, and the sub-Dispatches go last so
    /// that slice is the tail from `first_sub` on.
    fn into_parts(self) -> (FieldListRewalk<'a>, Deps<SubDispatch<'a>>, usize) {
        let rewalk = FieldListRewalk {
            parts: self.parts,
            context: self.context,
            name_kind: self.name_kind,
            threaded: self.threaded,
            window: self.window,
            chain: self.chain,
            error_frame: self.error_frame,
        };
        let mut deps = Deps::from_producers(
            self.awaited_producers
                .into_iter()
                .map(ProducerId::scheduler_edge),
        );
        let first_sub = deps.len();
        for expr in self.sub_dispatches {
            deps.request(SubDispatch {
                expr,
                placement: DepPlacement::OwnScope,
            });
        }
        (rewalk, deps, first_sub)
    }

    /// Finish into the scheduler currency: an [`Outcome::Park`] whose dep-finish re-walks
    /// the field list once every dep resolves, then composes the pairs
    /// through `compose`. A pure decide, no write.
    ///
    /// The record-type sigil is this path's only caller, and it threads no self-reference, opens no
    /// declaration window and attaches no error frame — so the finish carries only `Copy` walk state
    /// and `compose`, and crosses the park bumped in the slot's own cart. The lexical chain is
    /// re-derived at wake rather than captured: the park keeps the slot's anchor, whose payload the
    /// step re-installs, so `view.active_chain()` is the very chain the decide walked under.
    pub(in crate::machine::execute) fn outcome<C>(
        self,
        view: &DecideCtx<'_, 'a, '_>,
        compose: C,
    ) -> Outcome<'a>
    where
        C: for<'r> Fn(Vec<(BinderSymbol, KType)>, &'r RunRegistries) -> Result<KType, KError>
            + Copy
            + 'a,
    {
        let (rewalk, deps, first_sub) = self.into_parts();
        debug_assert!(
            rewalk.threaded.is_empty() && rewalk.window.is_none() && rewalk.error_frame.is_none(),
            "the record-type sigil is `outcome`'s only path and carries none of the declaration state",
        );
        let FieldListRewalk {
            parts,
            context,
            name_kind,
            ..
        } = rewalk;
        let finish = move |view: &DecideCtx<'_, 'a, '_>, terminals: &[DepTerminal<'_>]| {
            // The sub-Dispatch tail feeds the walk; the deps ahead of it are notify-only waits on a
            // forward reference. The opens stay bound across the walk, so every value is read at one
            // common brand, and each field type is cloned out as owned data — no operand fold. Both
            // buffers die inside this wake step, so they ride the step scratch arena.
            let scratch = view.scratch();
            let mut opened = BumpVec::with_capacity_in(terminals.len() - first_sub, scratch);
            opened.extend(terminals[first_sub..].iter().map(|t| t.cell.open_at()));
            let mut owned: BumpVec<'_, Carried<'_>> =
                BumpVec::with_capacity_in(opened.len(), scratch);
            owned.extend(opened.iter().map(|o| o.value()));
            let sealed = rewalk_fields(
                view.current_scope(),
                parts,
                context,
                name_kind,
                [],
                None,
                view.active_chain(),
                None,
                &owned,
                view.registries(),
            )
            .and_then(|fields| {
                Ok(view
                    .step_ctx()
                    .type_carried(compose(fields, view.registries())?))
            });
            Outcome::Done(sealed)
        };
        // Lower each sub-Dispatch request into the library dep currency `Await::on` consumes; the
        // entries the deferral already named pass through, keeping the tail index above valid.
        let mut lowered: StepDeps<'a> = Deps::with_capacity_in(deps.len(), view.scratch());
        for entry in deps.into_entries() {
            match entry {
                Dep::Producer(source) => lowered.on(source),
                Dep::Request(sub) => {
                    lowered.request(sub.into_request());
                }
            }
        }
        Await::on(lowered)
            .error_frame(dep_error_frame())
            .finish_terminal(view.current_scope().brand(), finish)
    }

    /// Finish into the `Action` currency: an [`ActionKind::AwaitDeps`](crate::machine::core::ActionKind)
    /// whose re-walk of the field list lifts the `finalize` result — terminal plus binding writes —
    /// into the step's `Done` outcome.
    pub(crate) fn action(
        self,
        finalize: FieldListFinalizeAction<'a>,
    ) -> crate::machine::core::Action<'a> {
        use crate::machine::core::{Action, AwaitContinue};
        let (rewalk, deps, first_sub) = self.into_parts();
        let finish: AwaitContinue<'a> = Box::new(move |fctx, results| {
            // The sub-Dispatch tail feeds the walk; the deps ahead of it are notify-only waits on a
            // forward reference. The opens stay bound across the walk, so every value is read at one
            // common brand, and each field type is cloned out as owned data — no operand fold.
            let opened: Vec<_> = results[first_sub..]
                .iter()
                .map(|t| t.cell.open_at())
                .collect();
            let owned: Vec<Carried<'_>> = opened.iter().map(|o| o.value()).collect();
            Action::done_writing(
                fctx.scratch,
                rewalk
                    .run(fctx.scope, &owned, fctx.registries)
                    .and_then(|fields| finalize(fctx, rewalk.window.as_ref(), fields)),
            )
        });
        Action::await_deps(deps, finish)
    }
}

/// Elaborate a standalone `:{…}` record type to a `Carried::Type` record handle. A record type at a
/// value/type position declares no binder, so the elaborator threads no self-reference; a field
/// naming a forward type parks and a sigil field type sub-dispatches, both deferred through one
/// dep-finish (the field walker's own re-walk handles nested records).
pub(crate) fn elaborate_record_value<'step, 'view>(
    view: &DecideCtx<'_, 'step, 'view>,
    fields: FieldParts<'step>,
) -> Outcome<'step> {
    let mut elaborator = Elaborator::new(view.current_scope()).with_chain(view.active_chain());
    match parse_typed_field_list_via_elaborator(
        fields,
        FieldListContext::RECORD_TYPE,
        FieldNameKind::IdentifierOrType,
        &mut elaborator,
        None,
        view.registries(),
    ) {
        FieldListOutcome::Done(pairs) => {
            let kt = view.types().record(Record::from_pairs(pairs));
            Outcome::Done(Ok(view.step_ctx().type_carried(kt)))
        }
        FieldListOutcome::Err(msg) => Outcome::Done(Err(KError::new(KErrorKind::ShapeError(msg)))),
        FieldListOutcome::Pending {
            awaited_producers,
            sub_dispatches,
        } => FieldListDeferral::new(
            fields,
            awaited_producers,
            sub_dispatches,
            FieldListContext::RECORD_TYPE,
            FieldNameKind::IdentifierOrType,
        )
        .outcome(view, |pairs, registries| {
            Ok(registries.types.record(Record::from_pairs(pairs)))
        }),
    }
}
