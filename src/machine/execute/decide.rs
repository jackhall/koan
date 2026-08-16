//! Dispatch shape router, classifier, and shared spine — the *decide* phase.
//!
//! [`classify_dispatch`] classifies the slot via [`classify_dispatch_shape`]
//! and routes by shape:
//!
//! - **Keyworded** (a keyword is present) → [`keyworded::initial`]
//! - **FunctionValueCall** (lowercase Identifier head) →
//!   [`fn_value::initial`]
//! - **HeadDeferred** / **TypeHeadDeferred** (an `Expression` or `:(…)`
//!   head that evaluates before dispatching on its result) →
//!   [`head_deferred`]
//! - **OperatorChain** → [`operator_chain`]
//! - **TypeCall**, **BareIdentifier**, **BareTypeLeaf**,
//!   **SigiledTypeExpr**, **LiteralPassThrough** → [`single_poll`] handlers
//! - **NonCallableHead** (a literal/empty/lazy head) → a direct
//!   `DispatchFailed` raise carrying the offending head
//!
//! State and transitions live with their shape; this file keeps the cross-shape glue plus
//! [`run_action`], the shared *action* harness (a pure `Action -> Outcome` lowering). Every
//! per-shape handler *decides* against a read-only [`DecideCtx`] and returns an [`Outcome`] that
//! the harness ([`super::harness`]) applies — the harness holds the only `&mut Scheduler`, so the
//! shape modules never mutate the scheduler (nor spell its field names).

use crate::machine::ProducerId;
use crate::machine::core::RegionBrand;
use crate::machine::core::{
    Action, ActionKind, BlockEntry, FinishCtx, FramePlacement, ReturnContract, TailContract,
};
use crate::machine::model::{Carried, ExpressionPart, WorkingExpression, WorkingPart};
use crate::machine::{KError, KErrorKind, NodeId, TraceFrame};
use crate::source::Spanned;

use super::harness::KoanWorkload;
use super::ignore_results;
use super::nodes::NodeWork;
use super::obligation::{ReturnObligation, with_obligation};
use super::outcome::{TerminalDepFinish, continue_inline, dep_error_frame, tail_continue};
use crate::scheduler::{Dep, Deps};

// The dep currency lives in core (`action.rs`) so an `Action` can carry it; re-exported here as the
// decide-side view `Outcome` consumers reach through `super::decide`.
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
pub(in crate::machine::execute) use submit::SubmitContext;

#[cfg(test)]
mod tests;

pub(in crate::machine::execute) use super::outcome::{Await, Continuation, Outcome};
pub(crate) use constructors::{build_type_operand, seal_type_identity};
pub(in crate::machine::execute) use ctx::{DecideCtx, with_node_scope};
pub(crate) use field_list::{BrandCompose, FieldListDeferral};
pub(crate) use resolve::Resolution;
pub(super) use resolve::{TypeChannel, bare_name_of, type_channel};
pub use resolve_dispatch::{DispatchOutcome, Resolved};
#[cfg(test)]
pub use resolve_dispatch::{reset_resolve_dispatch_entry_count, resolve_dispatch_entry_count};

/// The shape classification and classifier live in
/// [`crate::machine::model::ast`] (pure-structural, cached on the node at parse
/// time); re-exported here so decide-internal call sites and tests keep the
/// `decide::{DispatchShape, classify_dispatch_shape}` path.
#[allow(unused_imports)]
pub(crate) use crate::machine::model::{DispatchShape, classify_dispatch_shape};

/// The staged form of one eager part shape. Private plumbing: exists so the
/// six-shape set is written exactly once (in [`eager_shape`]) while staging
/// stays by-value. Adding a shape here forces a `stage_eager_part` arm via
/// match exhaustiveness.
enum EagerShape {
    /// `(...)` — the nested inner expression dispatches directly.
    Subexpression,
    /// `:(…)` / `:{…}` — the whole part rewraps as a one-part sub-Dispatch
    /// to a type-side carrier.
    TypeExpression,
    ListLiteral,
    DictLiteral,
    RecordLiteral,
}

