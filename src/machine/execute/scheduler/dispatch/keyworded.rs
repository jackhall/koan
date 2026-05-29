//! Keyworded dispatch shape — state and transitions co-located.
//!
//! The Keyworded shape is the catch-all (any keyword present, or a
//! head that isn't a fast-lane shape). [`KeywordedState`] carries a
//! single [`ParkTrack`] (or `None` for the initial-entry shape) whose
//! three variants capture the three distinct park reasons:
//! [`ParkTrack::Overload`] (resolve returned `ParkOnProducers` before
//! the part walk ran), [`ParkTrack::BareName`] (the part walk
//! discovered ≥1 unresolved bare-name producer), and
//! [`ParkTrack::EagerSubs`] (≥1 eager sub-expression is pending). The
//! enum encodes mutual exclusivity at the type level; the resume
//! routing in [`KeywordedState::resume`] is a single `match`.

use std::marker::PhantomData;

use crate::machine::core::kfunction::KFunction;
use crate::machine::model::Parseable;
use crate::machine::model::ast::KExpression;
use crate::machine::{
    BindingIndex, Frame, KError, KErrorKind, NameOutcome, NodeId, ResolveOutcome, Scope,
};

use super::super::Scheduler;
use super::super::super::nodes::{NodeOutput, NodeStep};
use super::{
    DispatchState, EagerSubsInstall, EagerSubsTrack, Initialized, PartWalkResult, PendingSub,
    bare_name_of, propagate_dep_error,
};

pub(in crate::machine::execute) struct KeywordedState<'a> {
    pub(in crate::machine::execute) init: Initialized,
    /// Park reason carried by a `Keyworded` slot that has parked.
    /// `None` is the initial-entry shape; the transition
    /// `Initialized → Keyworded` writes `Some` when the slot stages
    /// eager subs or parks on producers.
    pub(in crate::machine::execute) track: Option<ParkTrack<'a>>,
}

/// Reason a `Keyworded` slot is parked. The three variants are mutually
/// exclusive by construction:
/// - [`ParkTrack::Overload`] — `ResolveOutcome::ParkOnProducers` arm of
///   [`KeywordedState::initial`] (and the post-eager-subs re-resolve in
///   [`KeywordedState::finish`]). The resolve fails *before* the part
///   walk runs.
/// - [`ParkTrack::BareName`] — the part walk discovered ≥1 unresolved
///   bare-name producer. The park-precedence guard installs the park
///   *before* staging any subs (submitting would leak nodes on the
///   re-Dispatch wake path).
/// - [`ParkTrack::EagerSubs`] — Resolved-with-subs or `Deferred` arm of
///   [`KeywordedState::initial`] staged ≥1 eager sub.
pub(in crate::machine::execute) enum ParkTrack<'a> {
    Overload(OverloadParkTrack<'a>),
    BareName(BareNameParkTrack<'a>),
    EagerSubs(EagerSubsTrack<'a>),
}

impl<'a> ParkTrack<'a> {
    /// Working expression carried by the track. Surfaced through
    /// [`crate::machine::execute::scheduler::dispatch::DispatchState::parked_carrier_expr`]
    /// so the drain-end cycle-detection guard can render a parked sample.
    pub(in crate::machine::execute) fn carrier_expr(&self) -> &KExpression<'a> {
        match self {
            ParkTrack::Overload(t) => &t.expr,
            ParkTrack::BareName(t) => &t.working_expr,
            ParkTrack::EagerSubs(t) => &t.working_expr,
        }
    }
}

/// Track state for the bare-name forward references a `Keyworded`
/// slot is parked on. Carries the partly-spliced `working_expr`
/// (Resolved wrap slots already substituted for `Future(obj)`; Parked
/// wrap and ref-name slots keep their original bare-name token) so the
/// re-entry can re-run [`KeywordedState::initial`] against it.
///
/// Park edges are installed as `Notify` (via `add_park_edge`): the
/// producers are sibling forward references, not children of this slot.
pub(in crate::machine::execute) struct BareNameParkTrack<'a> {
    pub(in crate::machine::execute) working_expr: KExpression<'a>,
    pub(in crate::machine::execute) producers: Vec<NodeId>,
    _ph: PhantomData<&'a KFunction<'a>>,
}

