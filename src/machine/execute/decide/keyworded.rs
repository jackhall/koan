//! Keyworded dispatch shape: the catch-all for any expression with a
//! keyword present, or a head that isn't a fast-lane shape.

use crate::machine::ProducerId;
use crate::machine::core::OpenedFunction;
use crate::machine::model::{WorkingExpression, WorkingPart};
use crate::machine::{DispatchOutcome, KError, KErrorKind};
use crate::scheduler::Deps;
use crate::source::Spanned;

use super::super::nodes::{NodeWork, WorkLabel};
use super::super::obligation::with_obligation;
use super::super::outcome::continue_inline;
use super::super::{TerminalDepFinish, ignore_results};
use super::ctx::DecideCtx;
use super::{
    Await, DepRequest, Outcome, PartWalk, Resolution, Resolved, park_resume_labelled,
    stage_eager_part, staged_slot_placeholder, working_frame,
};

/// Entry from the dispatch router. Resolved-no-subs terminates inline; every other outcome
/// parks — on an overload / bare-name claim ([`park_on_claims`]), or on eager subs — and
/// re-enters through a park-resume closure that re-runs this function on wake.
pub(super) fn initial<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    expr: WorkingExpression<'step>,
) -> Outcome<'step> {
    let bare_outcomes = ctx.build_bare_outcomes(expr.parts);
    let chain = ctx.chain_deref();
    // Resolve dispatch against the cart scope at `'step`: the `Resolved` carries the picked function
    // already at the cart lifetime, so it rides straight into `invoke_continue` with no re-anchor.
    let scope = ctx.current_scope();
    let outcome = scope.resolve_dispatch(&expr, chain, &bare_outcomes, ctx.types());
    let resolved = match outcome {
        DispatchOutcome::Resolved(r) => r,
        // Dispatch failures are slot-terminal (TRY-catchable), uniform with the
        // bare-identifier and head-deferred lanes — not a fatal `?` abort. `interpret`
        // reads each top-level slot result and re-raises, so the CLI surfacing is unchanged.
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
            return park_on_claims(sources, expr);
        }
    };
    // Binder name claims / pending overload slots were installed at statement submission from the
    // enclosing statement's parse-time aggregate (see `submit_expression`); nothing installs here.
    walk_and_invoke(ctx, resolved, expr, &bare_outcomes)
}

/// Shared [`DispatchOutcome::Resolved`] tail for [`initial`] and [`finish`]: run [`part_walk`]
/// over the pick's classified slots, then route the result. A walk that staged eager subs
/// installs them, discarding the speculative pick — the post-subs re-resolve ([`finish`]) picks
/// again against the spliced expression. Otherwise this is the synchronous call, the common path
/// for builtins and simple calls: `resolved.function` is already at the cart `'step` (resolved
/// against the cart scope), so it rides straight into the invoke, which reads each
/// inline-resolved arg's reach off its spliced cell.
fn walk_and_invoke<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    resolved: Resolved<'step>,
    expr: WorkingExpression<'step>,
    bare_outcomes: &[Option<Resolution>],
) -> Outcome<'step> {
    match part_walk(ctx, &expr, bare_outcomes, &resolved.slots) {
        // Nothing to splice and nothing to stage: the node the walk was handed is the node the
        // invoke wants, so it rides straight through with no rebuild.
        PartWalk::Unchanged => super::exec::invoke_continue(ctx, resolved.function, expr),
        // The walk spliced / staged into a fresh run and froze it back onto this node, so `span`,
        // `file` and the binder caches ride through to the invoke and to any re-resolve.
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
/// The re-resolve runs the same `bare_outcomes` cache + [`walk_and_invoke`] tail [`initial`]
/// does, because the arm that lands here — [`install_eager_only`], the `Deferred` outcome —
/// commits to **no** pick, and so has no wrap-slot mask to splice a bare-name argument by. A bare
/// name sharing an expression with an eager part (`(a ⊕ b) ⊕ c`, which is what a fold-left run of
/// three named operands reduces to) therefore reaches this point unresolved; the pick made here
/// against the spliced expression is what classifies it, and the walk splices it before the
/// invoke. A `Deferred` outcome is an error here, not another eager-subs round, so the two
/// resolves cannot ping-pong.
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
    ) {
        DispatchOutcome::Resolved(r) => walk_and_invoke(ctx, r, working_expr, &bare_outcomes),
        // Slot-terminal (TRY-catchable), uniform with `initial` — a post-eager-subs
        // re-resolve failure is a runtime error TRY can intercept, not a fatal abort.
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
        DispatchOutcome::ParkOnProducers(sources) => park_on_claims(sources, working_expr),
        DispatchOutcome::UnboundName(name) => {
            Outcome::Done(Err(KError::new(KErrorKind::UnboundName(name))))
        }
    }
}

