//! Keyworded dispatch shape: the catch-all for any expression with a
//! keyword present, or a head that isn't a fast-lane shape.

use crate::machine::core::{BlockEntry, FramePlacement};
use crate::machine::model::KExpression;
use crate::machine::{DispatchOutcome, KError, KErrorKind, NameOutcome, NodeId, TraceFrame};

use super::super::ignore_results;
use super::super::nodes::{ChainOp, NodeWork};
use super::super::obligation::with_obligation;
use super::ctx::SchedulerView;
use super::ProducerDisposition;
use super::{
    bare_name_of, park_resume, propagate_dep_error, stage_eager_part, staged_slot_placeholder,
    BareCarrier, DepRequest, Outcome, PartWalkResult, Resolved,
};
use crate::scheduler::ResolvedDeps;

/// Entry from the dispatch router. Resolved-no-parks-no-subs terminates inline; all other
/// outcomes install a park (an overload / bare-name producer wait, or eager subs) and re-enter
/// through a [`park_resume`] closure that re-runs this function on wake.
pub(super) fn initial<'step>(
    ctx: &SchedulerView<'step, '_>,
    expr: KExpression<'step>,
    idx: usize,
) -> Outcome<'step> {
    let bare_outcomes = match ctx.build_bare_outcomes(&expr.parts) {
        Ok(outcomes) => outcomes,
        Err(e) => {
            return Outcome::Done(Err(
                e.with_frame(TraceFrame::from_expr("<wrap-resolve>", &expr))
            ))
        }
    };
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
        DispatchOutcome::ParkOnProducers(producers) => {
            return install_overload_park(ctx, producers, expr, idx);
        }
    };
    // Binder placeholders / pending-overload entries were installed at statement submission from the
    // enclosing statement's parse-time aggregate (see `submit_expression`); nothing installs here.
    let covered_mask = expr.binder_plan().map(|plan| plan.chain_slot_mask);
    walk_and_invoke(
        ctx,
        resolved,
        expr.parts,
        covered_mask,
        &bare_outcomes,
        idx,
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
    ctx: &SchedulerView<'step, '_>,
    resolved: Resolved<'step>,
    parts: Vec<crate::source::Spanned<crate::machine::model::ExpressionPart<'step>>>,
    covered_mask: Option<&'static [bool]>,
    bare_outcomes: &[Option<NameOutcome<'step>>],
    idx: usize,
    park: impl FnOnce(Vec<NodeId>, KExpression<'step>) -> Outcome<'step>,
) -> Outcome<'step> {
    let walk = match part_walk(
        ctx,
        parts,
        covered_mask,
        bare_outcomes,
        &resolved.slots,
        idx,
    ) {
        Ok(w) => w,
        Err(e) => return Outcome::Done(Err(e)),
    };
    let PartWalkResult {
        new_parts,
        producers_to_wait,
        staged_subs,
    } = walk;
    let new_expr = KExpression::new(new_parts);
    if !producers_to_wait.is_empty() {
        let _ = staged_subs;
        return park(producers_to_wait, new_expr);
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
    ctx: &SchedulerView<'step, '_>,
    working_expr: KExpression<'step>,
    idx: usize,
) -> Outcome<'step> {
    let bare_outcomes = match ctx.build_bare_outcomes(&working_expr.parts) {
        Ok(outcomes) => outcomes,
        Err(e) => {
            return Outcome::Done(Err(
                e.with_frame(TraceFrame::from_expr("<wrap-resolve>", &working_expr))
            ))
        }
    };
    let scope = ctx.current_scope();
    match scope.resolve_dispatch(
        &working_expr,
        ctx.chain_deref(),
        &bare_outcomes,
        ctx.types(),
    ) {
        DispatchOutcome::Resolved(r) => {
            let covered_mask = working_expr.binder_plan().map(|plan| plan.chain_slot_mask);
            walk_and_invoke(
                ctx,
                r,
                working_expr.parts,
                covered_mask,
                &bare_outcomes,
                idx,
                park_finish,
            )
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
        DispatchOutcome::ParkOnProducers(producers) => {
            install_overload_park(ctx, producers, working_expr, idx)
        }
        DispatchOutcome::UnboundName(name) => {
            Outcome::Done(Err(KError::new(KErrorKind::UnboundName(name))))
        }
    }
}

/// Park the post-eager-subs re-resolve on the bare-name producers its splice walk leaned on; the
/// wake re-runs [`finish`] against the partly-spliced expression.
fn park_finish<'step>(producers: Vec<NodeId>, working_expr: KExpression<'step>) -> Outcome<'step> {
    let carrier = working_expr.summarize();
    park_resume(
        producers,
        Some(carrier),
        Box::new(move |ctx, idx| finish(ctx, working_expr, idx)),
    )
}

