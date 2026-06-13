//! FunctionValueCall dispatch shape.
//!
//! Head resolution runs before any part walk: a value-bound head dispatches the call
//! immediately, an unbound name errors, and a still-finalizing head placeholder parks as a
//! [`FnValueState`] that re-runs the fast lane on resume.

use std::marker::PhantomData;

use crate::machine::core::kfunction::KFunction;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::KObject;
use crate::machine::model::Parseable;
use crate::machine::{KError, KErrorKind, NodeId, Resolution};

use super::super::nodes::NodeOutput;
use super::apply_callable::{apply_callable, ResolvedCallable};
use super::ctx::DispatchCx;
use super::outcome::DispatchOutcome;
use super::{DispatchState, Initialized};

/// Parked `FunctionValueCall` state. Eager subs park as a [`NodeWork::DispatchCombine`]
/// (`apply_callable::install_eager_subs_track`), so the only thing a `FnValueState` carries is a
/// still-finalizing *head* binding — a lowercase-identifier head that resolved to a placeholder.
pub(in crate::machine::execute) struct FnValueState<'run> {
    pub(in crate::machine::execute) init: Initialized,
    pub(in crate::machine::execute) head_placeholder: FnValueHeadPlaceholderTrack<'run>,
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
    pub(in crate::machine::execute) fn with_head_placeholder(
        init: Initialized,
        track: FnValueHeadPlaceholderTrack<'run>,
    ) -> Self {
        Self {
            init,
            head_placeholder: track,
        }
    }

    pub(super) fn initial(
        ctx: &DispatchCx<'run, '_>,
        expr: KExpression<'run>,
    ) -> DispatchOutcome<'run> {
        let head = match &expr.parts[0].value {
            ExpressionPart::Identifier(n) => n.clone(),
            _ => unreachable!("FunctionValueCall shape implies Identifier head"),
        };
        let chain = ctx.chain_deref();
        match ctx.current_scope().resolve_with_chain(&head, chain) {
            Resolution::Value(obj) => Self::dispatch_callable_value(ctx, expr, obj),
            Resolution::Placeholder(producer_id) => Self::install_head_park(producer_id, expr),
            Resolution::UnboundName => {
                DispatchOutcome::Terminal(NodeOutput::Err(KError::new(KErrorKind::UnboundName(head))))
            }
        }
    }

    pub(super) fn resume(self, ctx: &DispatchCx<'run, '_>) -> DispatchOutcome<'run> {
        let FnValueState {
            init,
            head_placeholder,
        } = self;
        let _ = init;
        let FnValueHeadPlaceholderTrack { expr, producer, .. } = head_placeholder;
        let _ = producer;
        Self::initial(ctx, expr)
    }

    /// Resolve the already-bound head value to a [`ResolvedCallable`] and hand
    /// off to the shared apply-a-callable tail. The head is a value-bound
    /// lowercase identifier, so only a `KFunction` (functor or not) is callable —
    /// the partition invariant keeps a type out of `bindings.data`, so a
    /// constructor-typed head reaches dispatch through the type channel
    /// (`HeadDeferred`), never here. Anything else is a non-callable `TypeMismatch`.
    fn dispatch_callable_value(
        ctx: &DispatchCx<'run, '_>,
        expr: KExpression<'run>,
        head_obj: &'run KObject<'run>,
    ) -> DispatchOutcome<'run> {
        let callable = match head_obj {
            KObject::KFunction(f, _) => ResolvedCallable::Function(f),
            other => {
                return DispatchOutcome::Terminal(NodeOutput::Err(KError::new(
                    KErrorKind::TypeMismatch {
                        arg: "verb".to_string(),
                        expected: "KFunction or Type".to_string(),
                        got: other.summarize(),
                    },
                )))
            }
        };
        apply_callable(ctx, callable, &expr)
    }

    fn install_head_park(producer: NodeId, expr: KExpression<'run>) -> DispatchOutcome<'run> {
        let track = FnValueHeadPlaceholderTrack::new(expr, producer);
        let init = Initialized {
            pre_subs: Vec::new(),
        };
        DispatchOutcome::ParkSelf {
            producers: vec![producer],
            state: DispatchState::FunctionValueCall(Box::new(Self::with_head_placeholder(
                init, track,
            ))),
        }
    }
}
