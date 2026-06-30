//! Keyworded dispatch shape: the catch-all for any expression with a
//! keyword present, or a head that isn't a fast-lane shape.

use crate::machine::core::kfunction::action::FramePlacement;
use crate::machine::model::ast::KExpression;
use crate::machine::model::values::CarriedFamily;
use crate::machine::model::{Carried, Parseable};
use crate::machine::{
    BindingIndex, FrameSet, KError, KErrorKind, NameOutcome, NodeId, ResolveOutcome, TraceFrame,
    ValueCarrierResolution,
};
use crate::witnessed::Sealed;

use super::super::ignore_results;
use super::super::nodes::NodeWork;
use super::ctx::SchedulerView;
use super::{bare_name_of, park_resume, propagate_dep_error, Outcome, PartWalkResult, PendingSub};

/// Entry from the dispatch router. Resolved-no-parks-no-subs terminates inline; all other
/// outcomes install a park (an overload / bare-name producer wait, or eager subs) and re-enter
/// through a [`park_resume`] closure that re-runs this function on wake.
pub(super) fn initial<'step>(
    ctx: &SchedulerView<'step, '_>,
    expr: KExpression<'step>,
    pre_subs: Vec<(usize, NodeId)>,
    idx: usize,
) -> Outcome<'step> {
    let bare_outcomes = ctx.build_bare_outcomes(&expr.parts);
    // A bare-name arg whose producer already errored can never resolve.
    for outcome in bare_outcomes.iter().flatten() {
        if let NameOutcome::ProducerErrored(e) = outcome {
            let frame = TraceFrame::from_expr("<wrap-resolve>", &expr);
            return Outcome::Done(Err(propagate_dep_error(e, Some(frame))));
        }
    }
    let chain = ctx.chain_deref();
    // Resolve dispatch against the cart scope at `'step`: the `Resolved` carries the picked function
    // already at the cart lifetime, so it rides straight into `invoke_continue` with no re-anchor.
    let scope = ctx.current_scope();
    let outcome = scope.resolve_dispatch(&expr, chain, &bare_outcomes);
    let resolved = match outcome {
        ResolveOutcome::Resolved(r) => r,
        // Dispatch failures are slot-terminal (TRY-catchable), uniform with the
        // bare-identifier and head-deferred lanes — not a fatal `?` abort. `interpret`
        // reads each top-level slot result and re-raises, so the CLI surfacing is unchanged.
        ResolveOutcome::Ambiguous(n) => {
            return Outcome::Done(Err(KError::new(KErrorKind::AmbiguousDispatch {
                expr: expr.summarize(),
                candidates: n,
            })));
        }
        ResolveOutcome::Unmatched => {
            return Outcome::Done(Err(KError::new(KErrorKind::DispatchFailed {
                expr: expr.summarize(),
                reason: "no matching function".to_string(),
            })));
        }
        ResolveOutcome::UnboundName(name) => {
            return Outcome::Done(Err(KError::new(KErrorKind::UnboundName(name))));
        }
        ResolveOutcome::Deferred => {
            debug_assert!(
                pre_subs.is_empty(),
                "Deferred resolve_dispatch implies no binder pick at submit time; \
                 `pre_subs` must be empty here",
            );
            return install_eager_only(ctx, expr);
        }
        ResolveOutcome::ParkOnProducers(producers) => {
            return install_overload_park(ctx, producers, expr, pre_subs, idx);
        }
    };
    let lex_index = ctx
        .active_chain()
        .expect("dispatching slot must have an active chain")
        .index;
    let bind_index = BindingIndex::value(lex_index);
    if let Some((name, kind)) = resolved.placeholder.as_ref() {
        if let Err(e) = scope.install_placeholder(name.clone(), NodeId(idx), bind_index, *kind) {
            return Outcome::Done(Err(e));
        }
    }
    if let Some(bucket) = resolved.pending_overload_bucket.as_ref() {
        if let Err(e) = scope.install_pending_overload(bucket.clone(), NodeId(idx), bind_index) {
            return Outcome::Done(Err(e));
        }
    }
    let walk = match part_walk(
        ctx,
        expr.parts,
        &pre_subs,
        &bare_outcomes,
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
        arg_carriers,
    } = walk;
    let new_expr = KExpression::new(new_parts);
    if !producers_to_wait.is_empty() {
        // Park-precedence guard: drop staged_subs (and their inline carriers) on the floor;
        // re-Dispatch on wake re-runs the walk and re-stages them.
        let _ = staged_subs;
        let _ = arg_carriers;
        return install_bare_name_park(producers_to_wait, new_expr, pre_subs);
    }
    if staged_subs.is_empty() {
        // The synchronous (no-eager-subs) call — the common path for builtins and simple calls.
        // `resolved.function` is already at the cart `'step` (resolved against the cart scope), so it
        // rides straight into the invoke. `arg_carriers` are the inline-resolved bound-name args'
        // reach carriers delivered to the body.
        return super::exec::invoke_continue(resolved.function, new_expr, arg_carriers);
    }
    let _ = resolved; // discard the speculative pick.
    install_eager_subs_track(ctx, new_expr, staged_subs, pre_subs, arg_carriers)
}