/// THE six-shape eager match — the only place the eager part-shape set is
/// enumerated. `None` means the part is not eager.
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

/// [`is_eager_part`] read through a dispatch slot: only a slot still holding raw AST can be eager —
/// the scheduler's own arms (a synthesized node, a resolved cell, a staging hole) are past staging.
pub(in crate::machine::execute) fn is_eager_working_part(part: &WorkingPart<'_>) -> bool {
    match part {
        // A node the scheduler synthesized is an operand awaiting its own dispatch, exactly as a
        // parsed `(...)` is — the operator-chain fold's accumulator is the one that reaches here.
        WorkingPart::Expression(_) => true,
        // A threaded record-type body is the `:{…}` eager shape with its co-declared references
        // already sealed in — staged the same way, as a one-part sub-Dispatch.
        WorkingPart::RecordType(_) => true,
        WorkingPart::Ast(ast) => is_eager_part(ast),
        WorkingPart::Spliced { .. } | WorkingPart::StagedSlot => false,
    }
}

/// Stage one slot of a working expression. A slot still holding raw AST classifies through
/// [`stage_eager_part`]; a node the scheduler synthesized (the operator-chain fold's accumulator)
/// is already a working expression, so it dispatches directly with no crossing. Every other
/// scheduler-side arm — a resolved cell, a staging hole — rides through as `Err`.
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
        // Rewrap the whole part, as the AST `:{…}` shape does: the record-type wrapper is the
        // handler, so the sub-Dispatch must see a one-part `RecordType`-shaped node, not the body.
        WorkingPart::RecordType(_) => Ok(DepRequest::Dispatch {
            expr: WorkingExpression::new(brand, vec![Spanned::bare(part)]),
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
            // Rewrap the whole part — the same shape `classify_aggregate_part`
            // builds, equivalent to the destructure-and-rewrap the walks did.
            expr: WorkingExpression::new(brand, vec![Spanned::bare(WorkingPart::Ast(part))]),
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

/// The [`WorkingPart::StagedSlot`] hole a staged slot leaves in `new_parts`, holding the
/// slot's position/index until the eager-subs finish rebuilds the run with the resolved
/// `Spliced` cell in its place.
pub(in crate::machine::execute) fn staged_slot_placeholder<'a>() -> Spanned<WorkingPart<'a>> {
    Spanned::bare(WorkingPart::StagedSlot)
}

/// The trace frame for a dispatch node — [`TraceFrame::from_expr`]'s peer for the scheduler's own
/// per-call node, resolving `span` / `file` to a source location the same way. `function` is the
/// caller-chosen label (`"<bind>"`, `"<dispatch-park>"`) for a scheduler-internal frame with no
/// `KFunction` behind it.
pub(in crate::machine::execute) fn working_frame(
    function: impl Into<String>,
    expr: &WorkingExpression<'_>,
) -> TraceFrame {
    TraceFrame {
        function: function.into(),
        expression: expr.summarize(),
        location: expr.span.zip(expr.file).map(|(span, file)| {
            crate::source::with(file, |f| {
                let (line, col_utf16) = f.resolve(span.start);
                crate::source::SourceLoc {
                    path: f.path.clone(),
                    line,
                    col_utf16,
                }
            })
        }),
    }
}

/// Clone a dep's terminal error and attach a caller-chosen frame.
/// `frame = None` is the frameless variant.
pub(in crate::machine::execute) fn propagate_dep_error(
    e: &KError,
    frame: Option<TraceFrame>,
) -> KError {
    let cloned = e.clone_for_propagation();
    match frame {
        Some(f) => cloned.with_frame(f),
        None => cloned,
    }
}

// ---------- Outcome constructors (the dispatch-currency → Outcome mapping) ----------

/// Park the slot on `sources` — the binder edges its names resolved to — and re-run its `resume`
/// decide on wake. `carrier` is the parked expression's pre-rendered summary for the deadlock
/// report (`None` when the park carries no renderable form) — rendering it here keeps the AST out
/// of the scheduler. `dep_error_frame` labels the propagation when one of those sources turns out
/// to name an already-errored producer, which the harness rules on when it installs.
pub(in crate::machine::execute) fn park_resume<'step>(
    sources: Vec<ProducerId>,
    carrier: Option<String>,
    resume: ResumeFn<'step>,
) -> Outcome<'step> {
    park_resume_labelled(sources, carrier, None, resume)
}

