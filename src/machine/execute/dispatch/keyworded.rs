//! Keyworded dispatch shape: the catch-all for any expression with a
//! keyword present, or a head that isn't a fast-lane shape.

use std::marker::PhantomData;

use crate::machine::core::kfunction::{KFunction, SchedulerHandle};
use crate::machine::model::ast::KExpression;
use crate::machine::model::Parseable;
use crate::machine::{
    BindingIndex, Frame, KError, KErrorKind, NameOutcome, NodeId, ResolveOutcome,
};

use super::super::nodes::{NodeOutput, NodeStep};
use super::{
    bare_name_of, propagate_dep_error, DispatchCtx, DispatchState, EagerSubsInstall,
    EagerSubsTrack, Initialized, PartWalkResult, PendingSub,
};

pub(in crate::machine::execute) struct KeywordedState<'run> {
    pub(in crate::machine::execute) init: Initialized,
    /// `None` on initial entry; `Some` once the slot has parked.
    pub(in crate::machine::execute) track: Option<ParkTrack<'run>>,
}

/// Park reason for a `Keyworded` slot. Variants are mutually exclusive
/// by construction: a single resolve either parks on producers
/// (`Overload`), or runs the part walk which discovers bare-name
/// producers (`BareName`) or stages eager subs (`EagerSubs`). The
/// bare-name park must be installed *before* staging any subs — submitting
/// would leak nodes on the re-Dispatch wake path.
pub(in crate::machine::execute) enum ParkTrack<'run> {
    Overload(OverloadParkTrack<'run>),
    BareName(BareNameParkTrack<'run>),
    EagerSubs(EagerSubsTrack<'run>),
}

impl<'run> ParkTrack<'run> {
    /// Working expression carried by the track, for drain-end
    /// cycle-detection sample rendering.
    pub(in crate::machine::execute) fn carrier_expr(&self) -> &KExpression<'run> {
        match self {
            ParkTrack::Overload(t) => &t.expr,
            ParkTrack::BareName(t) => &t.working_expr,
            ParkTrack::EagerSubs(t) => &t.working_expr,
        }
    }
}

/// Track for bare-name forward-reference parks. `working_expr` is
/// partly spliced — Resolved wrap slots already substituted for
/// `Future(obj)`; Parked wrap and ref-name slots keep their original
/// bare-name token — so resume re-runs `initial` against it.
pub(in crate::machine::execute) struct BareNameParkTrack<'run> {
    pub(in crate::machine::execute) working_expr: KExpression<'run>,
    _ph: PhantomData<&'run KFunction<'run>>,
}

impl<'run> BareNameParkTrack<'run> {
    pub(in crate::machine::execute) fn new(working_expr: KExpression<'run>) -> Self {
        Self {
            working_expr,
            _ph: PhantomData,
        }
    }
}

/// Track for forward-reference overload-producer parks. Carries the
/// *original* `expr` — no splice has happened yet — so resume hands it
/// straight back to `initial`.
pub(in crate::machine::execute) struct OverloadParkTrack<'run> {
    pub(in crate::machine::execute) expr: KExpression<'run>,
    _ph: PhantomData<&'run KFunction<'run>>,
}

impl<'run> OverloadParkTrack<'run> {
    pub(in crate::machine::execute) fn new(expr: KExpression<'run>) -> Self {
        Self {
            expr,
            _ph: PhantomData,
        }
    }
}

impl<'run> KeywordedState<'run> {
    pub(in crate::machine::execute) fn with_eager_subs(
        init: Initialized,
        track: EagerSubsTrack<'run>,
    ) -> Self {
        Self {
            init,
            track: Some(ParkTrack::EagerSubs(track)),
        }
    }

    pub(in crate::machine::execute) fn with_bare_name_park(
        init: Initialized,
        track: BareNameParkTrack<'run>,
    ) -> Self {
        Self {
            init,
            track: Some(ParkTrack::BareName(track)),
        }
    }