/// Re-resolve dispatch against the (now fully spliced) `working_expr`
/// after eager subs complete.
pub(super) fn finish<'step>(
    ctx: &SchedulerView<'step, '_>,
    working_expr: KExpression<'step>,
    idx: usize,
    arg_carriers: Vec<(usize, Sealed<CarriedFamily, FrameSet>)>,
) -> Outcome<'step> {
    match ctx
        .current_scope()
        .resolve_dispatch(&working_expr, ctx.chain_deref(), &[])
    {
        // The post-eager-subs re-dispatch lands resolved calls here — fold the resolved call into
        // the `Continue` that installs its frame and runs `invoke`, threading the arg carriers
        // (inline-resolved plus eager-sub) collected before the re-resolve.
        ResolveOutcome::Resolved(r) => {
            super::exec::invoke_continue(r.function, working_expr, arg_carriers)
        }
        // Slot-terminal (TRY-catchable), uniform with `initial` — a post-eager-subs
        // re-resolve failure is a runtime error TRY can intercept, not a fatal abort.
        ResolveOutcome::Ambiguous(n) => {
            Outcome::Done(Err(KError::new(KErrorKind::AmbiguousDispatch {
                expr: working_expr.summarize(),
                candidates: n,
            })))
        }
        ResolveOutcome::Deferred | ResolveOutcome::Unmatched => {
            Outcome::Done(Err(KError::new(KErrorKind::DispatchFailed {
                expr: working_expr.summarize(),
                reason: "no matching function".to_string(),
            })))
        }
        ResolveOutcome::ParkOnProducers(producers) => {
            install_overload_park(ctx, producers, working_expr, Vec::new(), idx)
        }
        ResolveOutcome::UnboundName(name) => {
            Outcome::Done(Err(KError::new(KErrorKind::UnboundName(name))))
        }
    }
}

/// Fold the post-eager-subs re-resolve into a [`Outcome::Continue`]: a dep-free decide that re-runs
/// [`finish`] against the fully-spliced `working_expr` on the next pop, with no committed function
/// pick. `Inherit` — a re-resolve runs in the slot's current frame.
pub(super) fn redispatch_continue<'step>(
    working_expr: KExpression<'step>,
    arg_carriers: Vec<(usize, Sealed<CarriedFamily, FrameSet>)>,
) -> Outcome<'step> {
    let carrier = working_expr.summarize();
    let work = NodeWork::new(
        Vec::new(),
        0,
        ignore_results(Box::new(move |ctx, idx| {
            finish(ctx, working_expr, idx, arg_carriers)
        })),
        Some(carrier),
    );
    Outcome::Continue {
        work,
        frame: FramePlacement::Inherit,
        contract: None,
        block_entry: None,
        body_index: 0,
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
    pre_subs: Vec<(usize, NodeId)>,
    idx: usize,
) -> Outcome<'step> {
    let mut to_wait: Vec<NodeId> = Vec::new();
    for p in producers {
        if ctx.is_result_ready(p) {
            if let Err(e) = ctx.result_error(p) {
                let frame = TraceFrame::from_expr("<dispatch-park>", &expr);
                return Outcome::Done(Err(propagate_dep_error(e, Some(frame))));
            }
        } else if !ctx.would_create_cycle(p, NodeId(idx)) && !to_wait.contains(&p) {
            to_wait.push(p);
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
        to_wait,
        Some(carrier),
        Box::new(move |ctx, idx| initial(ctx, expr, pre_subs, idx)),
    )
}