impl<'a> BareNameParkTrack<'a> {
    pub(in crate::machine::execute) fn new(
        working_expr: KExpression<'a>,
        producers: Vec<NodeId>,
    ) -> Self {
        Self { working_expr, producers, _ph: PhantomData }
    }
}

/// Track state for the forward-reference overload producers a
/// `Keyworded` slot is parked on. Carries the *original* `expr` (no
/// splice has happened yet) so the resume can hand it straight back to
/// [`KeywordedState::initial`].
///
/// Park edges are installed as `Notify` (via `add_park_edge`).
pub(in crate::machine::execute) struct OverloadParkTrack<'a> {
    pub(in crate::machine::execute) expr: KExpression<'a>,
    pub(in crate::machine::execute) producers: Vec<NodeId>,
    _ph: PhantomData<&'a KFunction<'a>>,
}

impl<'a> OverloadParkTrack<'a> {
    pub(in crate::machine::execute) fn new(
        expr: KExpression<'a>,
        producers: Vec<NodeId>,
    ) -> Self {
        Self { expr, producers, _ph: PhantomData }
    }
}

impl<'a> KeywordedState<'a> {
    pub(in crate::machine::execute) fn from_init(init: Initialized) -> Self {
        Self { init, track: None }
    }

    pub(in crate::machine::execute) fn with_eager_subs(
        init: Initialized,
        track: EagerSubsTrack<'a>,
    ) -> Self {
        Self { init, track: Some(ParkTrack::EagerSubs(track)) }
    }

    pub(in crate::machine::execute) fn with_bare_name_park(
        init: Initialized,
        track: BareNameParkTrack<'a>,
    ) -> Self {
        Self { init, track: Some(ParkTrack::BareName(track)) }
    }

    pub(in crate::machine::execute) fn with_overload_park(
        init: Initialized,
        track: OverloadParkTrack<'a>,
    ) -> Self {
        Self { init, track: Some(ParkTrack::Overload(track)) }
    }

