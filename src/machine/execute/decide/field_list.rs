//! Shared deferral for typed field lists whose elaboration parked on a forward type or
//! scheduled sub-Dispatches for sigil field types — FN parameter lists, the
//! NEWTYPE record repr, the UNION schema, and the standalone record-type sigil.
//!
//! [`FieldListDeferral`] bundles the forward-ref producers, the sigil sub-Dispatches, and the
//! elaborator state a re-walk needs. Its finish methods declare a dep-finish that waits on
//! `[awaited_producers ++ sub_dispatches]` and re-walks the field list through
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

use crate::machine::ProducerId;
use crate::machine::core::bindings::WriteOp;
use crate::machine::core::{DepPlacement, FinishCtx};
use crate::machine::core::{LexicalFrame, StepAllocator};
use crate::machine::model::Carried;
use crate::machine::model::WorkingExpression;
use crate::machine::model::{
    DeclWindow, Elaborator, FieldListContext, FieldListOutcome, FieldNameKind, FieldParts,
    ResultFeed, parse_typed_field_list_via_elaborator,
};
use crate::machine::model::{KType, Record, TypeRegistry};
use crate::machine::{KError, KErrorKind, Scope, TraceFrame};
use crate::scheduler::{Dep, Deps};
use crate::witnessed::BumpAllocator;

use super::super::StepCarried;
use super::super::TerminalDepFinish;
use super::super::outcome::{Await, Outcome, StepDeps, dep_error_frame};
use super::SubDispatch;
use super::ctx::DecideCtx;

/// Composes the final `KType` from the elaborated pairs, plus whatever owned type content the
/// caller closed over (e.g. the FN return type). The composed value is allocated into the
/// consumer's own region through the single type door.
pub(crate) type BrandCompose<'step> = Box<
    dyn for<'r> FnOnce(Vec<(String, KType)>, &'r TypeRegistry) -> Result<KType, KError> + 'step,
>;

/// `Action`-path finalize, returning a witnessed carrier beside the binding writes the declarator
/// decided; the pair lifts straight into
/// [`Action::done_writing`](crate::machine::core::Action::done_writing).
pub(crate) type FieldListFinalizeAction<'a> = Box<
    dyn for<'r, 'w> FnOnce(
            &FinishCtx<'a, 'r>,
            Option<&'w DeclWindow<'a>>,
            Vec<(String, KType)>,
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
    threaded: Vec<String>,
    window: Option<DeclWindow<'step>>,
    chain: Option<Rc<LexicalFrame>>,
    error_frame: Option<TraceFrame>,
}