/// [`park_resume`] carrying an explicit dep-error frame — the park sites that label their
/// propagation (`<dispatch-park>`, `<operator-chain>`) reach for this one, so an error the install
/// surfaces is framed at the site that asked for the park rather than arriving bare.
pub(in crate::machine::execute) fn park_resume_labelled<'step>(
    sources: Vec<ProducerId>,
    carrier: Option<String>,
    dep_error_frame: Option<TraceFrame>,
    resume: ResumeFn<'step>,
) -> Outcome<'step> {
    Outcome::Park {
        deps: Deps::from_producers(sources.into_iter().map(ProducerId::scheduler_edge)),
        continuation: Continuation::Resume { carrier, resume },
        dep_error_frame,
    }
}

/// A bare-identifier slot whose name binds to the binder behind `source`: the slot's result *is*
/// that producer's result, so the harness splices the slot out (no forwarding node) — see
/// [`Outcome::Forward`].
pub(in crate::machine::execute) fn forward_to_producer<'step>(
    source: ProducerId,
) -> Outcome<'step> {
    Outcome::Forward(source.scheduler_edge())
}

/// Replace the slot with a fresh frameless `Dispatch` of `inner` — the decide reduced its
/// expression to a nested one to re-classify (`(inner)`, `:(...)` unwrap). A re-classification that
/// carries an established tail-chain obligation wraps the successor continuation with it (via
/// [`decide_tail`]), so the re-classified step re-deposits the checker rather than dropping it —
/// this slot holds no contract of its own, so the ambient obligation is the whole winner.
pub(in crate::machine::execute) fn become_dispatch<'step>(
    view: &DecideCtx<'_, 'step, '_>,
    inner: WorkingExpression<'step>,
) -> Outcome<'step> {
    continue_inline(decide_tail(inner, view.current_obligation_duplicate()))
}