    pub(in crate::machine::execute) fn with_overload_park(
        init: Initialized,
        track: OverloadParkTrack<'run>,
    ) -> Self {
        Self {
            init,
            track: Some(ParkTrack::Overload(track)),
        }
    }

    /// Entry from the dispatch router. Resolved-no-parks-no-subs
    /// terminates inline; all other outcomes install a `ParkTrack` and
    /// re-enter through [`Self::resume`].
    pub(super) fn initial(
        ctx: &mut DispatchCtx<'run, '_>,
        expr: KExpression<'run>,
        pre_subs: Vec<(usize, NodeId)>,
        idx: usize,
    ) -> Result<NodeStep<'run>, KError> {
        let bare_outcomes = ctx.build_bare_outcomes(&expr.parts);
        // A bare-name arg whose producer already errored can never resolve.
        for outcome in bare_outcomes.iter().flatten() {
            if let NameOutcome::ProducerErrored(e) = outcome {
                let frame = Frame::from_expr("<wrap-resolve>", &expr);
                return Ok(NodeStep::Done(NodeOutput::Err(propagate_dep_error(
                    e,
                    Some(frame),
                ))));
            }
        }
        let chain = ctx.chain_deref();
        let outcome = ctx
            .current_scope()
            .resolve_dispatch(&expr, chain, &bare_outcomes);
        let resolved = match outcome {
            ResolveOutcome::Resolved(r) => r,
            ResolveOutcome::Ambiguous(n) => {
                return Err(KError::new(KErrorKind::AmbiguousDispatch {
                    expr: expr.summarize(),
                    candidates: n,
                }));
            }
            ResolveOutcome::Unmatched => {
                return Err(KError::new(KErrorKind::DispatchFailed {
                    expr: expr.summarize(),
                    reason: "no matching function".to_string(),
                }));
            }
            ResolveOutcome::UnboundName(name) => {
                return Err(KError::new(KErrorKind::UnboundName(name)));
            }
            ResolveOutcome::Deferred => {
                debug_assert!(
                    pre_subs.is_empty(),
                    "Deferred resolve_dispatch implies no binder pick at submit time; \
                     `pre_subs` must be empty here",
                );
                return Self::install_eager_only(ctx, expr, idx);
            }
            ResolveOutcome::ParkOnProducers(producers) => {
                return Ok(Self::install_overload_park(
                    ctx, producers, expr, pre_subs, idx,
                ));
            }
        };
        let lex_index = ctx
            .active_chain()
            .expect("dispatching slot must have an active chain")
            .index;
        let bind_index = BindingIndex::value(lex_index);
        if let Some(name) = resolved.placeholder_name.as_ref() {
            if let Err(e) =
                ctx.current_scope()
                    .install_placeholder(name.clone(), NodeId(idx), bind_index)
            {
                return Ok(NodeStep::Done(NodeOutput::Err(e)));
            }
        }
        if let Some(bucket) = resolved.pending_overload_bucket.as_ref() {
            if let Err(e) = ctx.current_scope().install_pending_overload(
                bucket.clone(),
                NodeId(idx),
                bind_index,
            ) {
                return Ok(NodeStep::Done(NodeOutput::Err(e)));
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
            Err(e) => return Ok(NodeStep::Done(NodeOutput::Err(e))),
        };
        let PartWalkResult {
            new_parts,
            producers_to_wait,
            staged_subs,
        } = walk;
        let new_expr = KExpression::new(new_parts);
        if !producers_to_wait.is_empty() {
            // Park-precedence guard: drop staged_subs on the floor;
            // re-Dispatch on wake re-runs the walk and re-stages them.
            let _ = staged_subs;
            return Ok(Self::install_bare_name_park(
                ctx,
                producers_to_wait,
                new_expr,
                pre_subs,
                idx,
            ));
        }
        if staged_subs.is_empty() {
            // The synchronous (no-eager-subs) call — the common path for builtins and simple calls.
            let body = super::exec::invoke(ctx, resolved.function, new_expr);
            return Ok(ctx.body_result_to_step(body, idx));
        }
        let _ = resolved; // discard the speculative pick.
        Self::install_eager_subs_track(ctx, new_expr, staged_subs, pre_subs, idx)
    }

