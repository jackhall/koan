//! FunctionValueCall dispatch shape — state and transitions
//! co-located.
//!
//! Two Track variants on [`FnValueState`]: `eager_subs` (the head
//! resolved to a `KFunction` value carrier and ≥1 eager sub is
//! pending) and `head_placeholder` (the head name resolved to a
//! forward-reference `Placeholder`). Mutually exclusive — head
//! resolution succeeds (or doesn't) before the part walk runs.

use std::marker::PhantomData;

use crate::builtins::{struct_value, tagged_union};
use crate::machine::core::kfunction::KFunction;
use crate::machine::model::Parseable;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::KObject;
use crate::machine::{KError, KErrorKind, NodeId, Resolution, Scope};

use super::super::Scheduler;
use super::super::super::nodes::{NodeOutput, NodeStep};
use super::{
    DispatchState, EagerSubsInstall, EagerSubsTrack, Initialized, extract_named_call_inner,
    stage_all_eager_parts,
};
use super::single_poll::schedule_constructor_body;

pub(in crate::machine::execute) struct FnValueState<'a> {
    pub(in crate::machine::execute) init: Initialized,
    /// Eager-subs track installed by [`FnValueState::install_eager_subs_track`].
    pub(in crate::machine::execute) eager_subs: Option<EagerSubsTrack<'a>>,
    /// Head-placeholder park track installed by the
    /// `Resolution::Placeholder` arm of [`FnValueState::initial`].
    pub(in crate::machine::execute) head_placeholder:
        Option<FnValueHeadPlaceholderTrack<'a>>,
}

/// Track state for the head-placeholder park a `FunctionValueCall`
/// slot is parked on when the head name resolved to a forward-
/// reference `Resolution::Placeholder(producer)`. Carries the
/// *original* (unspliced) call expression so the resume can re-run
/// the fast lane against it once the producer is bound.
pub(in crate::machine::execute) struct FnValueHeadPlaceholderTrack<'a> {
    pub(in crate::machine::execute) expr: KExpression<'a>,
    pub(in crate::machine::execute) producer: NodeId,
    _ph: PhantomData<&'a KFunction<'a>>,
}

impl<'a> FnValueHeadPlaceholderTrack<'a> {
    pub(in crate::machine::execute) fn new(expr: KExpression<'a>, producer: NodeId) -> Self {
        Self { expr, producer, _ph: PhantomData }
    }
}

impl<'a> FnValueState<'a> {
    pub(in crate::machine::execute) fn from_init(init: Initialized) -> Self {
        Self { init, eager_subs: None, head_placeholder: None }
    }

    pub(in crate::machine::execute) fn with_eager_subs(
        init: Initialized,
        track: EagerSubsTrack<'a>,
    ) -> Self {
        Self { init, eager_subs: Some(track), head_placeholder: None }
    }

    pub(in crate::machine::execute) fn with_head_placeholder(
        init: Initialized,
        track: FnValueHeadPlaceholderTrack<'a>,
    ) -> Self {
        Self { init, eager_subs: None, head_placeholder: Some(track) }
    }

    /// Entry from the dispatch router for the FunctionValueCall shape.
    /// Routes the `KFunction` carrier through the eager-subs Track
    /// installer and the `Resolution::Placeholder` head park through
    /// the head-placeholder Track installer — both inline into the
    /// slot's `DispatchState`.
    pub(super) fn initial(
        sched: &mut Scheduler<'a>,
        expr: KExpression<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        let head = match &expr.parts[0].value {
            ExpressionPart::Identifier(n) => n.clone(),
            _ => unreachable!("FunctionValueCall shape implies Identifier head"),
        };
        let chain = sched.active_chain.as_deref();
        match scope.resolve_with_chain(&head, chain) {
            Resolution::Value(obj) => Self::dispatch_callable_value(sched, expr, obj, scope, idx),
            Resolution::Placeholder(producer_id) => {
                Ok(Self::install_head_park(sched, producer_id, expr, idx))
            }
            Resolution::UnboundName => Ok(NodeStep::Done(NodeOutput::Err(KError::new(
                KErrorKind::UnboundName(head),
            )))),
        }
    }