/// Walk raw parts emitting a [`StagedSlot`](WorkingPart::StagedSlot) marker at every
/// eager slot and a parallel staged-subs Vec; non-eager parts pass
/// through unchanged.
///
/// `wrap_indices` names bare-name value slots (the `wrap_indices` set from
/// [`KFunction::classify_for_pick`](crate::machine::core::KFunction::classify_for_pick))
/// to resolve before bind. The keyword path resolves these via `bare_outcomes`
/// because it must know their carried type *during* overload selection; the
/// post-pick named-argument / function-value tail has already committed to one
/// callable, so it resolves them by sub-Dispatch through the same eager-subs
/// parking/resume path as `Expression` parts. Callers with no committed pick
/// (the keyworded `Deferred` arm, which re-resolves on finish) pass `&[]`.
pub(super) fn stage_all_eager_parts<'step>(
    brand: RegionBrand<'step>,
    parts: &[Spanned<WorkingPart<'step>>],
    wrap_indices: &[usize],
) -> (
    Vec<Spanned<WorkingPart<'step>>>,
    Vec<(usize, DepRequest<'step>)>,
) {
    let mut new_parts: Vec<Spanned<WorkingPart<'step>>> = Vec::with_capacity(parts.len());
    let mut staged: Vec<(usize, DepRequest<'step>)> = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        let span = part.span;
        if wrap_indices.contains(&i) {
            // Bare-name value slot: resolve the name through a single-part
            // sub-Dispatch (the `BareIdentifier` / `BareTypeLeaf` fast lane), so
            // the resolved `Spliced` carrier reaches `accepts_part` at bind. Not
            // one of the six eager shapes (it wraps bare Identifier/Type parts),
            // so this stays a pre-check before the stager.
            let wrapped = WorkingExpression::new(
                brand,
                vec![Spanned {
                    value: part.value,
                    span,
                }],
            );
            staged.push((
                i,
                DepRequest::Dispatch {
                    expr: wrapped,
                    placement: DepPlacement::OwnScope,
                },
            ));
            new_parts.push(staged_slot_placeholder());
            continue;
        }
        match stage_eager_working_part(brand, part.value) {
            Ok(dep) => {
                staged.push((i, dep));
                new_parts.push(staged_slot_placeholder());
            }
            Err(_) => new_parts.push(*part),
        }
    }
    (new_parts, staged)
}

// ---------- Resume closure ----------

/// A dispatch slot's decide — the `DecideCtx -> Outcome` closure a dispatch
/// [`NodeWork`](super::nodes::NodeWork) runs.
/// A birth decide classifies the carried `expr` and routes; a park's resume re-runs
/// the decide its park captured (a bare leaf, an evolving `working_expr`). Boxing keeps the router
/// blind to which family it is — every park wakes through the drain's step uniformly.
pub(in crate::machine::execute) type ResumeFn<'step> =
    Box<dyn for<'view> FnOnce(&DecideCtx<'_, 'step, 'view>, NodeId) -> Outcome<'step> + 'step>;

// ---------- Cross-shape driver ----------

/// Build a birth dispatch [`NodeWork`](super::nodes::NodeWork) for `expr`, wrapping the birth-dispatch
/// continuation with the tail chain's declared-return `obligation` when one is present (via
/// [`with_obligation`], so the replacement step re-deposits the checker into the ambient slot-step
/// state before classifying — the keep-first capture that carries the first caller's declared return
/// down the chain). Pass `None` for a plain birth dispatch that carries no inherited obligation.
pub(in crate::machine::execute) fn decide_tail<'step>(
    expr: WorkingExpression<'step>,
    obligation: Option<ReturnObligation>,
) -> NodeWork<'step, KoanWorkload> {
    let carrier = expr.summarize();
    // A birth decide waits on no deps: it runs on first poll, classifies, and routes.
    let continuation = ignore_results(Box::new(move |view, id| classify_dispatch(view, expr, id)));
    NodeWork::new(with_obligation(obligation, continuation), Some(carrier))
}

/// Build a [`NodeWork`](super::nodes::NodeWork) that fails on its first poll with `error`. Used by
/// submission to pre-error a nested binder in an eager sub-dispatch position: the node is slot-terminal
/// (TRY-catchable) and propagates through its dep like any other failed dep. `carrier` renders the
/// offending expression for the deadlock report.
pub(in crate::machine::execute) fn decide_error<'step>(
    error: KError,
    carrier: String,
) -> NodeWork<'step, KoanWorkload> {
    let continuation = ignore_results(Box::new(move |_view: &DecideCtx<'_, '_, '_>, _id| {
        Outcome::Done(Err(error))
    }));
    NodeWork::new(continuation, Some(carrier))
}