/// Fold the post-eager-subs re-resolve into a [`Outcome::Continue`]: a dep-free decide that re-runs
/// [`finish`] against the fully-spliced `working_expr` on the next pop, with no committed function
/// pick. `Inherit` — a re-resolve runs in the slot's current frame. A re-resolve inside an
/// established chain wraps the re-resolve continuation with the ambient obligation (this slot holds
/// no contract of its own), so the checker survives the hop.
pub(super) fn redispatch_continue<'step>(
    view: &SchedulerView<'step, '_>,
    working_expr: KExpression<'step>,
) -> Outcome<'step> {
    let carrier = working_expr.summarize();
    let continuation = ignore_results(Box::new(move |ctx, idx| finish(ctx, working_expr, idx)));
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

/// Park on forward-reference overload producers, filtering `producers` for cycles and
/// already-errored terminals; on wake `resume` re-runs [`initial`] against the original `expr`.
/// Visibility is widened for `single_poll::type_call`, which reuses this path for
/// forward-reference type-binder parks.
pub(in crate::machine::execute::dispatch) fn install_overload_park<'step>(
    ctx: &SchedulerView<'step, '_>,
    producers: Vec<NodeId>,
    expr: KExpression<'step>,
    idx: usize,
) -> Outcome<'step> {
    // Classify each candidate through the shared park ladder; a ready-errored producer short-circuits,
    // a ready-Ok or would-cycle producer is skipped, and a still-finalizing one joins the park set
    // (deduped by `park_on`).
    let mut to_wait = ResolvedDeps::new();
    for p in producers {
        match ctx.producer_disposition(p, NodeId(idx)) {
            ProducerDisposition::Errored(e) => {
                let frame = TraceFrame::from_expr("<dispatch-park>", &expr);
                return Outcome::Done(Err(propagate_dep_error(e, Some(frame))));
            }
            ProducerDisposition::Ready | ProducerDisposition::Cycle => {}
            ProducerDisposition::Park => {
                to_wait.park_on(p);
            }
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
    park_resume(
        to_wait.parks().to_vec(),
        Some(carrier),
        Box::new(move |ctx, idx| initial(ctx, expr, idx)),
    )
}

/// `DispatchOutcome::Deferred` arm: stage every eager part and park
/// on them, with no speculative function pick captured.
fn install_eager_only<'step>(
    ctx: &SchedulerView<'step, '_>,
    expr: KExpression<'step>,
) -> Outcome<'step> {
    // Deferred arm: no committed pick yet (resume re-resolves on finish), so no
    // bare-name slots to pre-resolve here.
    let covered_mask = expr.binder_plan().map(|plan| plan.chain_slot_mask);
    let (new_parts, mut staged_subs) = super::stage_all_eager_parts(expr.parts, &[]);
    mark_covered_subs(&mut staged_subs, covered_mask);
    debug_assert!(
        !staged_subs.is_empty(),
        "install_eager_only invoked from Deferred arm; \
         resolve_dispatch contract requires at least one eager part",
    );
    let new_expr = KExpression::new(new_parts);
    // The Deferred arm has no pre-pick, so no inline-resolved wrap slots.
    install_eager_subs_track(ctx, new_expr, staged_subs)
}

/// Mark every staged `Dispatch` at a covered chain-slot index as `binder_covered`, so a binder in a
/// binder's own eager declaration slot (`LET f = (FN …)`) rides through submission rather than being
/// rejected as an eager-position nested binder. Indices outside the mask (or all-`false` masks) leave
/// the deps uncovered. `covered_mask` is the working expression's `binder_plan().chain_slot_mask`.
fn mark_covered_subs(
    staged_subs: &mut [(usize, DepRequest<'_>)],
    covered_mask: Option<&'static [bool]>,
) {
    let Some(mask) = covered_mask else { return };
    for (index, dep) in staged_subs.iter_mut() {
        if mask.get(*index).copied().unwrap_or(false) {
            if let DepRequest::Dispatch { binder_covered, .. } = dep {
                *binder_covered = true;
            }
        }
    }
}

/// Park on bare-name forward-reference producers. `working_expr` is partly spliced — Resolved wrap
/// slots already substituted for `Spliced(obj)`; Parked wrap and ref-name slots keep their original
/// bare-name token — so on wake `resume` re-runs [`initial`] against it.
fn install_bare_name_park<'step>(
    producers: Vec<NodeId>,
    working_expr: KExpression<'step>,
) -> Outcome<'step> {
    let carrier = working_expr.summarize();
    park_resume(
        producers,
        Some(carrier),
        Box::new(move |ctx, idx| initial(ctx, working_expr, idx)),
    )
}

fn install_eager_subs_track<'step>(
    ctx: &SchedulerView<'step, '_>,
    working_expr: KExpression<'step>,
    staged_subs: Vec<(usize, DepRequest<'step>)>,
) -> Outcome<'step> {
    // The combine carrier owns its deps directly; the Keyworded eager-subs resume state is
    // never re-entered (a re-Dispatch never lands here — the combine finish runs instead).
    // The wrap slots that resolved in place are already spliced cells on `working_expr`,
    // read back by the invoke.
    ctx.install_eager_subs(working_expr, staged_subs, None)
}

