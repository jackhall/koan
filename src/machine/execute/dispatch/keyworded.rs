//! Keyworded dispatch shape: the catch-all for any expression with a
//! keyword present, or a head that isn't a fast-lane shape.

use crate::machine::core::ProducerId;
use crate::machine::core::{BlockEntry, FramePlacement};
use crate::machine::model::WorkingExpression;
use crate::machine::{DispatchOutcome, KError, KErrorKind, NameOutcome, NodeId};

use super::super::ignore_results;
use super::super::nodes::{ChainOp, NodeWork};
use super::super::obligation::with_obligation;
use super::ctx::SchedulerView;
use super::{
    BareCarrier, DepRequest, Outcome, PartWalkResult, Resolved, bare_name_of, park_resume,
    park_resume_labelled, stage_eager_part, staged_slot_placeholder, working_frame,
};
use crate::scheduler::ResolvedDeps;

/// Entry from the dispatch router. Resolved-no-parks-no-subs terminates inline; all other
/// outcomes install a park (an overload / bare-name claim wait, or eager subs) and re-enter
/// through a [`park_resume`] closure that re-runs this function on wake.
pub(super) fn initial<'step>(
    ctx: &SchedulerView<'_, 'step, '_>,
    expr: WorkingExpression<'step>,
    id: NodeId,
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
            return install_overload_park(ctx, sources, expr, id);
        }
    };
    // Binder name claims / pending overload slots were installed at statement submission from the
    // enclosing statement's parse-time aggregate (see `submit_expression`); nothing installs here.
    walk_and_invoke(
        ctx,
        resolved,
        expr,
        &bare_outcomes,
        id,
        install_bare_name_park,
    )
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
    ctx: &SchedulerView<'_, 'step, '_>,
    resolved: Resolved<'step>,
    expr: WorkingExpression<'step>,
    bare_outcomes: &[Option<NameOutcome>],
    id: NodeId,
    park: impl FnOnce(Vec<ProducerId>, WorkingExpression<'step>) -> Outcome<'step>,
) -> Outcome<'step> {
    let walk = match part_walk(ctx, expr.parts, bare_outcomes, &resolved.slots, id) {
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
    install_eager_subs_track(ctx, new_expr, staged_subs)
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
pub(super) fn finish<'step>(
    ctx: &SchedulerView<'_, 'step, '_>,
    working_expr: WorkingExpression<'step>,
    id: NodeId,
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
            walk_and_invoke(ctx, r, working_expr, &bare_outcomes, id, park_finish)
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
        DispatchOutcome::ParkOnProducers(sources) => {
            install_overload_park(ctx, sources, working_expr, id)
        }
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
        Box::new(move |ctx, id| finish(ctx, working_expr, id)),
    )
}

/// Fold the post-eager-subs re-resolve into a [`Outcome::Continue`]: a dep-free decide that re-runs
/// [`finish`] against the fully-spliced `working_expr` on the next pop, with no committed function
/// pick. `Inherit` — a re-resolve runs in the slot's current frame. A re-resolve inside an
/// established chain wraps the re-resolve continuation with the ambient obligation (this slot holds
/// no contract of its own), so the checker survives the hop.
pub(super) fn redispatch_continue<'step>(
    view: &SchedulerView<'_, 'step, '_>,
    working_expr: WorkingExpression<'step>,
) -> Outcome<'step> {
    let carrier = working_expr.summarize();
    let continuation = ignore_results(Box::new(move |ctx, id| finish(ctx, working_expr, id)));
    let continuation = match view.current_obligation_duplicate() {
        Some(obligation) => with_obligation(obligation, continuation),
        None => continuation,
    };
    let work = NodeWork::new(ResolvedDeps::new(), continuation, Some(carrier));
    Outcome::Continue {
        work,
        frame: FramePlacement::Inherit,
        chain: ChainOp::Unchanged,
        block_entry: BlockEntry::None,
    }
}

