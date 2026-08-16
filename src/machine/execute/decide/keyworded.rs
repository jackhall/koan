//! Keyworded dispatch shape: the catch-all for any expression with a
//! keyword present, or a head that isn't a fast-lane shape.

use crate::machine::ProducerId;
use crate::machine::core::OpenedFunction;
use crate::machine::model::{WorkingExpression, WorkingPart};
use crate::machine::{DispatchOutcome, KError, KErrorKind};
use crate::scheduler::Deps;
use crate::source::Spanned;

use super::super::nodes::NodeWork;
use super::super::obligation::with_obligation;
use super::super::outcome::continue_inline;
use super::super::{TerminalDepFinish, ignore_results};
use super::ctx::DecideCtx;
use super::{
    Await, DepRequest, Outcome, Resolution, Resolved, park_resume, park_resume_labelled,
    stage_eager_part, staged_slot_placeholder, working_frame,
};

/// Entry from the dispatch router. Resolved-no-parks-no-subs terminates inline; every other outcome
/// parks — on an overload / bare-name claim, or on eager subs — and re-enters through a
/// [`park_resume`] closure that re-runs this function on wake.
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
    walk_and_invoke(ctx, resolved, expr, &bare_outcomes, install_bare_name_park)
}

/// Shared [`DispatchOutcome::Resolved`] tail for [`initial`] and [`finish`]: run [`part_walk`]
/// over the pick's classified slots, then route the result. A walk that leaned on a
/// still-finalizing bare-name producer parks through `park` — each caller resumes *itself*
/// against the partly-spliced expression and drops any staged subs on the floor (park
/// precedence: the wake re-runs the caller's
/// resolve, which re-stages them). A walk that staged eager subs installs them, discarding the
/// speculative pick — the post-subs re-resolve ([`finish`]) picks again against the spliced
/// expression. Otherwise this is the synchronous call, the common path for builtins and simple
/// calls: `resolved.function` is already at the cart `'step` (resolved against the cart scope),
/// so it rides straight into the invoke, which reads each inline-resolved arg's reach off its
/// spliced cell.
fn walk_and_invoke<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    resolved: Resolved<'step>,
    expr: WorkingExpression<'step>,
    bare_outcomes: &[Option<Resolution>],
    park: impl FnOnce(Vec<ProducerId>, WorkingExpression<'step>) -> Outcome<'step>,
) -> Outcome<'step> {
    let walk = match part_walk(ctx, expr.parts, bare_outcomes, &resolved.slots) {
        Ok(w) => w,
        Err(e) => return Outcome::Done(Err(e)),
    };
    let PartWalkResult {
        new_parts,
        sources_to_wait,
        staged_subs,
    } = walk;
    // The walk spliced / staged into a fresh run; freeze it back onto this node so `span`, `file`
    // and the binder plan ride through to the invoke and to any re-resolve.
    let new_expr = expr.respliced(ctx.current_scope().brand(), new_parts);
    if !sources_to_wait.is_empty() {
        let _ = staged_subs;
        return park(sources_to_wait, new_expr);
    }
    if staged_subs.is_empty() {
        return super::exec::invoke_continue(ctx, resolved.function, new_expr);
    }
    let _ = resolved; // discard the speculative pick.
    install_eager_subs(ctx, new_expr, staged_subs, None)
}

/// Re-resolve dispatch against `working_expr` once its eager subs have spliced back in.
///
/// The re-resolve runs the same `bare_outcomes` cache + [`walk_and_invoke`] tail [`initial`]
/// does, because the arm that lands here — [`install_eager_only`], the `Deferred` outcome —
/// commits to **no** pick, and so has no wrap-slot mask to splice a bare-name argument by. A bare
/// name sharing an expression with an eager part (`(a ⊕ b) ⊕ c`, which is what a fold-left run of
/// three named operands reduces to) therefore reaches this point unresolved; the pick made here
/// against the spliced expression is what classifies it, and the walk splices it before the
/// invoke. Where [`initial`] parks back into itself, this re-resolve parks back into itself
/// ([`park_finish`]) — and a `Deferred` outcome is an error here, not another eager-subs round,
/// so the two resolves cannot ping-pong.
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
        DispatchOutcome::Resolved(r) => {
            walk_and_invoke(ctx, r, working_expr, &bare_outcomes, park_finish)
        }
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