/// `ResolveOutcome::Deferred` arm: stage every eager part and park
/// on them, with no speculative function pick captured.
fn install_eager_only<'step>(
    ctx: &SchedulerView<'step, '_>,
    expr: KExpression<'step>,
) -> Outcome<'step> {
    // Deferred arm: no committed pick yet (resume re-resolves on finish), so no
    // bare-name slots to pre-resolve here.
    let (new_parts, staged_subs) = super::stage_all_eager_parts(expr.parts, &[]);
    debug_assert!(
        !staged_subs.is_empty(),
        "install_eager_only invoked from Deferred arm; \
         resolve_dispatch contract requires at least one eager part",
    );
    let new_expr = KExpression::new(new_parts);
    // The Deferred arm has no pre-pick, so no inline-resolved wrap slots — no inline carriers.
    install_eager_subs_track(ctx, new_expr, staged_subs, Vec::new(), Vec::new())
}

/// Park on bare-name forward-reference producers. `working_expr` is partly spliced — Resolved wrap
/// slots already substituted for `Spliced(obj)`; Parked wrap and ref-name slots keep their original
/// bare-name token — so on wake `resume` re-runs [`initial`] against it.
fn install_bare_name_park<'step>(
    producers: Vec<NodeId>,
    working_expr: KExpression<'step>,
    pre_subs: Vec<(usize, NodeId)>,
) -> Outcome<'step> {
    let carrier = working_expr.summarize();
    park_resume(
        producers,
        Some(carrier),
        Box::new(move |ctx, idx| initial(ctx, working_expr, pre_subs, idx)),
    )
}

fn install_eager_subs_track<'step>(
    ctx: &SchedulerView<'step, '_>,
    working_expr: KExpression<'step>,
    staged_subs: Vec<(usize, PendingSub<'step>)>,
    pre_subs: Vec<(usize, NodeId)>,
    inline_carriers: Vec<(usize, Sealed<CarriedFamily, FrameSet>)>,
) -> Outcome<'step> {
    // The combine carrier owns its deps directly; the Keyworded eager-subs resume state is
    // never re-entered (a re-Dispatch never lands here — the combine finish runs instead),
    // so `pre_subs` is unused on this path. `inline_carriers` are the wrap slots that resolved in
    // place; the eager-subs finish merges them with the staged subs' carriers before re-dispatch.
    let _ = pre_subs;
    ctx.install_eager_subs(working_expr, staged_subs, None, inline_carriers)
}