/// Park the walk on `producer`, or error if the edge would close a cycle. The one place the
/// walk's cycle-check → `SchedulerDeadlock` → dedup-push ladder lives — called from both the
/// wrap-slot and ref-name arms of [`part_walk`].
fn park_walk_producer(
    ctx: &SchedulerView<'_, '_>,
    producer: NodeId,
    idx: usize,
    part: &crate::machine::model::ExpressionPart<'_>,
    producers_to_wait: &mut Vec<NodeId>,
) -> Result<(), KError> {
    if ctx.would_create_cycle(producer, NodeId(idx)) {
        let name = bare_name_of(part).unwrap_or_default();
        return Err(KError::new(KErrorKind::SchedulerDeadlock {
            pending: 1,
            sample: format!("cycle in type alias `{name}`"),
        }));
    }
    if !producers_to_wait.contains(&producer) {
        producers_to_wait.push(producer);
    }
    Ok(())
}

/// Fused splice / park / eager-sub walk over `parts`. Pure: no
/// scheduler submission, no park-edge installation — the caller
/// decides whether to install a combined park or submit the staged
/// subs. `Err(KError)` surfaces a *slot-terminal* error (cycle /
/// unbound wrap), not a scheduler-level error.
fn part_walk<'step>(
    ctx: &SchedulerView<'step, '_>,
    parts: Vec<crate::source::Spanned<crate::machine::model::ExpressionPart<'step>>>,
    covered_mask: Option<&'static [bool]>,
    bare_outcomes: &[Option<NameOutcome<'step>>],
    slots: &crate::machine::core::ClassifiedSlots,
    idx: usize,
) -> Result<PartWalkResult<'step>, KError> {
    use crate::machine::model::ExpressionPart;
    use crate::source::Spanned;

    let wrap_set = &slots.wrap_indices;
    let ref_name_set = &slots.ref_name_indices;
    let eager_filter = slots.eager_indices.as_deref();
    let mut new_parts: Vec<Spanned<ExpressionPart<'step>>> = Vec::with_capacity(parts.len());
    let mut producers_to_wait: Vec<NodeId> = Vec::new();
    let mut staged_subs: Vec<(usize, DepRequest<'step>)> = Vec::new();
    for (i, part) in parts.into_iter().enumerate() {
        let span = part.span;
        if wrap_set.contains(&i) {
            if !matches!(
                &part.value,
                ExpressionPart::Identifier(_) | ExpressionPart::Type(_)
            ) {
                debug_assert!(false, "wrap_indices implies bare-name part");
                new_parts.push(Spanned {
                    value: part.value,
                    span,
                });
                continue;
            }
            match ctx.resolve_bare_carrier(&part.value)? {
                // A resolved bound name splices inline as its binding-scope carrier — value and reach
                // as one cell. A resident read: the value lives in this scope's region, so the
                // delivery envelope's pin is the scope's own region owner (the seal-resident veneer) —
                // self-covering, identical in shape to a delivered dep.
                BareCarrier::Sealed(cell) => new_parts.push(Spanned {
                    value: ExpressionPart::Spliced { cell },
                    span,
                }),
                BareCarrier::Parked(p) => {
                    park_walk_producer(ctx, p, idx, &part.value, &mut producers_to_wait)?;
                    new_parts.push(Spanned {
                        value: part.value,
                        span,
                    });
                }
                BareCarrier::Unbound(name) => {
                    return Err(KError::new(KErrorKind::UnboundName(name)));
                }
            }
            continue;
        }
        if ref_name_set.contains(&i) {
            let park_eligible = matches!(
                &part.value,
                ExpressionPart::Identifier(_) | ExpressionPart::Type(_)
            );
            if park_eligible {
                if let Some(NameOutcome::Parked(p)) = &bare_outcomes[i] {
                    park_walk_producer(ctx, *p, idx, &part.value, &mut producers_to_wait)?;
                }
            }
            new_parts.push(Spanned {
                value: part.value,
                span,
            });
            continue;
        }
        let in_eager_filter = eager_filter.is_none_or(|idxs| idxs.contains(&i));
        if in_eager_filter {
            match stage_eager_part(part.value) {
                Ok(mut dep) => {
                    // A binder's own eager chain slot (this expr's `binder_plan().chain_slot_mask`) is
                    // covered: the enclosing statement already installed its aggregate, so the nested
                    // binder rides through submission instead of being rejected as an eager position.
                    if covered_mask.is_some_and(|m| m.get(i).copied().unwrap_or(false)) {
                        if let DepRequest::Dispatch { binder_covered, .. } = &mut dep {
                            *binder_covered = true;
                        }
                    }
                    staged_subs.push((i, dep));
                    new_parts.push(staged_slot_placeholder());
                    continue;
                }
                Err(value) => new_parts.push(Spanned { value, span }),
            }
        } else {
            new_parts.push(Spanned {
                value: part.value,
                span,
            });
        }
    }
    Ok(PartWalkResult {
        new_parts,
        producers_to_wait,
        staged_subs,
    })
}
