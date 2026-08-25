//! Dispatch shape router, classifier, and shared spine — the *decide* phase.
//!
//! [`classify_dispatch`] routes a slot on its cached [`DispatchShape`] to the matching per-shape
//! decide. State and transitions live with their shape; this file keeps the cross-shape glue plus
//! [`run_action`], the shared *action* harness (a pure `Action -> Outcome` lowering).
//!
//! Every per-shape handler decides against a read-only [`DecideCtx`] and returns an [`Outcome`]
//! that the harness ([`super::harness`]) applies, so no shape module mutates the scheduler.

use crate::machine::DeliveredCarried;
use crate::machine::ProducerId;
use crate::machine::core::RegionBrand;
use crate::machine::core::{
    Action, ActionKind, AwaitContinue, BlockEntry, BlockRequest, FinishCtx, FramePlacement,
    ReturnContract, TailContract,
};
use crate::machine::model::{Carried, ExpressionPart, WorkingExpression, WorkingPart};
use crate::machine::{KError, KErrorKind, NodeId};
use crate::source::Spanned;
use std::rc::Rc;

use super::harness::KoanWorkload;
use super::nodes::{NodeWork, WorkLabel};
use super::obligation::ReturnObligation;
pub(in crate::machine::execute) use super::outcome::StepDeps;
use super::outcome::{
    DepTerminal, NodeContinuation, ParkDeps, catching, continue_inline, decide_only,
    dep_error_frame, erase_boxed, erase_bumped, tail_continue,
};
use crate::machine::model::RunRegistries;
use crate::scheduler::{Dep, Deps};
use crate::witnessed::BumpAllocator;

pub(in crate::machine::execute) use crate::machine::core::{
    BodyPlacement, DepPlacement, DepRequest, SubDispatch,
};

pub(in crate::machine::execute) mod apply_callable;
mod constructors;
mod ctx;
mod exec;
pub(in crate::machine) mod field_list;
pub(in crate::machine::execute) mod fn_value;
pub(in crate::machine::execute) mod head_deferred;
pub(in crate::machine::execute) mod keyworded;
mod literal;
pub(in crate::machine::execute) mod operator_chain;
mod resolve;
pub(in crate::machine) mod resolve_dispatch;
pub(in crate::machine) mod resolve_type_identifier;
pub(in crate::machine::execute) mod single_poll;
mod submit;
pub(in crate::machine::execute) use submit::{SubmitContext, statement_binder_plan};

#[cfg(test)]
mod tests;

pub(in crate::machine::execute) use super::outcome::{
    Await, Continuation, DeferredTraceFrame, Outcome,
};
pub(crate) use constructors::{build_type_operand, seal_type_identity};
pub(in crate::machine::execute) use ctx::{DecideCtx, with_node_scope};
pub(crate) use field_list::{BrandCompose, FieldListDeferral};
pub(crate) use resolve::Resolution;
pub(super) use resolve::{TypeChannel, type_channel};
pub use resolve_dispatch::{DispatchOutcome, Resolved};
#[cfg(test)]
pub use resolve_dispatch::{reset_resolve_dispatch_entry_count, resolve_dispatch_entry_count};

/// Shape classification is pure-structural and cached on the node at parse time; re-exported so
/// decide-internal call sites and tests keep the `decide::` path.
#[allow(unused_imports)]
pub(crate) use crate::machine::model::{DispatchShape, classify_dispatch_shape};

/// The staged form of one eager part shape. Adding a variant forces a [`stage_eager_part`] arm via
/// match exhaustiveness.
enum EagerShape {
    /// `(...)` — the nested inner expression dispatches directly.
    Subexpression,
    /// `:(…)` / `:{…}` — the whole part rewraps as a one-part sub-Dispatch to a type-side carrier.
    TypeExpression,
    ListLiteral,
    DictLiteral,
    RecordLiteral,
}

/// The only place the eager part-shape set is enumerated. `None` means the part is not eager.
fn eager_shape(part: &ExpressionPart<'_>) -> Option<EagerShape> {
    match part {
        ExpressionPart::Expression(_) => Some(EagerShape::Subexpression),
        ExpressionPart::SigiledTypeExpr(_) | ExpressionPart::RecordType(_) => {
            Some(EagerShape::TypeExpression)
        }
        ExpressionPart::ListLiteral(_) => Some(EagerShape::ListLiteral),
        ExpressionPart::DictLiteral(_) => Some(EagerShape::DictLiteral),
        ExpressionPart::RecordLiteral(_) => Some(EagerShape::RecordLiteral),
        _ => None,
    }
}

