//! FunctionValueCall dispatch shape.
//!
//! The two tracks on [`FnValueState`] are mutually exclusive: head
//! resolution decides between them before any part walk runs.

use std::marker::PhantomData;

use crate::machine::core::kfunction::KFunction;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::types::{KType, UserTypeKind};
use crate::machine::model::KObject;
use crate::machine::model::Parseable;
use crate::machine::{KError, KErrorKind, NodeId, Resolution, Scope};

use super::super::nodes::{NodeOutput, NodeStep};
use super::constructors;
use super::{
    body_shape_err, extract_call_body, stage_all_eager_parts, CallBody, DispatchCtx, DispatchState,
    EagerSubsInstall, EagerSubsTrack, Initialized, NAMED_ONLY, POSITIONAL_ONLY,
};

pub(in crate::machine::execute) struct FnValueState<'a> {
    pub(in crate::machine::execute) init: Initialized,
    pub(in crate::machine::execute) eager_subs: Option<EagerSubsTrack<'a>>,
    pub(in crate::machine::execute) head_placeholder: Option<FnValueHeadPlaceholderTrack<'a>>,
}

/// Carries the *original* (unspliced) call expression so the resume
/// can re-run the fast lane against it once the producer is bound.
pub(in crate::machine::execute) struct FnValueHeadPlaceholderTrack<'a> {
    pub(in crate::machine::execute) expr: KExpression<'a>,
    pub(in crate::machine::execute) producer: NodeId,
    _ph: PhantomData<&'a KFunction<'a>>,
}

impl<'a> FnValueHeadPlaceholderTrack<'a> {
    pub(in crate::machine::execute) fn new(expr: KExpression<'a>, producer: NodeId) -> Self {
        Self {
            expr,
            producer,
            _ph: PhantomData,
        }
    }
}

impl<'a> FnValueState<'a> {
    pub(in crate::machine::execute) fn from_init(init: Initialized) -> Self {
        Self {
            init,
            eager_subs: None,
            head_placeholder: None,
        }
    }

    pub(in crate::machine::execute) fn with_eager_subs(
        init: Initialized,
        track: EagerSubsTrack<'a>,
    ) -> Self {
        Self {
            init,
            eager_subs: Some(track),
            head_placeholder: None,
        }
    }

    pub(in crate::machine::execute) fn with_head_placeholder(
        init: Initialized,
        track: FnValueHeadPlaceholderTrack<'a>,
    ) -> Self {
        Self {
            init,
            eager_subs: None,
            head_placeholder: Some(track),
        }
    }

    pub(super) fn initial(
        ctx: &mut DispatchCtx<'a, '_>,
        expr: KExpression<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        let head = match &expr.parts[0].value {
            ExpressionPart::Identifier(n) => n.clone(),
            _ => unreachable!("FunctionValueCall shape implies Identifier head"),
        };
        let chain = ctx.chain_deref();
        match scope.resolve_with_chain(&head, chain) {
            Resolution::Value(obj) => Self::dispatch_callable_value(ctx, expr, obj, scope, idx),
            Resolution::Placeholder(producer_id) => {
                Ok(Self::install_head_park(ctx, producer_id, expr, idx))
            }
            Resolution::UnboundName => Ok(NodeStep::Done(NodeOutput::Err(KError::new(
                KErrorKind::UnboundName(head),
            )))),
        }
    }