    /// Entry from the dispatch router for the Keyworded shape. Routes
    /// the *one-shot* (Resolved, no parks, no eager subs) case to a
    /// direct terminate that inlines the placeholder install and the
    /// `function.bind` call without going through any per-variant
    /// state. The Resolved-with-parks and Resolved-with-eager-subs
    /// sub-cases install the bare-name-park / eager-subs Track on
    /// `KeywordedState`; `Deferred` folds into the eager-subs Track
    /// with no captured function; `ParkOnProducers` installs the
    /// overload-park Track. All re-entry routes through [`Self::resume`].
    pub(super) fn initial(
        sched: &mut Scheduler<'a>,
        expr: KExpression<'a>,
        pre_subs: Vec<(usize, NodeId)>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        let bare_outcomes = sched.build_bare_outcomes(&expr.parts, scope);
        // ProducerErrored short-circuit: a bare-name arg whose producer has
        // already terminalized with `Err` can never resolve.
        for outcome in bare_outcomes.iter().flatten() {
            if let NameOutcome::ProducerErrored(e) = outcome {
                let frame = Frame::from_expr("<wrap-resolve>", &expr);
                return Ok(NodeStep::Done(NodeOutput::Err(propagate_dep_error(e, Some(frame)))));
            }
        }
        let chain = sched.active_chain.as_deref();
        let outcome = scope.resolve_dispatch_with_chain(&expr, chain, &bare_outcomes);
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
                return Self::install_eager_only(sched, expr, scope, idx);
            }
            ResolveOutcome::ParkOnProducers(producers) => {
                return Ok(Self::install_overload_park(sched, producers, expr, pre_subs, idx));
            }
        };
        // Install dispatch-time placeholders.
        let lex_index = sched
            .active_chain
            .as_ref()
            .expect("dispatching slot must have an active chain")
            .index;
        let bind_index = BindingIndex {
            idx: lex_index,
            nominal_binder: resolved.function.is_nominal_binder,
        };
        if let Some(name) = resolved.placeholder_name.as_ref() {
            if let Err(e) = scope.install_placeholder(name.clone(), NodeId(idx), bind_index) {
                return Ok(NodeStep::Done(NodeOutput::Err(e)));
            }
        }
        if let Some(bucket) = resolved.pending_overload_bucket.as_ref() {
            if let Err(e) =
                scope.install_pending_overload(bucket.clone(), NodeId(idx), bind_index)
            {
                return Ok(NodeStep::Done(NodeOutput::Err(e)));
            }
        }
        let walk = match part_walk(
            sched,
            expr.parts,
            &pre_subs,
            &bare_outcomes,
            &resolved.slots,
            idx,
        ) {
            Ok(w) => w,
            Err(e) => return Ok(NodeStep::Done(NodeOutput::Err(e))),
        };
        let PartWalkResult { new_parts, producers_to_wait, staged_subs } = walk;
        let new_expr = KExpression::new(new_parts);
        if !producers_to_wait.is_empty() {
            // Park-precedence guard: drop staged_subs on the floor;
            // re-Dispatch on wake re-runs the walk and re-stages them.
            let _ = staged_subs;
            return Ok(Self::install_bare_name_park(
                sched,
                producers_to_wait,
                new_expr,
                pre_subs,
                idx,
            ));
        }
        if staged_subs.is_empty() {
            return match resolved.function.bind(new_expr) {
                Ok(future) => Ok(sched.invoke_to_step(future, scope, idx)),
                Err(e) => Ok(NodeStep::Done(NodeOutput::Err(e))),
            };
        }
        let _ = resolved; // discard the speculative pick.
        Self::install_eager_subs_track(sched, new_expr, staged_subs, pre_subs, scope, idx)
    }

    /// Resume entry. The enum-typed `track` makes mutual exclusivity a
    /// type-system fact; the match is exhaustive across park reasons.
    pub(super) fn resume(
        self,
        sched: &mut Scheduler<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        let KeywordedState { init, track } = self;
        let track = track.expect(
            "Keyworded resume is only entered after a track is installed",
        );
        match track {
            ParkTrack::Overload(OverloadParkTrack { expr, .. }) => {
                Self::initial(sched, expr, init.pre_subs, scope, idx)
            }
            ParkTrack::BareName(BareNameParkTrack { working_expr, .. }) => {
                Self::initial(sched, working_expr, init.pre_subs, scope, idx)
            }
            ParkTrack::EagerSubs(track) => sched.resume_eager_subs(track, scope, idx),
        }
    }

    /// Re-resolve completion shared between the parked-track resume and
    /// the all-subs-terminal-at-install short-circuit. Called from
    /// [`Scheduler::resume_eager_subs`] for the `picked = None`
    /// (Keyworded install) arm.
    pub(super) fn finish(
        sched: &mut Scheduler<'a>,
        working_expr: KExpression<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        match scope.resolve_dispatch(&working_expr) {
            ResolveOutcome::Resolved(r) => {
                let future = r.function.bind(working_expr)?;
                Ok(sched.invoke_to_step_pinned(future, scope, idx))
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
                sched,
                producers,
                working_expr,
                Vec::new(),
                idx,
            )),
            ResolveOutcome::UnboundName(name) => Err(KError::new(KErrorKind::UnboundName(name))),
        }
    }

    /// Realize the overload-park Track: filter `producers` for cycles
    /// and already-errored terminals, install `Notify` park edges, and
    /// transition to `Keyworded(WaitingOverloadPark)`. Cross-shape
    /// entry — also called from `single_poll::constructor_call` for
    /// forward-reference type-binder parks.
    pub(in crate::machine::execute::scheduler) fn install_overload_park(
        sched: &mut Scheduler<'a>,
        producers: Vec<NodeId>,
        expr: KExpression<'a>,
        pre_subs: Vec<(usize, NodeId)>,
        idx: usize,
    ) -> NodeStep<'a> {
        let mut to_wait: Vec<NodeId> = Vec::new();
        for p in producers {
            if sched.is_result_ready(p) {
                if let Err(e) = sched.read_result(p) {
                    let frame = Frame::from_expr("<dispatch-park>", &expr);
                    return NodeStep::Done(NodeOutput::Err(propagate_dep_error(e, Some(frame))));
                }
            } else if !sched.deps.would_create_cycle(p, NodeId(idx)) && !to_wait.contains(&p) {
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
            sched.deps.add_park_edge(*p, NodeId(idx));
        }
        let track = OverloadParkTrack::new(expr, to_wait);
        let init = Initialized { pre_subs };
        sched.replace_with_parked_dispatch(DispatchState::Keyworded(Box::new(
            Self::with_overload_park(init, track),
        )))
    }

    /// Eager-only Track installer for the `ResolveOutcome::Deferred`
    /// arm. Schedules every eager part as a sub-Dispatch (or aggregate)
    /// and parks the slot on them; on track completion the resume
    /// re-resolves dispatch against the spliced expression.
    fn install_eager_only(
        sched: &mut Scheduler<'a>,
        expr: KExpression<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        let (new_parts, staged_subs) = super::stage_all_eager_parts(expr.parts);
        debug_assert!(
            !staged_subs.is_empty(),
            "install_eager_only invoked from Deferred arm; \
             resolve_dispatch contract requires at least one eager part",
        );
        let new_expr = KExpression::new(new_parts);
        Self::install_eager_subs_track(sched, new_expr, staged_subs, Vec::new(), scope, idx)
    }

    /// Realize the bare-name park Track: install `Notify` park edges
    /// from each producer to this slot and transition to
    /// `Keyworded(WaitingBareNamePark)`.
    fn install_bare_name_park(
        sched: &mut Scheduler<'a>,
        producers: Vec<NodeId>,
        working_expr: KExpression<'a>,
        pre_subs: Vec<(usize, NodeId)>,
        idx: usize,
    ) -> NodeStep<'a> {
        for p in &producers {
            sched.deps.add_park_edge(*p, NodeId(idx));
        }
        let track = BareNameParkTrack::new(working_expr, producers);
        let init = Initialized { pre_subs };
        sched.replace_with_parked_dispatch(DispatchState::Keyworded(Box::new(
            Self::with_bare_name_park(init, track),
        )))
    }

    /// Eager-subs install: route the shared install outcome by the
    /// `AllInline`/`Parked`/`DepError` shape. The `AllInline` arm tails
    /// into [`Self::finish`] (re-resolve); the `Parked` arm wraps the
    /// track in a `KeywordedState::with_eager_subs` and replaces the
    /// slot.
    fn install_eager_subs_track(
        sched: &mut Scheduler<'a>,
        working_expr: KExpression<'a>,
        staged_subs: Vec<(usize, PendingSub<'a>)>,
        pre_subs: Vec<(usize, NodeId)>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        match sched.install_eager_subs(working_expr, staged_subs, None, scope, idx) {
            EagerSubsInstall::DepError(step) => Ok(step),
            EagerSubsInstall::AllInline(working_expr) => {
                Self::finish(sched, working_expr, scope, idx)
            }
            EagerSubsInstall::Parked(track) => {
                let init = Initialized { pre_subs };
                Ok(sched.replace_with_parked_dispatch(DispatchState::Keyworded(Box::new(
                    Self::with_eager_subs(init, track),
                ))))
            }
        }
    }
}