    /// Resume entry, dispatched on the installed `ParkTrack` variant.
    pub(super) fn resume(
        self,
        ctx: &mut DispatchCtx<'run, '_>,
        idx: usize,
    ) -> Result<NodeStep<'run>, KError> {
        let KeywordedState { init, track } = self;
        let track = track.expect("Keyworded resume is only entered after a track is installed");
        match track {
            ParkTrack::Overload(OverloadParkTrack { expr, .. }) => {
                Self::initial(ctx, expr, init.pre_subs, idx)
            }
            ParkTrack::BareName(BareNameParkTrack { working_expr, .. }) => {
                Self::initial(ctx, working_expr, init.pre_subs, idx)
            }
            ParkTrack::EagerSubs(track) => ctx.resume_eager_subs(track, idx),
        }
    }

    /// Re-resolve dispatch against the (now fully spliced) `working_expr`
    /// after eager subs complete.
    pub(super) fn finish(
        ctx: &mut DispatchCtx<'run, '_>,
        working_expr: KExpression<'run>,
        idx: usize,
    ) -> Result<NodeStep<'run>, KError> {
        match ctx
            .current_scope()
            .resolve_dispatch(&working_expr, ctx.chain_deref(), &[])
        {
            // The post-eager-subs re-dispatch lands resolved calls here.
            ResolveOutcome::Resolved(r) => {
                let body = super::exec::invoke(ctx, r.function, working_expr);
                Ok(ctx.body_result_to_step(body, idx))
            }
            ResolveOutcome::Ambiguous(n) => Err(KError::new(KErrorKind::AmbiguousDispatch {
                expr: working_expr.summarize(),
                candidates: n,
            })),
            ResolveOutcome::Deferred | ResolveOutcome::Unmatched => {
                Err(KError::new(KErrorKind::DispatchFailed {
                    expr: working_expr.summarize(),
                    reason: "no matching function".to_string(),
                }))
            }
            ResolveOutcome::ParkOnProducers(producers) => Ok(Self::install_overload_park(
                ctx,
                producers,
                working_expr,
                Vec::new(),
                idx,
            )),
            ResolveOutcome::UnboundName(name) => Err(KError::new(KErrorKind::UnboundName(name))),
        }
    }

    /// Realize an overload-park Track, filtering `producers` for cycles
    /// and already-errored terminals. Visibility is widened for
    /// `single_poll::type_call`, which reuses this path for
    /// forward-reference type-binder parks.
    pub(in crate::machine::execute::dispatch) fn install_overload_park(
        ctx: &mut DispatchCtx<'run, '_>,
        producers: Vec<NodeId>,
        expr: KExpression<'run>,
        pre_subs: Vec<(usize, NodeId)>,
        idx: usize,
    ) -> NodeStep<'run> {
        let mut to_wait: Vec<NodeId> = Vec::new();
        for p in producers {
            if ctx.is_result_ready(p) {
                if let Err(e) = ctx.read_result(p) {
                    let frame = Frame::from_expr("<dispatch-park>", &expr);
                    return NodeStep::Done(NodeOutput::Err(propagate_dep_error(e, Some(frame))));
                }
            } else if !ctx.would_create_cycle(p, NodeId(idx)) && !to_wait.contains(&p) {
                to_wait.push(p);
            }
        }
        if to_wait.is_empty() {
            return NodeStep::Done(NodeOutput::Err(KError::new(KErrorKind::DispatchFailed {
                expr: expr.summarize(),
                reason: "no matching function".to_string(),
            })));
        }
        for p in &to_wait {
            ctx.add_park_edge(*p, NodeId(idx));
        }
        let track = OverloadParkTrack::new(expr);
        let init = Initialized { pre_subs };
        ctx.replace_with_parked_dispatch(DispatchState::Keyworded(Box::new(
            Self::with_overload_park(init, track),
        )))
    }

    /// `ResolveOutcome::Deferred` arm: stage every eager part and park
    /// on them, with no speculative function pick captured.
    fn install_eager_only(
        ctx: &mut DispatchCtx<'run, '_>,
        expr: KExpression<'run>,
        idx: usize,
    ) -> Result<NodeStep<'run>, KError> {
        // Deferred arm: no committed pick yet (resume re-resolves on finish), so no
        // bare-name slots to pre-resolve here.
        let (new_parts, staged_subs) = super::stage_all_eager_parts(expr.parts, &[]);
        debug_assert!(
            !staged_subs.is_empty(),
            "install_eager_only invoked from Deferred arm; \
             resolve_dispatch contract requires at least one eager part",
        );
        let new_expr = KExpression::new(new_parts);
        Self::install_eager_subs_track(ctx, new_expr, staged_subs, Vec::new(), idx)
    }

    fn install_bare_name_park(
        ctx: &mut DispatchCtx<'run, '_>,
        producers: Vec<NodeId>,
        working_expr: KExpression<'run>,
        pre_subs: Vec<(usize, NodeId)>,
        idx: usize,
    ) -> NodeStep<'run> {
        for p in &producers {
            ctx.add_park_edge(*p, NodeId(idx));
        }
        let track = BareNameParkTrack::new(working_expr);
        let init = Initialized { pre_subs };
        ctx.replace_with_parked_dispatch(DispatchState::Keyworded(Box::new(
            Self::with_bare_name_park(init, track),
        )))
    }

    fn install_eager_subs_track(
        ctx: &mut DispatchCtx<'run, '_>,
        working_expr: KExpression<'run>,
        staged_subs: Vec<(usize, PendingSub<'run>)>,
        pre_subs: Vec<(usize, NodeId)>,
        idx: usize,
    ) -> Result<NodeStep<'run>, KError> {
        match ctx.install_eager_subs(working_expr, staged_subs, None, idx) {
            EagerSubsInstall::DepError(step) => Ok(step),
            EagerSubsInstall::AllInline(working_expr) => Self::finish(ctx, working_expr, idx),
            EagerSubsInstall::Parked(track) => {
                let init = Initialized { pre_subs };
                Ok(
                    ctx.replace_with_parked_dispatch(DispatchState::Keyworded(Box::new(
                        Self::with_eager_subs(init, track),
                    ))),
                )
            }
        }
    }
}