/// Park on forward-reference overload claims, dropping any whose edge would close a wake cycle
/// (the one pre-wiring question a decide still asks the graph); on wake `resume` re-runs
/// [`initial`] against the original `expr`. Visibility is widened for `single_poll::type_call`,
/// which reuses this path for forward-reference type-binder parks.
///
/// Whether a claim's binder has already terminalized is *not* asked here — the harness rules on
/// that when it installs the park, and the `<dispatch-park>` frame rides along on
/// `dep_error_frame` so a propagated error keeps this site's label.
pub(in crate::machine::execute::dispatch) fn install_overload_park<'step>(
    ctx: &SchedulerView<'_, 'step, '_>,
    sources: Vec<ProducerId>,
    expr: WorkingExpression<'step>,
    id: NodeId,
) -> Outcome<'step> {
    let mut to_wait: Vec<ProducerId> = Vec::new();
    for source in sources {
        if !ctx.would_create_cycle_from(source, id) && !to_wait.contains(&source) {
            to_wait.push(source);
        }
    }
    if to_wait.is_empty() {
        return Outcome::Done(Err(KError::new(KErrorKind::DispatchFailed {
            expr: expr.summarize(),
            reason: "no matching function".to_string(),
        })));
    }
    // Summarize the *original* `expr` for the deadlock report — no splice has happened yet — then
    // hand `expr` itself to the resume closure.
    let carrier = expr.summarize();
    let frame = working_frame("<dispatch-park>", &expr);
    park_resume_labelled(
        to_wait,
        Some(carrier),
        Some(frame),
        Box::new(move |ctx, id| initial(ctx, expr, id)),
    )
}

/// `DispatchOutcome::Deferred` arm: stage every eager part and park
/// on them, with no speculative function pick captured.
fn install_eager_only<'step>(
    ctx: &SchedulerView<'_, 'step, '_>,
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
    install_eager_subs_track(ctx, new_expr, staged_subs)
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
        Box::new(move |ctx, id| initial(ctx, working_expr, id)),
    )
}

fn install_eager_subs_track<'step>(
    ctx: &SchedulerView<'_, 'step, '_>,
    working_expr: WorkingExpression<'step>,
    staged_subs: Vec<(usize, DepRequest<'step>)>,
) -> Outcome<'step> {
    // The combine carrier owns its deps directly; the Keyworded eager-subs resume state is
    // never re-entered (a re-Dispatch never lands here — the combine finish runs instead).
    // The wrap slots that resolved in place are already spliced cells on `working_expr`,
    // read back by the invoke.
    ctx.install_eager_subs(working_expr, staged_subs, None)
}

/// Park the walk on `source`, or error if the edge would close a cycle. The one place the
/// walk's cycle-check → `SchedulerDeadlock` → dedup-push ladder lives — called from both the
/// wrap-slot and ref-name arms of [`part_walk`].
fn park_walk_source(
    ctx: &SchedulerView<'_, '_, '_>,
    source: ProducerId,
    id: NodeId,
    part: &crate::machine::model::ExpressionPart<'_>,
    sources_to_wait: &mut Vec<ProducerId>,
) -> Result<(), KError> {
    if ctx.would_create_cycle_from(source, id) {
        let name = bare_name_of(part).unwrap_or_default();
        return Err(KError::new(KErrorKind::SchedulerDeadlock {
            pending: 1,
            sample: format!("cycle in type alias `{name}`"),
        }));
    }
    if !sources_to_wait.contains(&source) {
        sources_to_wait.push(source);
    }
    Ok(())
}

/// Fused splice / park / eager-sub walk over `parts`. Pure: no
/// scheduler submission, no park-edge installation — the caller
/// decides whether to install a combined park or submit the staged
/// subs. `Err(KError)` surfaces a *slot-terminal* error (cycle /
/// unbound wrap), not a scheduler-level error.
fn part_walk<'step>(
    ctx: &SchedulerView<'_, 'step, '_>,
    parts: &[crate::source::Spanned<crate::machine::model::WorkingPart<'step>>],
    bare_outcomes: &[Option<NameOutcome>],
    slots: &crate::machine::core::ClassifiedSlots,
    id: NodeId,
) -> Result<PartWalkResult<'step>, KError> {
    use crate::machine::model::{ExpressionPart, WorkingPart};
    use crate::source::Spanned;

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
            match ctx.resolve_bare_carrier(&name_part) {
                // A resolved bound name splices inline as its binding-scope carrier — value and reach
                // as one cell — rested into this scope's own region. A name bound here rests for
                // free (the self rule strips this region from what is retained); one bound further
                // out lodges its binding scope's coverage, which is what keeps the read live.
                BareCarrier::Sealed(cell) => new_parts.push(Spanned {
                    value: WorkingPart::Spliced {
                        cell: ctx.current_scope().rest_delivered(&cell),
                    },
                    span,
                }),
                BareCarrier::Parked(p) => {
                    park_walk_source(ctx, p, id, &name_part, &mut sources_to_wait)?;
                    new_parts.push(*part);
                }
                BareCarrier::Unbound(name) => {
                    return Err(KError::new(KErrorKind::UnboundName(name)));
                }
            }
            continue;
        }
        if ref_name_set.contains(&i) {
            if let (true, Some(NameOutcome::Parked(p))) = (bare_name, &bare_outcomes[i]) {
                let name_part = ast.expect("bare_name implies an AST slot");
                park_walk_source(ctx, *p, id, &name_part, &mut sources_to_wait)?;
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