/// Park the post-eager-subs re-resolve on the bare-name claims its splice walk leaned on; the
/// wake re-runs [`finish`] against the partly-spliced expression.
fn park_finish<'step>(
    sources: Vec<ProducerId>,
    working_expr: WorkingExpression<'step>,
) -> Outcome<'step> {
    let carrier = working_expr.summarize();
    park_resume(
        sources,
        Some(carrier),
        Box::new(move |ctx, _id| finish(ctx, working_expr)),
    )
}

/// Park on the overload claims dispatch resolution leaned on — a visible pending overload slot, or
/// a forward-reference producer a relaxed candidate needs — and re-run [`initial`] against `expr`
/// on wake. The claims are lexically-earlier binders still in flight (the binding tables' exclusive
/// cutoff keeps a statement's own claim out of its own subtree), so the wait is always
/// well-founded; the `<dispatch-park>` frame rides on `dep_error_frame` so a propagated error keeps
/// this site's label.
fn park_on_claims<'step>(
    sources: Vec<ProducerId>,
    expr: WorkingExpression<'step>,
) -> Outcome<'step> {
    // Summarize the expression for the deadlock report before handing it to the resume closure.
    let carrier = expr.summarize();
    let frame = working_frame("<dispatch-park>", &expr);
    park_resume_labelled(
        sources,
        Some(carrier),
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
    let carrier = working_expr.summarize();
    let continuation = with_obligation(
        view.current_obligation_duplicate(),
        ignore_results(Box::new(move |ctx, _id| finish(ctx, working_expr))),
    );
    continue_inline(NodeWork::new(continuation, Some(carrier)))
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
    let (new_parts, staged_subs) = super::stage_all_eager_parts(brand, expr.parts, &[]);
    debug_assert!(
        !staged_subs.is_empty(),
        "install_eager_only invoked from Deferred arm; \
         resolve_dispatch contract requires at least one eager part",
    );
    let new_expr = expr.respliced(brand, new_parts);
    // The Deferred arm has no pre-pick, so no inline-resolved wrap slots.
    install_eager_subs(ctx, new_expr, staged_subs, None)
}

/// Park on bare-name forward-reference claims. `working_expr` is partly spliced — Resolved wrap
/// slots already substituted for `Spliced(obj)`; Parked wrap and ref-name slots keep their original
/// bare-name token — so on wake `resume` re-runs [`initial`] against it.
fn install_bare_name_park<'step>(
    sources: Vec<ProducerId>,
    working_expr: WorkingExpression<'step>,
) -> Outcome<'step> {
    let carrier = working_expr.summarize();
    park_resume(
        sources,
        Some(carrier),
        Box::new(move |ctx, _id| initial(ctx, working_expr)),
    )
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
        // the run is copied out, each staging hole overwritten with its cell, and the result
        // re-frozen through `respliced` (which carries `span` / `file` / the binder plan over and
        // refills the structural cache from the spliced run).
        let scope = ctx.current_scope();
        let mut parts: Vec<Spanned<WorkingPart<'step>>> = working_expr.parts.to_vec();
        for (slot, terminal) in part_indices.iter().zip(terminals) {
            // Lift the dep's resident cell back into a delivery envelope and rest that envelope
            // into this step's own region in one door: the cell keeps the producer's carrier, the
            // envelope's whole coverage moves into the region's union bundle. That is what keeps
            // the value's backing retained until the bind reads it — which happens in this same
            // step, on the decide that folds the resolved call (`enter_user_fn`), so the cell is
            // never read across a tail hop.
            parts[*slot].value = WorkingPart::Spliced {
                cell: scope.rest_spliced(&terminal.cell),
            };
        }
        let spliced = working_expr.respliced(scope.brand(), parts);
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