    pub(super) fn resume(
        self,
        ctx: &mut DispatchCtx<'a, '_>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
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
            return ctx.resume_eager_subs(track, scope, idx);
        }
        let track = head_placeholder
            .expect("FunctionValueCall resume is only entered after a track is installed");
        let FnValueHeadPlaceholderTrack { expr, producer, .. } = track;
        let _ = producer;
        Self::initial(ctx, expr, scope, idx)
    }

    fn dispatch_callable_value(
        ctx: &mut DispatchCtx<'a, '_>,
        expr: KExpression<'a>,
        head_obj: &'a KObject<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        let body = match extract_call_body(&expr) {
            Ok(b) => b,
            Err(e) => return Ok(NodeStep::Done(NodeOutput::Err(e))),
        };
        match head_obj {
            KObject::KFunction(f, _) => match body {
                CallBody::Named(fields) => match f.reconstruct_positional(fields) {
                    Ok(rebuilt) => Self::install_eager_subs_track(ctx, rebuilt, f, scope, idx),
                    Err(e) => Ok(NodeStep::Done(NodeOutput::Err(e))),
                },
                CallBody::Positional(_) => Ok(body_shape_err(&expr, NAMED_ONLY)),
            },
            // A value-classified alias of a constructible type — `LET outcome = Outcome`
            // then `(outcome (err "x"))`. The alias carries the type's identity directly
            // (`KTypeValue(UserType { .. })`); construction reads the schema off that
            // identity, the same payload `bindings.types[name]` holds.
            KObject::KTypeValue(KType::UserType {
                kind,
                scope_id,
                name,
            }) => match kind {
                UserTypeKind::Struct { fields } => match body {
                    CallBody::Named(record_fields) => {
                        Ok(constructors::dispatch_construct_struct(
                            ctx,
                            name.clone(),
                            *scope_id,
                            std::rc::Rc::clone(fields),
                            record_fields,
                            scope,
                            idx,
                        ))
                    }
                    CallBody::Positional(_) => Ok(body_shape_err(&expr, NAMED_ONLY)),
                },
                UserTypeKind::Tagged { schema } | UserTypeKind::TypeConstructor { schema, .. } => {
                    match body {
                        CallBody::Positional(parts) => {
                            Ok(constructors::dispatch_construct_tagged(
                                ctx,
                                name.clone(),
                                *scope_id,
                                std::rc::Rc::clone(schema),
                                parts,
                                scope,
                                idx,
                            ))
                        }
                        CallBody::Named(_) => Ok(body_shape_err(&expr, POSITIONAL_ONLY)),
                    }
                }
                UserTypeKind::Newtype { .. } => match body {
                    CallBody::Positional(parts) => {
                        let identity_ref: &'a KType<'a> = scope.arena.alloc(KType::UserType {
                            kind: kind.clone(),
                            scope_id: *scope_id,
                            name: name.clone(),
                        });
                        let body = crate::builtins::newtype_def::newtype_construct(
                            scope,
                            ctx,
                            identity_ref,
                            parts,
                        );
                        Ok(super::single_poll::schedule_constructor_body(
                            ctx, body, idx,
                        ))
                    }
                    CallBody::Named(_) => Ok(body_shape_err(&expr, POSITIONAL_ONLY)),
                },
            },
            other => Ok(NodeStep::Done(NodeOutput::Err(KError::new(
                KErrorKind::TypeMismatch {
                    arg: "verb".to_string(),
                    expected: "KFunction or Type".to_string(),
                    got: other.summarize(),
                },
            )))),
        }
    }

    /// When no subs schedule or all short-circuit at install time,
    /// `picked` is bound directly without installing a track.
    fn install_eager_subs_track(
        ctx: &mut DispatchCtx<'a, '_>,
        expr: KExpression<'a>,
        picked: &'a KFunction<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        let (new_parts, staged_subs) = stage_all_eager_parts(expr.parts);
        let working_expr = KExpression::new(new_parts);
        match ctx.install_eager_subs(working_expr, staged_subs, Some(picked), scope, idx) {
            EagerSubsInstall::DepError(step) => Ok(step),
            EagerSubsInstall::AllInline(working_expr) => match picked.bind(working_expr) {
                Ok(future) => Ok(ctx.invoke_to_step_pinned(future, scope, idx)),
                Err(e) => Ok(NodeStep::Done(NodeOutput::Err(e))),
            },
            EagerSubsInstall::Parked(track) => {
                // FunctionValueCall is non-binder; `pre_subs` is always empty.
                let init = Initialized {
                    pre_subs: Vec::new(),
                };
                Ok(
                    ctx.replace_with_parked_dispatch(DispatchState::FunctionValueCall(Box::new(
                        Self::with_eager_subs(init, track),
                    ))),
                )
            }
        }
    }

    fn install_head_park(
        ctx: &mut DispatchCtx<'a, '_>,
        producer: NodeId,
        expr: KExpression<'a>,
        idx: usize,
    ) -> NodeStep<'a> {
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
