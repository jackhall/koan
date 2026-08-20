//! Keyworded dispatch shape: the catch-all for any expression with a
//! keyword present, or a head that isn't a fast-lane shape.

use crate::machine::ProducerId;
use crate::machine::core::OpenedFunction;
use crate::machine::model::{WorkingExpression, WorkingPart};
use crate::machine::{DispatchOutcome, KError, KErrorKind};
use crate::scheduler::Deps;
use crate::source::Spanned;
use crate::witnessed::BumpAllocator;

use super::super::nodes::{NodeWork, WorkLabel};
use super::super::obligation::with_obligation;
use super::super::outcome::continue_inline;
use super::super::{TerminalDepFinish, ignore_results};
use super::ctx::DecideCtx;
use super::{
    Await, DepRequest, Outcome, PartWalk, Resolution, Resolved, park_resume_labelled,
    stage_eager_part, staged_slot_placeholder, working_frame,
};

/// Entry from the dispatch router. Resolution failures are slot-terminal (TRY-catchable), uniform
/// with the bare-identifier and head-deferred lanes; `ParkOnProducers` re-runs this function on
/// wake and `Deferred` re-resolves through [`finish`] once its eager subs land.
pub(super) fn initial<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    expr: WorkingExpression<'step>,
) -> Outcome<'step> {
    let bare_outcomes = ctx.build_bare_outcomes(expr.parts);
    let chain = ctx.chain_deref();
    // Resolving against the cart scope puts the pick at the cart `'step` lifetime, so it reaches
    // `invoke_continue` with no re-anchor.
    let scope = ctx.current_scope();
    let outcome = scope.resolve_dispatch(&expr, chain, &bare_outcomes, ctx.types(), ctx.scratch());
    let resolved = match outcome {
        DispatchOutcome::Resolved(r) => r,
        DispatchOutcome::Ambiguous(n) => {
            return Outcome::Done(Err(KError::new(KErrorKind::AmbiguousDispatch {
                expr: expr.summarize(),
                candidates: n,
            })));
        }
        DispatchOutcome::Unmatched => {
            return Outcome::Done(Err(KError::new(KErrorKind::DispatchFailed {
                expr: expr.summarize(),
                reason: "no matching function".to_string(),
            })));
        }
        DispatchOutcome::UnboundName(name) => {
            return Outcome::Done(Err(KError::new(KErrorKind::UnboundName(name))));
        }
        DispatchOutcome::Deferred => {
            return install_eager_only(ctx, expr);
        }
        DispatchOutcome::ParkOnProducers(sources) => {
            return park_on_claims(sources, expr, ctx.scratch());
        }
    };
    // Binder name claims / pending overload slots are installed at statement submission from the
    // enclosing statement's parse-time aggregate (see `submit_expression`); nothing installs here.
    walk_and_invoke(ctx, resolved, expr, &bare_outcomes)
}

/// Shared [`DispatchOutcome::Resolved`] tail for [`initial`] and [`finish`]. A walk that staged
/// eager subs discards the speculative pick, because the post-subs re-resolve ([`finish`]) picks
/// again against the spliced expression. Otherwise this is the synchronous call — the common path
/// for builtins and simple calls.
fn walk_and_invoke<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    resolved: Resolved<'step>,
    expr: WorkingExpression<'step>,
    bare_outcomes: &[Option<Resolution>],
) -> Outcome<'step> {
    match part_walk(ctx, &expr, bare_outcomes, &resolved.slots) {
        PartWalk::Unchanged => super::exec::invoke_continue(ctx, resolved.function, expr),
        PartWalk::Respliced {
            expr: new_expr,
            staged_subs,
        } => {
            if staged_subs.is_empty() {
                return super::exec::invoke_continue(ctx, resolved.function, new_expr);
            }
            let _ = resolved; // discard the speculative pick.
            install_eager_subs(ctx, new_expr, staged_subs, None)
        }
    }
}