/// Classify a freshly-born dispatch expression's shape and route to the matching per-shape decide,
/// returning the [`Outcome`] for the harness to apply. Fast-lane shapes terminalize or
/// single-producer-park in one poll; a shape that parks returns a `Park` whose resume
/// closure re-enters the drain's step, never back through here.
fn classify_dispatch<'step>(
    view: &DecideCtx<'_, 'step, '_>,
    expr: WorkingExpression<'step>,
    id: NodeId,
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
            let name = match expr.parts[0].value {
                WorkingPart::Ast(ExpressionPart::Identifier(n)) => n,
                _ => unreachable!("BareIdentifier shape implies single Identifier part"),
            };
            single_poll::bare_identifier(view, view.current_scope(), name)
        }
        DispatchShape::FunctionValueCall => fn_value::initial(view, expr),
        DispatchShape::TypeCall => single_poll::type_call(view, expr),
        DispatchShape::HeadDeferred => head_deferred::initial_expr(view, expr),
        DispatchShape::TypeHeadDeferred => head_deferred::initial_type(view, expr),
        // Slot-terminal (TRY-catchable), uniform with every other dispatch failure —
        // a non-callable head is a runtime error, not a fatal drive abort.
        DispatchShape::NonCallableHead => {
            Outcome::Done(Err(KError::new(KErrorKind::DispatchFailed {
                expr: expr.summarize(),
                reason: format!(
                    "head is not callable: `{}`",
                    expr.parts
                        .first()
                        .map(|p| p.value.summarize())
                        .unwrap_or_else(|| "<empty>".into())
                ),
            })))
        }
        DispatchShape::OperatorChain => operator_chain::run(view, view.current_scope(), &expr, id),
        DispatchShape::Keyworded => keyworded::initial(view, expr, id),
        DispatchShape::SigiledTypeExpr => single_poll::sigiled_type_expr(view, expr),
        DispatchShape::RecordType => single_poll::record_type(view, expr),
        DispatchShape::LiteralPassThrough => single_poll::literal_pass_through(view, expr),
    }
}

// ---------- The action harness ----------

