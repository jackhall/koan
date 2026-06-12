//! FunctionValueCall dispatch shape.
//!
//! The two tracks on [`FnValueState`] are mutually exclusive: head
//! resolution decides between them before any part walk runs.

use std::marker::PhantomData;

use crate::machine::core::kfunction::{KFunction, SchedulerHandle};
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::KObject;
use crate::machine::model::Parseable;
use crate::machine::{KError, KErrorKind, NodeId, Resolution};

use super::super::nodes::{NodeOutput, NodeStep};
use super::apply_callable::{apply_callable, ResolvedCallable};
use super::{DispatchCtx, DispatchState, EagerSubsTrack, Initialized};

pub(in crate::machine::execute) struct FnValueState<'run> {
    pub(in crate::machine::execute) init: Initialized,
    pub(in crate::machine::execute) eager_subs: Option<EagerSubsTrack<'run>>,
    pub(in crate::machine::execute) head_placeholder: Option<FnValueHeadPlaceholderTrack<'run>>,
}

/// Carries the *original* (unspliced) call expression so the resume
/// can re-run the fast lane against it once the producer is bound.
pub(in crate::machine::execute) struct FnValueHeadPlaceholderTrack<'run> {
    pub(in crate::machine::execute) expr: KExpression<'run>,
    pub(in crate::machine::execute) producer: NodeId,
    _ph: PhantomData<&'run KFunction<'run>>,
}

impl<'run> FnValueHeadPlaceholderTrack<'run> {
    pub(in crate::machine::execute) fn new(expr: KExpression<'run>, producer: NodeId) -> Self {
        Self {
            expr,
            producer,
            _ph: PhantomData,
        }
    }
}

impl<'run> FnValueState<'run> {
    // Under `dispatch-combine` the apply-a-callable path parks committed-pick eager
    // subs as a `DispatchCombine`, never a `FnValueState` eager-subs track, so this
    // constructor (and the `eager_subs` resume branch) is reachable only on the legacy path.
    #[cfg(not(feature = "dispatch-combine"))]
    pub(in crate::machine::execute) fn with_eager_subs(
        init: Initialized,
        track: EagerSubsTrack<'run>,
    ) -> Self {
        Self {
            init,
            eager_subs: Some(track),
            head_placeholder: None,
        }
    }

    pub(in crate::machine::execute) fn with_head_placeholder(
        init: Initialized,
        track: FnValueHeadPlaceholderTrack<'run>,
    ) -> Self {
        Self {
            init,
            eager_subs: None,
            head_placeholder: Some(track),
        }
    }

    pub(super) fn initial(
        ctx: &mut DispatchCtx<'run, '_>,
        expr: KExpression<'run>,
        idx: usize,
    ) -> NodeStep<'run> {
        let head = match &expr.parts[0].value {
            ExpressionPart::Identifier(n) => n.clone(),
            _ => unreachable!("FunctionValueCall shape implies Identifier head"),
        };
        let chain = ctx.chain_deref();
        match ctx.current_scope().resolve_with_chain(&head, chain) {
            Resolution::Value(obj) => Self::dispatch_callable_value(ctx, expr, obj, idx),
            Resolution::Placeholder(producer_id) => {
                Self::install_head_park(ctx, producer_id, expr, idx)
            }
            Resolution::UnboundName => NodeStep::Done(NodeOutput::Err(KError::new(
                KErrorKind::UnboundName(head),
            ))),
        }
    }

    pub(super) fn resume(
        self,
        ctx: &mut DispatchCtx<'run, '_>,
        idx: usize,
    ) -> NodeStep<'run> {
        let FnValueState {
            init,
            eager_subs,
            head_placeholder,
        } = self;
        let _ = init;
        if let Some(track) = eager_subs {
            debug_assert!(
                head_placeholder.is_none(),
                "eager_subs and head_placeholder are mutually exclusive",
            );
            return ctx.resume_eager_subs(track, idx);
        }
        let track = head_placeholder
            .expect("FunctionValueCall resume is only entered after a track is installed");
        let FnValueHeadPlaceholderTrack { expr, producer, .. } = track;
        let _ = producer;
        Self::initial(ctx, expr, idx)
    }

    /// Resolve the already-bound head value to a [`ResolvedCallable`] and hand
    /// off to the shared apply-a-callable tail. The head is a value-bound
    /// lowercase identifier, so only a `KFunction` (functor or not) is callable —
    /// the partition invariant keeps a type out of `bindings.data`, so a
    /// constructor-typed head reaches dispatch through the type channel
    /// (`HeadDeferred`), never here. Anything else is a non-callable `TypeMismatch`.
    fn dispatch_callable_value(
        ctx: &mut DispatchCtx<'run, '_>,
        expr: KExpression<'run>,
        head_obj: &'run KObject<'run>,
        idx: usize,
    ) -> NodeStep<'run> {
        let callable = match head_obj {
            KObject::KFunction(f, _) => ResolvedCallable::Function(f),
            other => {
                return NodeStep::Done(NodeOutput::Err(KError::new(
                    KErrorKind::TypeMismatch {
                        arg: "verb".to_string(),
                        expected: "KFunction or Type".to_string(),
                        got: other.summarize(),
                    },
                )))
            }
        };
        apply_callable(ctx, callable, &expr, idx)
    }

    fn install_head_park(
        ctx: &mut DispatchCtx<'run, '_>,
        producer: NodeId,
        expr: KExpression<'run>,
        idx: usize,
    ) -> NodeStep<'run> {
        ctx.add_park_edge(producer, NodeId(idx));
        let track = FnValueHeadPlaceholderTrack::new(expr, producer);
        let init = Initialized {
            pre_subs: Vec::new(),
        };
        ctx.replace_with_parked_dispatch(DispatchState::FunctionValueCall(Box::new(
            Self::with_head_placeholder(init, track),
        )))
    }
}