/// True iff this part shape is one the eager loop schedules as a sub-Dispatch.
pub(in crate::machine::execute) fn is_eager_part(part: &ExpressionPart<'_>) -> bool {
    eager_shape(part).is_some()
}

/// [`is_eager_part`] read through a dispatch slot: a synthesized node or a threaded record-type
/// body stages exactly as its parsed `(...)` / `:{…}` counterpart does, while a resolved cell or a
/// staging hole is already past staging.
pub(in crate::machine::execute) fn is_eager_working_part(part: &WorkingPart<'_>) -> bool {
    match part {
        WorkingPart::Expression(_) => true,
        WorkingPart::RecordType(_) => true,
        WorkingPart::Ast(ast) => is_eager_part(ast),
        WorkingPart::Spliced { .. } | WorkingPart::StagedSlot => false,
    }
}

/// Stage one slot of a working expression. A slot still holding raw AST classifies through
/// [`stage_eager_part`]; a synthesized node is already a working expression, so it dispatches
/// directly. Every other arm — a resolved cell, a staging hole — rides through as `Err`.
pub(in crate::machine::execute) fn stage_eager_working_part<'a>(
    brand: RegionBrand<'a>,
    part: WorkingPart<'a>,
) -> Result<DepRequest<'a>, WorkingPart<'a>> {
    match part {
        WorkingPart::Ast(ast) => stage_eager_part(brand, ast).map_err(WorkingPart::Ast),
        WorkingPart::Expression(inner) => Ok(DepRequest::Dispatch {
            expr: *inner,
            placement: DepPlacement::OwnScope,
        }),
        // The record-type wrapper is the handler, so the sub-Dispatch must see a one-part
        // `RecordType`-shaped node, not the body.
        WorkingPart::RecordType(_) => Ok(DepRequest::Dispatch {
            expr: WorkingExpression::new(brand, &[Spanned::bare(part)]),
            placement: DepPlacement::OwnScope,
        }),
        other => Err(other),
    }
}

pub(in crate::machine::execute) fn stage_eager_part<'a>(
    brand: RegionBrand<'a>,
    part: ExpressionPart<'a>,
) -> Result<DepRequest<'a>, ExpressionPart<'a>> {
    match eager_shape(&part) {
        None => Err(part),
        Some(EagerShape::Subexpression) => {
            let ExpressionPart::Expression(inner) = part else {
                unreachable!("eager_shape matched Subexpression")
            };
            Ok(DepRequest::Dispatch {
                expr: WorkingExpression::from_ast(brand, *inner),
                placement: DepPlacement::OwnScope,
            })
        }
        Some(EagerShape::TypeExpression) => Ok(DepRequest::Dispatch {
            // The type-expression wrapper is the handler, so the sub-Dispatch sees the whole
            // part rewrapped — the same shape `classify_aggregate_part` builds.
            expr: WorkingExpression::new(brand, &[Spanned::bare(WorkingPart::Ast(part))]),
            placement: DepPlacement::OwnScope,
        }),
        Some(EagerShape::ListLiteral) => {
            let ExpressionPart::ListLiteral(items) = part else {
                unreachable!("eager_shape matched ListLiteral")
            };
            Ok(DepRequest::ListLit(items))
        }
        Some(EagerShape::DictLiteral) => {
            let ExpressionPart::DictLiteral(pairs) = part else {
                unreachable!("eager_shape matched DictLiteral")
            };
            Ok(DepRequest::DictLit(pairs))
        }
        Some(EagerShape::RecordLiteral) => {
            let ExpressionPart::RecordLiteral(fields) = part else {
                unreachable!("eager_shape matched RecordLiteral")
            };
            Ok(DepRequest::RecordLit(fields))
        }
    }
}

/// The hole a staged slot leaves behind, holding its position until the eager-subs finish rebuilds
/// the run with the resolved `Spliced` cell in its place.
pub(in crate::machine::execute) fn staged_slot_placeholder<'a>() -> Spanned<WorkingPart<'a>> {
    Spanned::bare(WorkingPart::StagedSlot)
}