/// Fused splice / park / eager-sub walk over `parts`. Per part exactly
/// one arm fires: pre-sub splice (reuse recorded NodeId), wrap slot
/// (Resolved ⇒ rewrite to `Future(obj)`; Parked ⇒ cycle-check + push
/// producer; Unbound ⇒ slot-terminal `UnboundName`), ref-name slot
/// (Parked ⇒ cycle-check + push producer; Resolved / Unbound ⇒
/// no-op), or eager-sub slot (stage a sub-Dispatch / aggregate).
///
/// Pure: no scheduler submission, no park-edge installation. Caller
/// decides whether to install a combined park or submit the staged
/// subs.
///
/// `Err(KError)` surfaces a *slot terminal* error (cycle / unbound
/// wrap), not a scheduler-level error.
fn part_walk<'a>(
    sched: &mut Scheduler<'a>,
    parts: Vec<crate::machine::core::source::Spanned<crate::machine::model::ast::ExpressionPart<'a>>>,
    pre_subs: &[(usize, NodeId)],
    bare_outcomes: &[Option<NameOutcome<'a>>],
    slots: &crate::machine::core::kfunction::ClassifiedSlots,
    idx: usize,
) -> Result<PartWalkResult<'a>, KError> {
    use crate::machine::core::source::Spanned;
    use crate::machine::model::ast::{ExpressionPart, TypeParams};

    let wrap_set = &slots.wrap_indices;
    let ref_name_set = &slots.ref_name_indices;
    let eager_filter = slots.eager_indices.as_deref();
    let mut new_parts: Vec<Spanned<ExpressionPart<'a>>> = Vec::with_capacity(parts.len());
    let mut producers_to_wait: Vec<NodeId> = Vec::new();
    let mut staged_subs: Vec<(usize, PendingSub<'a>)> = Vec::new();
    for (i, part) in parts.into_iter().enumerate() {
        let span = part.span;
        if let Some(&(_, sub_id)) = pre_subs.iter().find(|(j, _)| *j == i) {
            staged_subs.push((i, PendingSub::Reuse(sub_id)));
            new_parts.push(Spanned::bare(ExpressionPart::Identifier(String::new())));
            continue;
        }
        if wrap_set.contains(&i) {
            match &bare_outcomes[i] {
                Some(NameOutcome::Resolved(obj)) => {
                    new_parts.push(Spanned { value: ExpressionPart::Future(obj), span });
                }
                Some(NameOutcome::Parked(p)) => {
                    if sched.deps.would_create_cycle(*p, NodeId(idx)) {
                        let name = bare_name_of(&part.value).unwrap_or_default();
                        return Err(KError::new(KErrorKind::SchedulerDeadlock {
                            pending: 1,
                            sample: format!("cycle in type alias `{name}`"),
                        }));
                    }
                    if !producers_to_wait.contains(p) {
                        producers_to_wait.push(*p);
                    }
                    new_parts.push(Spanned { value: part.value, span });
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
                    new_parts.push(Spanned { value: part.value, span });
                }
            }
            continue;
        }
        if ref_name_set.contains(&i) {
            let park_eligible = matches!(&part.value, ExpressionPart::Identifier(_))
                || matches!(
                    &part.value,
                    ExpressionPart::Type(t) if matches!(t.params, TypeParams::None)
                );
            if park_eligible {
                if let Some(NameOutcome::Parked(p)) = &bare_outcomes[i] {
                    if sched.deps.would_create_cycle(*p, NodeId(idx)) {
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
            new_parts.push(Spanned { value: part.value, span });
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
                other => new_parts.push(Spanned { value: other, span }),
            }
        } else {
            new_parts.push(Spanned { value: part.value, span });
        }
    }
    Ok(PartWalkResult { new_parts, producers_to_wait, staged_subs })
}