/// Record `source` as a claim this walk waits on, deduped. A claim is always a lexically-earlier
/// binder still in flight, so the wait is well-founded by the language's visibility cutoff and the
/// walk asks the graph nothing.
fn wait_on(source: ProducerId, sources_to_wait: &mut Vec<ProducerId>) {
    if !sources_to_wait.contains(&source) {
        sources_to_wait.push(source);
    }
}

/// Fused splice / park / eager-sub walk over `parts`. Pure: no dep realization, no park
/// installation — the caller decides whether to park on the collected claims or on the staged
/// subs. `Err(KError)` surfaces a *slot-terminal* error (an unbound wrap name), not a
/// scheduler-level error.
fn part_walk<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    parts: &[Spanned<WorkingPart<'step>>],
    bare_outcomes: &[Option<Resolution>],
    slots: &crate::machine::core::ClassifiedSlots,
) -> Result<PartWalkResult<'step>, KError> {
    use crate::machine::model::ExpressionPart;

    let brand = ctx.current_scope().brand();
    let wrap_set = &slots.wrap_indices;
    let ref_name_set = &slots.ref_name_indices;
    let eager_filter = slots.eager_indices.as_deref();
    let mut new_parts: Vec<Spanned<WorkingPart<'step>>> = Vec::with_capacity(parts.len());
    let mut sources_to_wait: Vec<ProducerId> = Vec::new();
    let mut staged_subs: Vec<(usize, DepRequest<'step>)> = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        let span = part.span;
        // A wrap / ref-name / eager slot is decided on the parser token it still holds; a slot the
        // scheduler already filled rides through every branch below untouched.
        let ast = part.value.as_ast();
        let bare_name = matches!(
            ast,
            Some(ExpressionPart::Identifier(_) | ExpressionPart::Type(_))
        );
        if wrap_set.contains(&i) {
            let Some(name_part) = ast.filter(|_| bare_name) else {
                debug_assert!(false, "wrap_indices implies bare-name part");
                new_parts.push(*part);
                continue;
            };
            match ctx.resolve_bare(&name_part) {
                // A resolved bound name splices inline as its binding-scope carrier — value and reach
                // as one cell — rested into this scope's own region. A name bound here rests for
                // free (the self rule strips this region from what is retained); one bound further
                // out lodges its binding scope's coverage, which is what keeps the read live.
                Resolution::Resolved(cell) => new_parts.push(Spanned {
                    value: WorkingPart::Spliced {
                        cell: ctx.current_scope().rest_delivered(&cell),
                    },
                    span,
                }),
                Resolution::Parked(source) => {
                    wait_on(source, &mut sources_to_wait);
                    new_parts.push(*part);
                }
                Resolution::Unbound(name) => {
                    return Err(KError::new(KErrorKind::UnboundName(name)));
                }
            }
            continue;
        }
        if ref_name_set.contains(&i) {
            if let (true, Some(Resolution::Parked(source))) = (bare_name, &bare_outcomes[i]) {
                wait_on(*source, &mut sources_to_wait);
            }
            new_parts.push(*part);
            continue;
        }
        let in_eager_filter = eager_filter.is_none_or(|idxs| idxs.contains(&i));
        match ast
            .filter(|_| in_eager_filter)
            .map(|a| stage_eager_part(brand, a))
        {
            Some(Ok(dep)) => {
                staged_subs.push((i, dep));
                new_parts.push(staged_slot_placeholder());
            }
            _ => new_parts.push(*part),
        }
    }
    Ok(PartWalkResult {
        new_parts,
        sources_to_wait,
        staged_subs,
    })
}

/// Result of a successful keyworded part walk.
struct PartWalkResult<'step> {
    new_parts: Vec<Spanned<WorkingPart<'step>>>,
    sources_to_wait: Vec<ProducerId>,
    staged_subs: Vec<(usize, DepRequest<'step>)>,
}