impl<'step> FieldListRewalk<'step> {
    /// `'f` is independent of `'step`: the walk reads a fed value only to extract its `KType` or
    /// render it into an error string, so the feed may arrive at the short borrow of a dep
    /// envelope's open guard, while the output pairs are owned `KType`s. `ResultFeed` is always
    /// installed — a `Done`-shaped walk never pops it, and a popped-dry feed hits the loud "fewer
    /// resolved sub-dispatches" error inside the walker.
    fn run<'f>(
        &self,
        scope: &Scope<'step>,
        feed: &[Carried<'f>],
        types: &TypeRegistry,
    ) -> Result<Vec<(String, KType)>, KError> {
        let mut result_feed = ResultFeed::new(feed);
        let mut elaborator = Elaborator::new(scope)
            .with_threaded(self.threaded.iter().cloned())
            .with_chain(self.chain.clone());
        if let Some(window) = self.window.as_ref() {
            elaborator = elaborator.with_window(window.view());
        }
        match parse_typed_field_list_via_elaborator(
            self.parts,
            self.context,
            self.name_kind,
            &mut elaborator,
            Some(&mut result_feed),
            types,
        ) {
            FieldListOutcome::Done(fields) => Ok(fields),
            FieldListOutcome::Err(msg) => {
                let error = KError::new(KErrorKind::ShapeError(msg));
                Err(match self.error_frame.clone() {
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

/// `feed` is the sub-Dispatch tail of the dep terminals in DFS order — the forward-ref deps are
/// notify-only waits, so they never reach the walk. Every field type the walk produces is
/// owned data, so the composed type embeds no borrow of a producer region.
fn compose_field_list<'step, 'f>(
    step_ctx: &StepAllocator<'step>,
    scope: &'step Scope<'step>,
    rewalk: FieldListRewalk<'step>,
    feed: &[Carried<'f>],
    compose: BrandCompose<'step>,
    types: &TypeRegistry,
) -> Result<StepCarried<'step>, KError> {
    let fields = rewalk.run(scope, feed, types)?;
    Ok(step_ctx.type_carried(compose(fields, types)?))
}

/// One field-list deferral, ready to finish into either dispatch currency. Holds the forward-ref
/// producers, the sigil sub-Dispatches (DFS order), and the elaborator state a re-walk rebuilds.
pub(crate) struct FieldListDeferral<'a> {
    parts: FieldParts<'a>,
    awaited_producers: Vec<ProducerId>,
    sub_dispatches: Vec<WorkingExpression<'a>>,
    context: FieldListContext,
    name_kind: FieldNameKind,
    threaded: Vec<String>,
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
    pub(crate) fn with_threaded(mut self, names: impl IntoIterator<Item = String>) -> Self {
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
    pub(in crate::machine::execute) fn outcome(
        self,
        compose: BrandCompose<'a>,
        scratch: BumpAllocator<'a>,
    ) -> Outcome<'a> {
        let (rewalk, deps, first_sub) = self.into_parts();
        let finish: TerminalDepFinish<'a> = Box::new(move |view, terminals| {
            // The sub-Dispatch tail feeds the walk; the deps ahead of it are notify-only waits on a
            // forward reference. The opens stay bound across the walk, so every value is read at one
            // common brand, and each field type is cloned out as owned data — no operand fold.
            let opened: Vec<_> = terminals[first_sub..]
                .iter()
                .map(|t| t.cell.open_at())
                .collect();
            let owned: Vec<Carried<'_>> = opened.iter().map(|o| o.value()).collect();
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
        // Lower each sub-Dispatch request into the library dep currency `Await::on` consumes; the
        // entries the deferral already named pass through, keeping the tail index above valid.
        let mut lowered: StepDeps<'a> = Deps::with_capacity_in(deps.len(), scratch);
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
            .finish_terminal(finish)
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
                rewalk
                    .run(fctx.scope, &owned, fctx.types)
                    .and_then(|fields| finalize(fctx, rewalk.window.as_ref(), fields)),
            )
        });
        Action::await_deps(deps, finish)
    }

    /// Finish into the `Action` currency through a [`BrandCompose`], adapting `compose` into a
    /// [`FieldListFinalizeAction`] that carries the composed `KType` through the finish's allocator.
    pub(crate) fn action_composed(
        self,
        compose: BrandCompose<'a>,
    ) -> crate::machine::core::Action<'a> {
        self.action(Box::new(move |fctx, _window, fields| {
            // A composed structural type declares no binder, so it writes nothing.
            Ok((
                fctx.ctx.type_carried(compose(fields, fctx.types)?),
                Vec::new(),
            ))
        }))
    }
}

/// Elaborate a standalone `:{…}` record type to a `Carried::Type` record handle. A record type at a
/// value/type position declares no binder, so the elaborator threads no self-reference; a field
/// naming a forward type parks and a sigil field type sub-dispatches, both deferred through one
/// dep-finish (the field walker's own re-walk handles nested records).
pub(crate) fn elaborate_record_value<'step, 'view>(
    view: &DecideCtx<'_, 'step, 'view>,
    fields: FieldParts<'step>,
    chain: Option<Rc<LexicalFrame>>,
) -> Outcome<'step> {
    let mut elaborator = Elaborator::new(view.current_scope()).with_chain(chain.clone());
    match parse_typed_field_list_via_elaborator(
        fields,
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
            awaited_producers,
            sub_dispatches,
        } => FieldListDeferral::new(
            fields,
            awaited_producers,
            sub_dispatches,
            FieldListContext::RECORD_TYPE,
            FieldNameKind::Identifier,
        )
        .with_chain(chain)
        .outcome(
            Box::new(|pairs, types| Ok(types.record(Record::from_pairs(pairs)))),
            view.scratch(),
        ),
    }
}