/// Park on the claims dispatch resolution leaned on — a still-finalizing bare-name producer from
/// the pre-admission scan, a visible pending overload slot, or a forward-reference producer a
/// relaxed candidate needs — and re-run [`initial`] against `expr` on wake. The claims are
/// lexically-earlier binders still in flight (the binding tables' exclusive cutoff keeps a
/// statement's own claim out of its own subtree), so the wait is always well-founded; the
/// `<dispatch-park>` frame rides on `dep_error_frame` so a propagated error keeps this site's
/// label.
fn park_on_claims<'step>(
    sources: Vec<ProducerId>,
    expr: WorkingExpression<'step>,
) -> Outcome<'step> {
    let frame = working_frame("<dispatch-park>", &expr);
    park_resume_labelled(
        sources,
        Some(frame),
        Box::new(move |ctx, _id| initial(ctx, expr)),
    )
}

/// Fold the post-eager-subs re-resolve into a [`Outcome::Continue`]: a dep-free decide that re-runs
/// [`finish`] against the fully-spliced `working_expr` on the next pop, with no committed function
/// pick. A re-resolve inside an established chain wraps the re-resolve continuation with the
/// ambient obligation (this slot holds no contract of its own), so the checker survives the hop.
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

/// `DispatchOutcome::Deferred` arm: stage every eager part and park
/// on them, with no speculative function pick captured.
fn install_eager_only<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    expr: WorkingExpression<'step>,
) -> Outcome<'step> {
    // Deferred arm: no committed pick yet (resume re-resolves on finish), so no
    // bare-name slots to pre-resolve here.
    let brand = ctx.current_scope().brand();
    let (new_expr, staged_subs) = match super::stage_all_eager_parts(brand, &expr, &[]) {
        PartWalk::Respliced { expr, staged_subs } => (expr, staged_subs),
        // The `Deferred` arm's contract is at least one eager part, so the walk always stages.
        PartWalk::Unchanged => {
            debug_assert!(
                false,
                "install_eager_only invoked from Deferred arm; \
                 resolve_dispatch contract requires at least one eager part",
            );
            (expr, Vec::new())
        }
    };
    // The Deferred arm has no pre-pick, so no inline-resolved wrap slots.
    install_eager_subs(ctx, new_expr, staged_subs, None)
}