/// [`TraceFrame::from_expr`](crate::machine::TraceFrame::from_expr)'s deferred peer for the
/// scheduler's own per-call node: captures the label and the `Copy` expression, so the frame costs
/// nothing until a dep error renders it. `function` is a label (`"<bind>"`, `"<dispatch-park>"`)
/// for a scheduler-internal frame with no `KFunction` behind it.
pub(in crate::machine::execute) fn working_frame<'step>(
    function: &'static str,
    expr: &WorkingExpression<'step>,
) -> DeferredTraceFrame<'step> {
    DeferredTraceFrame::Working {
        function,
        expr: *expr,
    }
}

/// Clone a dep's terminal error and attach a caller-chosen frame, rendering it here — the one point
/// a [`DeferredTraceFrame`] becomes trace text. `frame = None` is the frameless variant.
pub(in crate::machine::execute) fn propagate_dep_error(
    e: &KError,
    frame: Option<DeferredTraceFrame<'_>>,
    registries: &RunRegistries,
) -> KError {
    let cloned = e.clone_for_propagation();
    match frame {
        Some(f) => cloned.with_frame(f.render(registries)),
        None => cloned,
    }
}

// ---------- Outcome constructors (the dispatch-currency → Outcome mapping) ----------

/// Park the slot on `sources` — the binder edges its names resolved to — and re-run its `resume`
/// decide on wake. The park carries no deadlock sample of its own: it keeps the slot's anchor, so
/// the [`WorkLabel`] minted at submission is what a stuck slot renders through.
pub(in crate::machine::execute) fn park_resume<'step, F>(
    sources: Vec<ProducerId>,
    view: &DecideCtx<'_, 'step, '_>,
    resume: F,
) -> Outcome<'step>
where
    F: for<'view> Fn(&DecideCtx<'_, 'step, 'view>, NodeId) -> Outcome<'step> + Copy + 'step,
{
    park_resume_labelled(sources, None, view, resume)
}

/// [`park_resume`] carrying an explicit dep-error frame, so an error the install surfaces when a
/// source names an already-errored producer is framed at the park site rather than arriving bare.
///
/// A park keeps the slot's cart, so the resume closure is hosted in the region `view` already
/// stands in — the bumped tier, with `Copy` captures and no heap.
pub(in crate::machine::execute) fn park_resume_labelled<'step, F>(
    sources: Vec<ProducerId>,
    dep_error_frame: Option<DeferredTraceFrame<'step>>,
    view: &DecideCtx<'_, 'step, '_>,
    resume: F,
) -> Outcome<'step>
where
    F: for<'view> Fn(&DecideCtx<'_, 'step, 'view>, NodeId) -> Outcome<'step> + Copy + 'step,
{
    Outcome::Park {
        deps: ParkDeps::List(Deps::from_producers_in(
            sources.iter().copied().map(ProducerId::scheduler_edge),
            view.scratch(),
        )),
        continuation: Continuation::Ready(erase_bumped(
            view.current_scope().brand(),
            decide_only(resume),
        )),
        dep_error_frame,
    }
}

/// A bare-identifier slot whose name binds to the binder behind `source`: the slot's result *is*
/// that producer's result, so the harness splices the slot out rather than keeping a forwarding
/// node.
pub(in crate::machine::execute) fn forward_to_producer<'step>(
    source: ProducerId,
) -> Outcome<'step> {
    Outcome::Forward(source.scheduler_edge())
}

/// Replace the slot with a fresh frameless `Dispatch` of `inner` — the decide reduced its
/// expression to a nested one to re-classify (`(inner)`, `:(...)` unwrap). The slot holds no
/// contract of its own, so any established tail-chain obligation rides along and the re-classified
/// step re-deposits the checker rather than dropping it.
pub(in crate::machine::execute) fn become_dispatch<'step>(
    view: &DecideCtx<'_, 'step, '_>,
    inner: WorkingExpression<'step>,
) -> Outcome<'step> {
    let label = WorkLabel::of(&inner);
    continue_inline(
        decide_tail(
            inner,
            view.current_obligation(),
            view.current_scope().brand(),
        ),
        label,
    )
}