    /// Resume entry. Routes by install order: `eager_subs` first, then
    /// `head_placeholder` (mutually exclusive at install time).
    pub(super) fn resume(
        self,
        sched: &mut Scheduler<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        let FnValueState { init, eager_subs, head_placeholder } = self;
        let _ = init;
        if let Some(track) = eager_subs {
            debug_assert!(
                head_placeholder.is_none(),
                "eager_subs and head_placeholder are mutually exclusive",
            );
            return sched.resume_eager_subs(track, scope, idx);
        }
        let track = head_placeholder
            .expect("FunctionValueCall resume is only entered after a track is installed");
        let FnValueHeadPlaceholderTrack { expr, producer, .. } = track;
        let _ = producer;
        Self::initial(sched, expr, scope, idx)
    }

    /// Branch on the resolved head carrier. Routes the `KFunction` arm
    /// through [`Self::install_eager_subs_track`]; Struct / Tagged
    /// construction stays on `schedule_constructor_body`.
    fn dispatch_callable_value(
        sched: &mut Scheduler<'a>,
        expr: KExpression<'a>,
        head_obj: &'a KObject<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        let inner_parts = match extract_named_call_inner(&expr) {
            Ok(parts) => parts,
            Err(e) => return Ok(NodeStep::Done(NodeOutput::Err(e))),
        };
        match head_obj {
            KObject::KFunction(f, _) => match f.reconstruct_positional(inner_parts) {
                Ok(rebuilt) => Self::install_eager_subs_track(sched, rebuilt, f, scope, idx),
                Err(e) => Ok(NodeStep::Done(NodeOutput::Err(e))),
            },
            KObject::StructType { .. } => Ok(schedule_constructor_body(
                sched,
                struct_value::apply(head_obj, inner_parts),
                idx,
            )),
            KObject::TaggedUnionType { .. } => Ok(schedule_constructor_body(
                sched,
                tagged_union::apply(head_obj, inner_parts),
                idx,
            )),
            other => Ok(NodeStep::Done(NodeOutput::Err(KError::new(
                KErrorKind::TypeMismatch {
                    arg: "verb".to_string(),
                    expected: "KFunction or Type".to_string(),
                    got: other.summarize(),
                },
            )))),
        }
    }

    /// Realize the FunctionValueCall eager-subs Track: stage every
    /// eager part as a sub-Dispatch (or aggregate), submit each sub
    /// and either splice already-terminal results inline or
    /// `add_owned_edge` and record, then transition to
    /// `FunctionValueCall(WaitingEagerSubs)`. If no subs schedule or
    /// all subs short-circuit at install time, bind `picked` directly
    /// without installing a track.
    fn install_eager_subs_track(
        sched: &mut Scheduler<'a>,
        expr: KExpression<'a>,
        picked: &'a KFunction<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        let (new_parts, staged_subs) = stage_all_eager_parts(expr.parts);
        let working_expr = KExpression::new(new_parts);
        match sched.install_eager_subs(working_expr, staged_subs, Some(picked), scope, idx) {
            EagerSubsInstall::DepError(step) => Ok(step),
            EagerSubsInstall::AllInline(working_expr) => match picked.bind(working_expr) {
                Ok(future) => Ok(sched.invoke_to_step_pinned(future, scope, idx)),
                Err(e) => Ok(NodeStep::Done(NodeOutput::Err(e))),
            },
            EagerSubsInstall::Parked(track) => {
                // FunctionValueCall is non-binder; `pre_subs` is always empty.
                let init = Initialized { pre_subs: Vec::new() };
                Ok(sched.replace_with_parked_dispatch(DispatchState::FunctionValueCall(Box::new(
                    Self::with_eager_subs(init, track),
                ))))
            }
        }
    }

    /// Realize the head-placeholder Track: install a `Notify` park
    /// edge from the producer to this slot, then transition to
    /// `FunctionValueCall(WaitingHeadPlaceholder)`.
    fn install_head_park(
        sched: &mut Scheduler<'a>,
        producer: NodeId,
        expr: KExpression<'a>,
        idx: usize,
    ) -> NodeStep<'a> {
        sched.deps.add_park_edge(producer, NodeId(idx));
        let track = FnValueHeadPlaceholderTrack::new(expr, producer);
        let init = Initialized { pre_subs: Vec::new() };
        sched.replace_with_parked_dispatch(DispatchState::FunctionValueCall(Box::new(
            Self::with_head_placeholder(init, track),
        )))
    }
}