/// Park each staged eager dep and decide the eager-subs outcome. Every dep is already `DepRequest`
/// currency — every variant is a fresh owned edge the harness realizes. Nothing is read and spliced
/// inline here — that would embed a producer's frame-local terminal, which its per-call frame frees
/// at Done (it never lifts), so it would dangle. The finish rebuilds `working_expr` with the
/// resolved carriers in its staged slots — one rebuild for the whole batch, the parts run being
/// frozen once its door bumps it — and routes on `picked`: `Some(f)` folds the committed call into
/// a frame-installing `Continue`, `None` re-resolves via [`finish`]. With no deps, that routing
/// happens now. The `<bind>` dep-error frame rides on `dep_error_frame`. Pure — every write the
/// outcome implies is the harness's.
///
/// Shared with the post-pick lane in
/// [`apply_callable`](super::apply_callable::install_eager_subs_track), which commits a `picked`.
pub(super) fn install_eager_subs<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    working_expr: WorkingExpression<'step>,
    staged_subs: Vec<(usize, DepRequest<'step>)>,
    picked: Option<OpenedFunction<'step>>,
) -> Outcome<'step> {
    let (part_indices, deps): (Vec<usize>, Vec<DepRequest<'step>>) =
        staged_subs.into_iter().unzip();
    if deps.is_empty() {
        // Nothing to resolve — `working_expr` is already fully spliced, so route now not park.
        return finish_eager_subs(ctx, working_expr, picked);
    }
    let dep_error_frame = working_frame("<bind>", &working_expr);
    let finish: TerminalDepFinish<'step> = Box::new(move |ctx, terminals| {
        // Every dep resolved. Splice each value into its staged slot as the producer's own sealed
        // carrier — value and reach as one unit, adopted by the consuming bind at its own step
        // brand; `invoke` reads each cell back for the body-facing reach. Owned deps land in the
        // dep list in staging order — 1:1 with `part_indices`.
        //
        // A parts run is frozen once its door bumps it, so the whole batch lands in one rebuild:
        // the run is walked slot by slot with each staging hole replaced by its cell, straight
        // into the region's own bytes, and re-frozen through `respliced` (which carries `span` /
        // `file` / the binder plan over and refills the structural cache from the spliced run).
        // The deps were staged in slot order, so `part_indices` ascends and a single cursor over
        // it places every cell in one pass.
        let scope = ctx.current_scope();
        let parts = working_expr.parts;
        let mut filled = part_indices.iter().copied().zip(terminals).peekable();
        let spliced = working_expr.respliced(
            scope.brand(),
            parts.iter().enumerate().map(|(i, part)| {
                // Lift the dep's resident cell back into a delivery envelope and rest that
                // envelope into this step's own region in one door: the cell keeps the producer's
                // carrier, the envelope's whole coverage moves into the region's union bundle.
                // That is what keeps the value's backing retained until the bind reads it — which
                // happens in this same step, on the decide that folds the resolved call
                // (`enter_user_fn`), so the cell is never read across a tail hop.
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
    Await::on(Deps::from_requests(deps))
        .error_frame(dep_error_frame)
        .finish_terminal(finish)
}

/// Route a fully-spliced eager-subs `working_expr` to its continuation. `Some(f)` folds the
/// committed call into a frame-installing `Continue` via
/// [`invoke_continue`](super::exec::invoke_continue); `None` re-resolves via
/// [`redispatch_continue`], which re-runs [`finish`] — there an element-typed `Spliced(_)` revealed
/// by a sub surfaces as a slot-terminal `DispatchFailed`.
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

/// Fused splice / eager-sub walk over `expr`'s parts. Pure: no dep realization — the caller decides
/// whether to park on the staged subs. Infallible: dispatch resolution parks on a
/// still-finalizing bare name and admission rejects an unbound one before any pick, so every bare
/// name the pick's wrap set names is `Resolved` in `bare_outcomes` by the time the walk runs.
///
/// Staged first, rebuilt second. The two ways a slot moves are the pick's wrap set, known before
/// the walk, and an eager part, known once the stager has classified it — so the staging pass runs
/// alone, and a walk with an empty wrap set that stages nothing answers
/// [`PartWalk::Unchanged`] having built no run at all. That is the whole of a call like
/// `PRINT "hi"`: every slot passes through, so the node it was given is already the answer.
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
        // A wrap slot splices its bare name inline below rather than staging, so it is never
        // offered to the stager.
        if wrap_set.contains(&i) {
            continue;
        }
        // A literal-name slot's token (which the body reads as data) is a bare name — never an
        // eager shape — so the stager's `Err` passes it through untouched, as it does a slot the
        // scheduler already filled.
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
    // hole in one pass — the run fills the region's bytes directly, with no owned copy in between.
    let mut holes = staged_subs.iter().map(|(slot, _)| *slot).peekable();
    let expr = expr.respliced(
        brand,
        parts.iter().enumerate().map(|(i, part)| {
            if wrap_set.contains(&i) {
                // A wrap slot's bound name splices inline as its binding-scope carrier — value and
                // reach as one cell — rested into this scope's own region. A name bound here rests
                // for free (the self rule strips this region from what is retained); one bound
                // further out lodges its binding scope's coverage, which is what keeps the read
                // live.
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