/// Lower an [`Action`] into the [`Outcome`] currency — an `Action -> Outcome` transform
/// that issues no graph write: an `AwaitDeps`/`Catch` declares its deps (and a wrapped finish that
/// recurses `run_action` on the `AwaitContinue`/`CatchContinue` it produces) as an
/// [`Outcome::Park`], and the harness wires and applies. Every scheduler read the body needs is
/// deferred into the finish, which sees a read-only [`DecideCtx`] at wake.
///
/// `view` is the executing step's read view: a tail `Action` reads its established
/// declared-return obligation off it (the ambient slot-step state) to decide keep-first and wrap the
/// replacement continuation. A finish that emits its `Continue` later reads its own wake-time view
/// instead, so the obligation it sees is the one its park deposit re-installed.
pub(in crate::machine::execute) fn run_action<'step>(
    view: &DecideCtx<'_, 'step, '_>,
    action: Action<'step>,
) -> Outcome<'step> {
    // The step's binding-table writes travel as outcome data: deposit them into the harness-owned
    // sink in the order the bodies decided them, before interpreting what happens next. Every
    // recursive arm below (a wake-time finish's `Action`) deposits through this same call, so a
    // chain of finishes contributes its writes in program order.
    view.deposit_effects(action.effects);
    match action.next {
        // Already a step-branded carrier (or error): `finalize` seals it as-is, no co-location
        // bundle.
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
                // No leading statements: tail-replace directly into the tail body.
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
            // Leading statements become owned siblings in the block (one `BodyBlock` dep); the slot
            // parks on them so they run — and reclaim — before the tail continues. Where they
            // bind is what `block_entry` names: the block frame's own scope (MATCH / TRY arms via a
            // pre-built `FreshChild` cart, FN-body tails re-entering the already-installed cart with
            // `Inherit`), or a caller-allocated overlay under the inherited call-site cart (USING).
            let placement = match &block_entry {
                BlockEntry::FrameScope(frame) => BodyPlacement::Frame(std::rc::Rc::clone(frame)),
                BlockEntry::Overlay(overlay) => BodyPlacement::Overlay(overlay),
                BlockEntry::None => unreachable!("a leading-carrying tail enters a block"),
            };
            // `FreshTail` installs its cart only at apply time — after the leading statements would
            // already have fanned out — so a leading-carrying tail cannot ride it.
            debug_assert!(
                !matches!(frame_placement, FramePlacement::FreshTail { .. }),
                "a leading-carrying tail is a FreshChild frame, an Inherit cart, or an overlay"
            );
            let finish: TerminalDepFinish<'step> = Box::new(move |view, terminals| {
                let contract = match contract {
                    TailContract::Eager(contract) => contract,
                    // The return-type expression is the last leading statement (all owned), so its
                    // resolved value is the last owned terminal, read in place in the region it was
                    // delivered into. The per-call type is re-homed into the captured-scope region —
                    // a strict ancestor the cart keeps live — like the `Type` form's `PerCall.ret`.
                    TailContract::FromLastResult { func } => {
                        let terminal = terminals[terminals.len() - 1];
                        let opened = terminal.cell.open_at();
                        let kt = match opened.value() {
                            Carried::Type(t) => t,
                            Carried::Object(other) => {
                                return Outcome::Done(Err(KError::new(KErrorKind::ShapeError(
                                    format!(
                                        "FN deferred return-type expression produced a non-type {} value",
                                        other.ktype().name(view.types()),
                                    ),
                                ))));
                            }
                            Carried::UnresolvedType(ti) => {
                                return Outcome::Done(Err(KError::new(KErrorKind::UnboundName(
                                    ti.render(),
                                ))));
                            }
                        };
                        // The resolved type is a `Copy` handle, so the contract carries it directly
                        // and outlives the sub-dispatch's terminal without naming any region.
                        Some(ReturnContract::PerCall { func, ret: kt })
                    }
                };
                // The same tail-replace as the leading-free path, against this finish's own
                // wake-time view: the park that carried the leading statements re-deposited the
                // established obligation, so a chain checks its first caller's declared return
                // rather than this resolving tail's.
                tail_continue(
                    view,
                    tail,
                    contract,
                    frame_placement,
                    block_entry,
                    body_index,
                )
            });
            Await::on(Deps::from_requests([DepRequest::BodyBlock {
                statements: leading,
                placement,
            }]))
            .error_frame(dep_error_frame())
            .finish_terminal(finish)
        }

        ActionKind::AwaitDeps { deps, finish } => {
            // The builtin assembled the dep list itself, and results come back in that order. This
            // arm maps each sub-dispatch request into the library dep currency, leaving the entries
            // the builtin already named alone, and rebuilds the `Deps` envelope `Await::on`
            // consumes; the wrapped finish recurses `run_action` on the `AwaitContinue`.
            let mut lowered: Deps<DepRequest<'step>> = Deps::new();
            for entry in deps.into_entries() {
                match entry {
                    Dep::Producer(source) => lowered.on(source),
                    Dep::Request(sub) => {
                        lowered.request(sub.into_request());
                    }
                }
            }
            let wrapped: TerminalDepFinish<'step> = Box::new(move |view, results| {
                let fctx = FinishCtx {
                    scope: view.current_scope(),
                    ctx: view.step_ctx(),
                    types: view.types(),
                };
                run_action(view, finish(&fctx, results))
            });
            Await::on(lowered)
                .error_frame(dep_error_frame())
                .finish_terminal(wrapped)
        }

        ActionKind::Catch { watched, finish } => {
            // `watched` is realized (and owned) at apply time — an `InScope` watched enters a
            // fresh single-statement block, distinct from a dep-finish body's fan-out.
            let wrapped: super::CatchFinish<'step> = Box::new(move |view, result| {
                let fctx = FinishCtx {
                    scope: view.current_scope(),
                    ctx: view.step_ctx(),
                    types: view.types(),
                };
                run_action(view, finish(&fctx, result))
            });
            Outcome::Park {
                deps: Deps::new(),
                continuation: Continuation::Catch {
                    watched,
                    finish: wrapped,
                },
                dep_error_frame: None,
            }
        }
    }
}