/// Re-resolve dispatch against `working_expr` once its eager subs have spliced back in.
///
/// The re-resolve runs the same `bare_outcomes` cache + [`walk_and_invoke`] tail [`initial`] does,
/// because every arm that routes here commits **no** pick and so carries no wrap-slot mask to
/// splice a bare-name argument by. A bare name sharing an expression with an eager part
/// (`(a ⊕ b) ⊕ c`, what a fold-left run of three named operands reduces to) therefore reaches this
/// point unresolved; the pick made here against the spliced expression is what classifies it, and
/// the walk splices it before the invoke. A `Deferred` outcome is an error here, not another
/// eager-subs round, so the two resolves cannot ping-pong.
fn finish<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    working_expr: WorkingExpression<'step>,
) -> Outcome<'step> {
    let bare_outcomes = ctx.build_bare_outcomes(working_expr.parts);
    let scope = ctx.current_scope();
    match scope.resolve_dispatch(
        &working_expr,
        ctx.chain_deref(),
        &bare_outcomes,
        ctx.types(),
        ctx.scratch(),
    ) {
        DispatchOutcome::Resolved(r) => walk_and_invoke(ctx, r, working_expr, &bare_outcomes),
        DispatchOutcome::Ambiguous(n) => {
            Outcome::Done(Err(KError::new(KErrorKind::AmbiguousDispatch {
                expr: working_expr.summarize(),
                candidates: n,
            })))
        }
        DispatchOutcome::Deferred | DispatchOutcome::Unmatched => {
            Outcome::Done(Err(KError::new(KErrorKind::DispatchFailed {
                expr: working_expr.summarize(),
                reason: "no matching function".to_string(),
            })))
        }
        DispatchOutcome::ParkOnProducers(sources) => {
            park_on_claims(sources, working_expr, ctx.scratch())
        }
        DispatchOutcome::UnboundName(name) => {
            Outcome::Done(Err(KError::new(KErrorKind::UnboundName(name))))
        }
    }
}

/// Park on the claims dispatch resolution leaned on — a still-finalizing bare-name producer from
/// the pre-admission scan, a visible pending overload slot, or a forward-reference producer a
/// relaxed candidate needs — and re-run [`initial`] against `expr` on wake. The claims are
/// lexically-earlier binders still in flight (the binding tables' exclusive cutoff keeps a
/// statement's own claim out of its own subtree), so the wait is always well-founded.
fn park_on_claims<'step>(
    sources: Vec<ProducerId>,
    expr: WorkingExpression<'step>,
    scratch: BumpAllocator<'step>,
) -> Outcome<'step> {
    let frame = working_frame("<dispatch-park>", &expr);
    park_resume_labelled(
        sources,
        Some(frame),
        scratch,
        Box::new(move |ctx, _id| initial(ctx, expr)),
    )
}

/// Fold the post-eager-subs re-resolve into a dep-free decide that re-runs [`finish`] on the next
/// pop. The re-resolve slot holds no contract of its own, so the continuation carries the ambient
/// obligation and the declared-return checker survives the hop.
fn redispatch_continue<'step>(
    view: &DecideCtx<'_, 'step, '_>,
    working_expr: WorkingExpression<'step>,
) -> Outcome<'step> {
    let label = WorkLabel::of(&working_expr);
    let continuation = with_obligation(
        view.current_obligation_duplicate(),
        ignore_results(Box::new(move |ctx, _id| finish(ctx, working_expr))),
    );
    continue_inline(NodeWork::new(continuation), label)
}

/// `DispatchOutcome::Deferred` arm: stage every eager part and park on them. No pick is captured
/// and no wrap set exists, so no bare-name slot is pre-resolved here — [`finish`] re-resolves.
fn install_eager_only<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    expr: WorkingExpression<'step>,
) -> Outcome<'step> {
    let brand = ctx.current_scope().brand();
    let (new_expr, staged_subs) = match super::stage_all_eager_parts(brand, &expr, &[]) {
        PartWalk::Respliced { expr, staged_subs } => (expr, staged_subs),
        PartWalk::Unchanged => {
            debug_assert!(
                false,
                "install_eager_only invoked from Deferred arm; \
                 resolve_dispatch contract requires at least one eager part",
            );
            (expr, Vec::new())
        }
    };
    install_eager_subs(ctx, new_expr, staged_subs, None)
}