/// Fused splice / park / eager-sub walk over `parts`. Pure: no
/// scheduler submission, no park-edge installation — the caller
/// decides whether to install a combined park or submit the staged
/// subs. `Err(KError)` surfaces a *slot-terminal* error (cycle /
/// unbound wrap), not a scheduler-level error.
fn part_walk<'run>(
    ctx: &mut DispatchCtx<'run, '_>,
    parts: Vec<
        crate::machine::core::source::Spanned<crate::machine::model::ast::ExpressionPart<'run>>,
    >,
    pre_subs: &[(usize, NodeId)],
    bare_outcomes: &[Option<NameOutcome<'run>>],
    slots: &crate::machine::core::kfunction::ClassifiedSlots,
    idx: usize,
) -> Result<PartWalkResult<'run>, KError> {
    use crate::machine::core::source::Spanned;
    use crate::machine::model::ast::ExpressionPart;

    let wrap_set = &slots.wrap_indices;
    let ref_name_set = &slots.ref_name_indices;
    let eager_filter = slots.eager_indices.as_deref();
    let mut new_parts: Vec<Spanned<ExpressionPart<'run>>> = Vec::with_capacity(parts.len());
    let mut producers_to_wait: Vec<NodeId> = Vec::new();
    let mut staged_subs: Vec<(usize, PendingSub<'run>)> = Vec::new();
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
                    new_parts.push(Spanned {
                        value: ExpressionPart::Future(*c),
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
    })
}