/// What a dispatch part walk produced — the splice / stage pass over a node's slots.
///
/// A walk that splices no slot and stages no sub provably produced the run it was handed, so
/// [`Unchanged`](PartWalk::Unchanged) lets the caller keep its node: nothing re-bumped, no
/// structural cache recomputed from a run that did not move.
pub(in crate::machine::execute) enum PartWalk<'step> {
    /// Nothing spliced and nothing staged — the node the walk was given already holds this run.
    Unchanged,
    /// The walk rebuilt the run: `expr` is the re-frozen node, `staged` the deps whose resolved
    /// carriers fill its staging holes, in slot order.
    Respliced {
        expr: WorkingExpression<'step>,
        staged: StagedSubs<'step>,
    },
}

/// A part walk's staged subs, as the two lists their consumers actually want: the park's dep list,
/// and the slot each dep's result splices back into. The walk appends to both in one pass, so the
/// park currency is built where the requests are produced rather than transposed afterwards.
///
/// The two are hosted differently on purpose. `deps` is a [`StepDeps`], dead at the end of the pop
/// that wired it; `part_indices` is heap-hosted, because the dep-finish closure carries it **across
/// the park** — past the scratch reset — to place the resolved cells on wake.
pub(in crate::machine::execute) struct StagedSubs<'step> {
    pub(in crate::machine::execute) part_indices: Vec<usize>,
    pub(in crate::machine::execute) deps: StepDeps<'step>,
}

impl<'step> StagedSubs<'step> {
    pub(in crate::machine::execute) fn new_in(scratch: BumpAllocator<'step>) -> Self {
        StagedSubs {
            part_indices: Vec::new(),
            deps: Deps::new_in(scratch),
        }
    }

    /// Stage one dep against the part slot its result fills.
    pub(in crate::machine::execute) fn push(&mut self, slot: usize, dep: DepRequest<'step>) {
        self.part_indices.push(slot);
        self.deps.request(dep);
    }

    pub(in crate::machine::execute) fn is_empty(&self) -> bool {
        self.deps.is_empty()
    }

    /// The staged slots in ascending order — the cursor a rebuild walks its parts against.
    pub(in crate::machine::execute) fn slots(&self) -> impl Iterator<Item = usize> + '_ {
        self.part_indices.iter().copied()
    }
}

/// Walk raw parts emitting a [`StagedSlot`](WorkingPart::StagedSlot) marker at every eager slot
/// and a parallel staged-subs Vec; non-eager parts pass through unchanged.
///
/// `wrap_indices` names bare-name value slots (from
/// [`KFunction::classify_for_pick`](crate::machine::core::KFunction::classify_for_pick)) to
/// resolve before bind. Only a caller that has already committed to one callable passes them: with
/// a pick outstanding the carried type must be known *during* overload selection, which the
/// `bare_outcomes` lookup answers, so those callers pass `&[]`.
///
/// Staged first, rebuilt second: every slot this walk touches becomes a staging hole, so the
/// staged list alone decides whether the run moved.
pub(super) fn stage_all_eager_parts<'step>(
    brand: RegionBrand<'step>,
    origin: &WorkingExpression<'step>,
    wrap_indices: &[usize],
    scratch: BumpAllocator<'step>,
) -> PartWalk<'step> {
    let parts = origin.parts;
    let mut staged = StagedSubs::new_in(scratch);
    for (i, part) in parts.iter().enumerate() {
        if wrap_indices.contains(&i) {
            // Resolve the name through a single-part sub-Dispatch so the resolved `Spliced`
            // carrier reaches `accepts_part` at bind. Not one of the eager shapes, hence a
            // pre-check before the stager.
            let wrapped = WorkingExpression::synthesized(brand, std::slice::from_ref(part), origin);
            staged.push(
                i,
                DepRequest::Dispatch {
                    expr: wrapped,
                    placement: DepPlacement::OwnScope,
                },
            );
            continue;
        }
        if let Ok(dep) = stage_eager_working_part(brand, part.value) {
            staged.push(i, dep);
        }
    }
    if staged.is_empty() {
        return PartWalk::Unchanged;
    }
    // Staging ran in slot order, so one ascending cursor over the staged indices places every hole
    // in a single pass that fills the region's bytes directly, with no owned copy in between.
    let expr = {
        let mut holes = staged.slots().peekable();
        origin.respliced(
            brand,
            parts.iter().enumerate().map(|(i, part)| {
                if holes.next_if_eq(&i).is_some() {
                    staged_slot_placeholder()
                } else {
                    *part
                }
            }),
        )
    };
    PartWalk::Respliced { expr, staged }
}