/// Fused splice / park / eager-sub walk over `parts`. Pure: no
/// scheduler submission, no park-edge installation — the caller
/// decides whether to install a combined park or submit the staged
/// subs. `Err(KError)` surfaces a *slot-terminal* error (cycle /
/// unbound wrap), not a scheduler-level error.
fn part_walk<'step>(
    ctx: &SchedulerView<'step, '_>,
    parts: Vec<crate::source::Spanned<crate::machine::model::ast::ExpressionPart<'step>>>,
    pre_subs: &[(usize, NodeId)],
    bare_outcomes: &[Option<NameOutcome<'step>>],
    slots: &crate::machine::core::kfunction::ClassifiedSlots,
    idx: usize,
) -> Result<PartWalkResult<'step>, KError> {
    use crate::machine::model::ast::ExpressionPart;
    use crate::source::Spanned;

    let wrap_set = &slots.wrap_indices;
    let ref_name_set = &slots.ref_name_indices;
    let eager_filter = slots.eager_indices.as_deref();
    let mut new_parts: Vec<Spanned<ExpressionPart<'step>>> = Vec::with_capacity(parts.len());
    let mut producers_to_wait: Vec<NodeId> = Vec::new();
    let mut staged_subs: Vec<(usize, PendingSub<'step>)> = Vec::new();
    let mut arg_carriers: Vec<(usize, Sealed<CarriedFamily, FrameSet>)> = Vec::new();
    for (i, part) in parts.into_iter().enumerate() {
        let span = part.span;
        if let Some(&(_, sub_id)) = pre_subs.iter().find(|(j, _)| *j == i) {
            staged_subs.push((i, PendingSub::Reuse(sub_id)));
            new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
            continue;
        }
        if wrap_set.contains(&i) {
            match &bare_outcomes[i] {
                Some(NameOutcome::Resolved(c)) => {
                    // A value-bound name spliced inline also rides on a binding-scope carrier so the
                    // body names its reach by construction (the relocate-seam reconstruction is retired
                    // for objects). A first-class **type** stays on the type channel — no carrier here
                    // (the type family inverts under `alloc_ktype`).
                    if matches!(c, Carried::Object(_)) {
                        if let Some(name) = bare_name_of(&part.value) {
                            if let ValueCarrierResolution::Value(carrier) = ctx
                                .current_scope()
                                .resolve_value_carrier(&name, ctx.chain_deref())
                            {
                                arg_carriers.push((i, Sealed::seal(carrier)));
                            }
                        }
                    }
                    new_parts.push(Spanned {
                        value: ExpressionPart::Spliced(*c),
                        span,
                    });
                }
                Some(NameOutcome::Parked(p)) => {
                    if ctx.would_create_cycle(*p, NodeId(idx)) {
                        let name = bare_name_of(&part.value).unwrap_or_default();
                        return Err(KError::new(KErrorKind::SchedulerDeadlock {
                            pending: 1,
                            sample: format!("cycle in type alias `{name}`"),
                        }));
                    }
                    if !producers_to_wait.contains(p) {
                        producers_to_wait.push(*p);
                    }
                    new_parts.push(Spanned {
                        value: part.value,
                        span,
                    });
                }
                Some(NameOutcome::Unbound(name)) => {
                    return Err(KError::new(KErrorKind::UnboundName(name.clone())));
                }
                Some(NameOutcome::Cycle(_)) => {
                    unreachable!("cache built with consumer=None never yields Cycle");
                }
                Some(NameOutcome::ProducerErrored(_)) => {
                    unreachable!("ProducerErrored short-circuited upfront");
                }
                None => {
                    debug_assert!(false, "wrap_indices implies bare-name part");
                    new_parts.push(Spanned {
                        value: part.value,
                        span,
                    });
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
                    if ctx.would_create_cycle(*p, NodeId(idx)) {
                        let name = bare_name_of(&part.value).unwrap_or_default();
                        return Err(KError::new(KErrorKind::SchedulerDeadlock {
                            pending: 1,
                            sample: format!("cycle in type alias `{name}`"),
                        }));
                    }
                    if !producers_to_wait.contains(p) {
                        producers_to_wait.push(*p);
                    }
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
            match part.value {
                ExpressionPart::Expression(boxed) => {
                    staged_subs.push((i, PendingSub::Dispatch(*boxed)));
                    new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
                    continue;
                }
                ExpressionPart::SigiledTypeExpr(boxed) => {
                    let wrapped = KExpression::new(vec![Spanned::bare(
                        ExpressionPart::SigiledTypeExpr(boxed),
                    )]);
                    staged_subs.push((i, PendingSub::Dispatch(wrapped)));
                    new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
                    continue;
                }
                ExpressionPart::RecordType(boxed) => {
                    let wrapped =
                        KExpression::new(vec![Spanned::bare(ExpressionPart::RecordType(boxed))]);
                    staged_subs.push((i, PendingSub::Dispatch(wrapped)));
                    new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
                    continue;
                }
                ExpressionPart::ListLiteral(items) => {
                    staged_subs.push((i, PendingSub::ListLit(items)));
                    new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
                    continue;
                }
                ExpressionPart::DictLiteral(pairs) => {
                    staged_subs.push((i, PendingSub::DictLit(pairs)));
                    new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
                    continue;
                }
                ExpressionPart::RecordLiteral(fields) => {
                    staged_subs.push((i, PendingSub::RecordLit(fields)));
                    new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
                    continue;
                }
                other => new_parts.push(Spanned { value: other, span }),
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
        arg_carriers,
    })
}