/// Park each staged eager dep, then rebuild `working_expr` with the resolved carriers in its
/// staged slots and route on `picked`. Nothing is read and spliced inline before the park — that
/// would embed a producer's frame-local terminal, which its per-call frame frees at Done (it never
/// lifts), so it would dangle. Pure: every write the outcome implies is the harness's.
///
/// Shared with the post-pick lane
/// [`install_eager_subs_track`](super::apply_callable::install_eager_subs_track), which commits a
/// `picked`.
pub(super) fn install_eager_subs<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    working_expr: WorkingExpression<'step>,
    staged_subs: Vec<(usize, DepRequest<'step>)>,
    picked: Option<OpenedFunction<'step>>,
) -> Outcome<'step> {
    let (part_indices, deps): (Vec<usize>, Vec<DepRequest<'step>>) =
        staged_subs.into_iter().unzip();
    if deps.is_empty() {
        return finish_eager_subs(ctx, working_expr, picked);
    }
    let dep_error_frame = working_frame("<bind>", &working_expr);
    let finish: TerminalDepFinish<'step> = Box::new(move |ctx, terminals| {
        // A parts run is frozen once its door bumps it, so the whole batch must land in one
        // rebuild. Deps land in staging order, so `part_indices` ascends 1:1 with `terminals` and
        // a single cursor over it places every cell in one pass.
        let scope = ctx.current_scope();
        let parts = working_expr.parts;
        let mut filled = part_indices.iter().copied().zip(terminals).peekable();
        let spliced = working_expr.respliced(
            scope.brand(),
            parts.iter().enumerate().map(|(i, part)| {
                // Resting the dep's cell into this step's region is what keeps the value's backing
                // retained until the bind reads it — which happens in this same step, on the
                // decide that folds the resolved call, so the cell is never read across a tail hop.
                match filled.next_if(|(slot, _)| *slot == i) {
                    Some((_, terminal)) => Spanned {
                        value: WorkingPart::Spliced {
                            cell: scope.rest_spliced(&terminal.cell),
                        },
                        span: part.span,
                    },
                    None => *part,
                }
            }),
        );
        finish_eager_subs(ctx, spliced, picked)
    });
    Await::on(Deps::from_requests_in(deps, ctx.scratch()))
        .error_frame(dep_error_frame)
        .finish_terminal(finish)
}

/// Route a fully-spliced eager-subs `working_expr` to its continuation. Under `None` the re-resolve
/// sees the element types the subs revealed, so a `Spliced(_)` no candidate satisfies surfaces
/// there as a slot-terminal `DispatchFailed` rather than as a bind-time error.
fn finish_eager_subs<'step>(
    view: &DecideCtx<'_, 'step, '_>,
    working_expr: WorkingExpression<'step>,
    picked: Option<OpenedFunction<'step>>,
) -> Outcome<'step> {
    match picked {
        Some(f) => super::exec::invoke_continue(view, f, working_expr),
        None => redispatch_continue(view, working_expr),
    }
}

/// Splice / eager-sub walk over `expr`'s parts. Pure: no dep realization — the caller decides
/// whether to park on the staged subs. Infallible: dispatch resolution parks on a still-finalizing
/// bare name and admission rejects an unbound one before any pick, so every bare name the pick's
/// wrap set names is `Resolved` in `bare_outcomes` by the time the walk runs.
///
/// Staging runs as its own pass first, because the second way a slot moves — being an eager part —
/// is only known once the stager has classified it, while the wrap set is known up front. A walk
/// that moves nothing therefore answers [`PartWalk::Unchanged`] having built no run at all.
fn part_walk<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    expr: &WorkingExpression<'step>,
    bare_outcomes: &[Option<Resolution>],
    slots: &crate::machine::core::ClassifiedSlots,
) -> PartWalk<'step> {
    let brand = ctx.current_scope().brand();
    let parts = expr.parts;
    let wrap_set = &slots.wrap_indices;
    let eager_filter = slots.eager_indices.as_deref();
    let mut staged_subs: Vec<(usize, DepRequest<'step>)> = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        // A wrap slot splices inline below rather than staging, so it is never offered to the
        // stager.
        if wrap_set.contains(&i) {
            continue;
        }
        let in_eager_filter = eager_filter.is_none_or(|idxs| idxs.contains(&i));
        if let Some(Ok(dep)) = part
            .value
            .as_ast()
            .filter(|_| in_eager_filter)
            .map(|a| stage_eager_part(brand, a))
        {
            staged_subs.push((i, dep));
        }
    }
    if wrap_set.is_empty() && staged_subs.is_empty() {
        return PartWalk::Unchanged;
    }
    // Staging ran in slot order, so a single ascending cursor over the staged indices places every
    // hole in one pass.
    let mut holes = staged_subs.iter().map(|(slot, _)| *slot).peekable();
    let expr = expr.respliced(
        brand,
        parts.iter().enumerate().map(|(i, part)| {
            if wrap_set.contains(&i) {
                // A name bound in this region rests for free (the self rule strips this region from
                // what is retained); one bound further out lodges its binding scope's coverage,
                // which is what keeps the read live.
                let Some(Resolution::Resolved(cell)) =
                    bare_outcomes.get(i).and_then(|o| o.as_ref())
                else {
                    unreachable!("a picked wrap slot's bare name has a Resolved cache entry");
                };
                return Spanned {
                    value: WorkingPart::Spliced {
                        cell: ctx.current_scope().rest_delivered(cell),
                    },
                    span: part.span,
                };
            }
            if holes.next_if_eq(&i).is_some() {
                return staged_slot_placeholder();
            }
            *part
        }),
    );
    PartWalk::Respliced { expr, staged_subs }
}