// ---------- Cross-shape driver ----------

/// Build a birth dispatch [`NodeWork`](super::nodes::NodeWork) for `expr`. A declared-return
/// `obligation` rides as data and re-deposits into the ambient slot-step state before the step
/// classifies — the keep-first carriage of the first caller's declared return down a tail chain.
/// `None` inherits no obligation.
///
/// `host` is the region of the frame this work installs under — the ambient brand for a kept cart
/// or a submission, or the brand a fresh replacement's constructor mints off the cart it installs.
/// The decide's only capture is the `Copy` working expression, so it always erases onto the bumped
/// tier.
pub(in crate::machine::execute) fn decide_tail<'step>(
    expr: WorkingExpression<'step>,
    obligation: Option<ReturnObligation>,
    host: RegionBrand<'step>,
) -> NodeContinuation<'step> {
    // A birth decide waits on no deps: it runs on first poll, classifies, and routes.
    let decide =
        decide_only(move |view: &DecideCtx<'_, 'step, '_>, _id| classify_dispatch(view, expr));
    NodeContinuation::new(obligation, erase_bumped(host, decide))
}

/// Build a [`NodeWork`](super::nodes::NodeWork) that fails on its first poll with `error`. The node
/// is slot-terminal (TRY-catchable) and propagates through its dep like any other failed dep, so a
/// pre-errored slot needs no special case downstream.
pub(in crate::machine::execute) fn decide_error<'step>(
    error: KError,
) -> NodeWork<'step, KoanWorkload> {
    // Owning: the captured `KError` needs its drop glue, so this decide stays on the boxed tier.
    NodeWork::new(NodeContinuation::new(
        None,
        erase_boxed(
            move |_view: &DecideCtx<'_, '_, '_>,
                  _results: &[Result<DepTerminal<'_>, KError>],
                  _id: NodeId| Outcome::Done(Err(error)),
        ),
    ))
}

/// Classify a freshly-born dispatch expression's shape and route to the matching per-shape decide.
/// A shape that parks returns a `Park` whose resume closure re-enters the drain's step, never back
/// through here.
fn classify_dispatch<'step>(
    view: &DecideCtx<'_, 'step, '_>,
    expr: WorkingExpression<'step>,
) -> Outcome<'step> {
    match expr.shape() {
        DispatchShape::BareTypeLeaf => {
            let t = match expr.parts[0].value {
                WorkingPart::Ast(ExpressionPart::Type(t)) => t,
                _ => unreachable!("BareTypeLeaf shape implies single leaf Type part"),
            };
            single_poll::bare_type_leaf(view, view.current_scope(), t)
        }
        DispatchShape::BareIdentifier => {
            // The shape's other member is a lone `StagedSlot`, which no node carries into a
            // classify: a part walk's holes are all spliced by `install_eager_subs`'s finish,
            // which routes the filled node to its pick or its re-resolve, never back through here.
            let [
                Spanned {
                    value: WorkingPart::Ast(ExpressionPart::Identifier(name)),
                    ..
                },
            ] = expr.parts
            else {
                unreachable!(
                    "BareIdentifier routes a lone Identifier part; a lone StagedSlot shares the \
                     shape but is spliced before its node is classified"
                )
            };
            single_poll::bare_identifier(view, view.current_scope(), *name)
        }
        DispatchShape::FunctionValueCall => fn_value::initial(view, expr),
        DispatchShape::TypeCall => single_poll::type_call(view, expr),
        DispatchShape::HeadDeferred => head_deferred::initial_expr(view, expr),
        DispatchShape::TypeHeadDeferred => head_deferred::initial_type(view, expr),
        // Slot-terminal (TRY-catchable): a non-callable head is a runtime error, not a fatal
        // drive abort.
        DispatchShape::NonCallableHead => {
            Outcome::Done(Err(KError::new(KErrorKind::DispatchFailed {
                expr: expr.summarize(&view.registries().labels),
                reason: format!(
                    "head is not callable: `{}`",
                    expr.parts
                        .first()
                        .map(|p| p.value.summarize(&view.registries().labels))
                        .unwrap_or_else(|| "<empty>".into())
                ),
            })))
        }
        DispatchShape::OperatorChain => operator_chain::run(view, view.current_scope(), &expr),
        DispatchShape::Keyworded => keyworded::initial(view, expr),
        DispatchShape::SigiledTypeExpr => single_poll::sigiled_type_expr(view, expr),
        DispatchShape::RecordType => single_poll::record_type(view, expr),
        DispatchShape::LiteralPassThrough => single_poll::literal_pass_through(view, expr),
    }
}

// ---------- The action harness ----------

/// Project a builtin's [`AwaitContinue`] onto the terminal-finish delivery: assemble the wake-time
/// [`FinishCtx`] and recurse `run_action` on the `Action` it returns. Shared by the two await
/// currencies — the dep list and the block — which differ only in how their deps are named.
fn wrap_await_continue<'step>(
    finish: AwaitContinue<'step>,
) -> impl for<'view, 'd> FnOnce(&DecideCtx<'_, 'step, 'view>, &[DepTerminal<'d>]) -> Outcome<'step> + 'step
{
    move |view: &DecideCtx<'_, 'step, '_>, results: &[DepTerminal<'_>]| {
        let fctx = FinishCtx {
            scope: view.current_scope(),
            ctx: view.step_ctx(),
            registries: view.registries(),
        };
        run_action(view, finish(&fctx, results))
    }
}

/// Lower an [`Action`] into the [`Outcome`] currency, issuing no graph write of its own: an await
/// or catch declares its deps as an [`Outcome::Park`] and the harness wires and applies. Every
/// scheduler read the body needs is deferred into a finish, which sees its own wake-time
/// [`DecideCtx`] — so the obligation a finish reads is the one its park deposit re-installed,
/// while a tail `Action` reads the executing step's established obligation off `view`.
pub(in crate::machine::execute) fn run_action<'step>(
    view: &DecideCtx<'_, 'step, '_>,
    action: Action<'step>,
) -> Outcome<'step> {
    // Binding-table writes travel as outcome data, so they must reach the harness-owned sink in
    // the order the bodies decided them — a chain of finishes recurses through here and so
    // contributes its writes in program order.
    view.deposit_effects(action.effects);
    match action.next {
        ActionKind::Done(result) => Outcome::Done(result),

        ActionKind::Tail {
            leading,
            tail,
            contract,
            frame_placement,
            block_entry,
        } => {
            // A block-entering tail sits above the params (`1`) or the leading siblings (`N`); a
            // frameless continuation keeps the slot's block at index `0`.
            let body_index = if matches!(block_entry, BlockEntry::None) {
                0
            } else {
                leading.len() + 1
            };
            if leading.is_empty() {
                let contract = match contract {
                    TailContract::Eager(contract) => contract,
                    TailContract::FromLastResult { .. } => {
                        unreachable!(
                            "a from-last-result contract rides at least its type statement"
                        )
                    }
                };
                return tail_continue(
                    view,
                    tail,
                    contract,
                    frame_placement,
                    block_entry,
                    body_index,
                );
            }
            // Decompose the placement pair here, before the park: the finish's capture set is then
            // `Copy` data plus the one block-frame `Rc` that keeps the block alive across it —
            // nothing else holds the frame — and the pair is rebuilt from those locals at wake.
            // `FreshTail` installs its cart only at apply time, after the leading statements would
            // already have fanned out, so a leading-carrying tail cannot ride it.
            let (fresh_child, block_frame, overlay) = match (frame_placement, block_entry) {
                (FramePlacement::FreshChild { frame }, BlockEntry::FrameScope(entry)) => {
                    debug_assert!(
                        Rc::ptr_eq(&frame, &entry),
                        "a FreshChild block is the fresh frame's own scope"
                    );
                    (true, Some(entry), None)
                }
                (FramePlacement::Inherit, BlockEntry::FrameScope(entry)) => {
                    (false, Some(entry), None)
                }
                (FramePlacement::Inherit, BlockEntry::Overlay(overlay)) => {
                    (false, None, Some(overlay))
                }
                _ => unreachable!(
                    "a leading-carrying tail is a FreshChild frame, an Inherit cart, or an overlay"
                ),
            };
            // Leading statements become owned siblings in the block, and the slot parks on them so
            // they run — and reclaim — before the tail continues. The block frame or the overlay
            // names where they bind.
            let placement = match (&block_frame, overlay) {
                (Some(frame), _) => BodyPlacement::Frame(Rc::clone(frame)),
                (None, Some(overlay)) => BodyPlacement::Overlay(overlay),
                (None, None) => unreachable!("a leading-carrying tail enters a block"),
            };
            let finish = move |view: &DecideCtx<'_, 'step, '_>, terminals: &[DepTerminal<'_>]| {
                let contract = match contract {
                    TailContract::Eager(contract) => contract,
                    // The return-type expression is the last leading statement, so its resolved
                    // value is the last terminal, read in place in the region it was delivered
                    // into.
                    TailContract::FromLastResult { func, site } => {
                        let terminal = terminals[terminals.len() - 1];
                        let opened = terminal.cell.open_at();
                        let kt = match opened.value() {
                            Carried::Type(t) => t,
                            Carried::Object(other) => {
                                return Outcome::Done(Err(KError::new(KErrorKind::ShapeError(
                                    format!(
                                        "FN deferred return-type expression produced a non-type {} value",
                                        other.ktype().name(view.registries()),
                                    ),
                                ))));
                            }
                            Carried::UnresolvedType(ti) => {
                                return Outcome::Done(Err(KError::new(KErrorKind::UnboundName(
                                    crate::machine::model::render_label(
                                        ti.symbol(),
                                        view.registries(),
                                    ),
                                ))));
                            }
                        };
                        // `KType` is a `Copy` handle, so the contract outlives the sub-dispatch's
                        // terminal without naming any region.
                        Some(ReturnContract::PerCall {
                            func,
                            ret: kt,
                            site,
                        })
                    }
                };
                let (frame_placement, block_entry) = match (block_frame, overlay) {
                    (Some(frame), _) => (
                        if fresh_child {
                            FramePlacement::FreshChild {
                                frame: Rc::clone(&frame),
                            }
                        } else {
                            FramePlacement::Inherit
                        },
                        BlockEntry::FrameScope(frame),
                    ),
                    (None, Some(overlay)) => {
                        (FramePlacement::Inherit, BlockEntry::Overlay(overlay))
                    }
                    (None, None) => {
                        unreachable!("the pre-park decomposition emits a block frame or an overlay")
                    }
                };
                // Against this finish's own wake-time view: the park re-deposited the established
                // obligation, so a chain checks its first caller's declared return rather than
                // this resolving tail's.
                tail_continue(
                    view,
                    tail,
                    contract,
                    frame_placement,
                    block_entry,
                    body_index,
                )
            };
            Await::on_block(BlockRequest::Body {
                statements: leading,
                placement,
            })
            .error_frame(dep_error_frame())
            .finish_terminal(finish)
        }

        ActionKind::AwaitDeps { deps, finish } => {
            // Results come back in the order the builtin assembled the list — one per entry — so
            // the lowering must preserve it: an index banked at `Deps::request` still addresses
            // its own result.
            let mut lowered: StepDeps<'step> = Deps::with_capacity_in(deps.len(), view.scratch());
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
                .finish_terminal(wrap_await_continue(finish))
        }

        ActionKind::AwaitBlock { block, finish } => {
            // The block's dep count is the statement split's, so there is nothing to lower entry
            // by entry — it rides the block door whole.
            Await::on_block(block)
                .error_frame(dep_error_frame())
                .finish_terminal(wrap_await_continue(finish))
        }

        ActionKind::Catch { watched, finish } => {
            // `watched` is realized (and owned) at apply time — an `InScope` watched enters a
            // fresh single-statement block, distinct from a dep-finish body's fan-out.
            let wrapped = catching(
                move |view: &DecideCtx<'_, 'step, '_>, result: Result<DeliveredCarried, KError>| {
                    let fctx = FinishCtx {
                        scope: view.current_scope(),
                        ctx: view.step_ctx(),
                        registries: view.registries(),
                    };
                    run_action(view, finish(&fctx, result))
                },
            );
            Outcome::Park {
                deps: ParkDeps::List(Deps::new_in(view.scratch())),
                continuation: Continuation::Catch {
                    watched,
                    finish: erase_boxed(wrapped),
                },
                dep_error_frame: None,
            }
        }
    }
}
