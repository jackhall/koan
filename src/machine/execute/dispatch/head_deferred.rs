//! Head-deferred dispatch shapes — `HeadDeferred` and `TypeHeadDeferred`.
//!
//! Both evaluate the head (`parts[0]`) first, then apply the resulting value to
//! `parts[1..]` via the shared apply-a-callable tail. They share one boxed
//! [`HeadDeferredState`]; the `type_only` flag selects the admitted arm set on
//! resume:
//!
//! - `HeadDeferred` (head is a nested `Expression`, `type_only = false`): the
//!   resumed value may be a `KFunction` (functor or not — the `Function` arm), a
//!   bound functor reached through the type table (`KTypeValue(KFunctor { body:
//!   Some })` — also the `Function` arm), or a `KTypeValue(SetRef)` (the
//!   `Constructor` arm); any other value is a non-callable `DispatchFailed`.
//! - `TypeHeadDeferred` (head is a `:(...)` sigil, `type_only = true`): the
//!   resumed value is admitted only when it is type-shaped — a constructible type
//!   (`Constructor`), a functor value (`KFunction` with `is_functor`), or a
//!   type-bound functor (`KTypeValue(KFunctor { body: Some })`), all via the
//!   `Function` arm. A bare functor *annotation* (`KFunctor { body: None }`) is
//!   type-shaped but not invocable and a plain function or non-type value surface
//!   a type-shaped `TypeMismatch`.
//!
//! The head sub-dispatch is an Owned edge; the park/resume pair mirrors
//! `park_on_literal_producer` + `CtorState::resume`, no new scheduler primitive.

use crate::machine::core::kfunction::SchedulerHandle;
use crate::machine::core::source::Spanned;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::types::KType;
use crate::machine::model::{Carried, KObject, Parseable};
use crate::machine::{KError, KErrorKind, NodeId};

use super::super::nodes::{NodeOutput, NodeStep};
use super::apply_callable::{apply_callable, ResolvedCallable};
use super::{DispatchCtx, DispatchState};

/// Parked state for a head-deferred call. `resume` re-reads `parts[1..]` (the
/// call body) and branches on the head sub-dispatch's resolved value.
pub(in crate::machine::execute) struct HeadDeferredState<'run> {
    /// The full call expression; `parts[1..]` is the body the tail consumes.
    expr: KExpression<'run>,
    /// The parked head sub-dispatch producer (an Owned edge).
    head_sub: NodeId,
    /// `TypeHeadDeferred` prunes the plain-`Function` (non-functor) arm.
    type_only: bool,
}

impl<'run> HeadDeferredState<'run> {
    /// `HeadDeferred` entry: head is a nested `Expression`, dispatched directly.
    pub(in crate::machine::execute) fn initial_expr(
        ctx: &mut DispatchCtx<'run, '_>,
        expr: KExpression<'run>,
        idx: usize,
    ) -> NodeStep<'run> {
        let head = match &expr.parts[0].value {
            ExpressionPart::Expression(boxed) => (**boxed).clone(),
            _ => unreachable!("HeadDeferred shape implies nested Expression head"),
        };
        let head_sub = ctx.add_dispatch_here(head);
        Self::park_or_resume(ctx, expr, head_sub, false, idx)
    }

    /// `TypeHeadDeferred` entry: head is a `:(...)` sigil. Wrap it as a one-part
    /// `KExpression` so the type marker survives the sub-dispatch (mirrors
    /// `stage_all_eager_parts`).
    pub(in crate::machine::execute) fn initial_type(
        ctx: &mut DispatchCtx<'run, '_>,
        expr: KExpression<'run>,
        idx: usize,
    ) -> NodeStep<'run> {
        let head = match &expr.parts[0].value {
            ExpressionPart::SigiledTypeExpr(boxed) => KExpression::new(vec![Spanned::bare(
                ExpressionPart::SigiledTypeExpr(boxed.clone()),
            )]),
            _ => unreachable!("TypeHeadDeferred shape implies SigiledTypeExpr head"),
        };
        let head_sub = ctx.add_dispatch_here(head);
        Self::park_or_resume(ctx, expr, head_sub, true, idx)
    }

    /// Read the head sub inline if it is already ready, else install the Owned
    /// edge and park as a `HeadDeferred` state.
    fn park_or_resume(
        ctx: &mut DispatchCtx<'run, '_>,
        expr: KExpression<'run>,
        head_sub: NodeId,
        type_only: bool,
        idx: usize,
    ) -> NodeStep<'run> {
        // `head_sub` was dispatched this step, and submission is enqueue-then-drain, so it
        // is never terminal yet — the inline-resume branch is dead for a fresh head sub.
        // Locking this proves the site is a hand-rolled `Combine` (one owned dep + a resume
        // finish), collapsible onto `combine_here`. Mirrors `install_eager_subs`.
        debug_assert!(
            !ctx.is_result_ready(head_sub),
            "freshly-submitted head sub {head_sub:?} is immediately ready — \
             a fresh head sub is always a parked dep"
        );
        if ctx.is_result_ready(head_sub) {
            return Self {
                expr,
                head_sub,
                type_only,
            }
            .resume(ctx, idx);
        }
        ctx.add_owned_edge(head_sub, NodeId(idx));
        let state = HeadDeferredState {
            expr,
            head_sub,
            type_only,
        };
        ctx.replace_with_parked_dispatch(DispatchState::HeadDeferred(Box::new(state)))
    }

    /// Read the resumed head value, free the head sub, and branch into the shared
    /// apply-a-callable tail. A dep error propagates; a non-admitted value
    /// surfaces a shape-appropriate diagnostic.
    pub(in crate::machine::execute) fn resume(
        self,
        ctx: &mut DispatchCtx<'run, '_>,
        idx: usize,
    ) -> NodeStep<'run> {
        let HeadDeferredState {
            expr,
            head_sub,
            type_only,
        } = self;
        let head = match ctx.read_result(head_sub) {
            Ok(v) => v,
            Err(e) => {
                let err = e.clone_for_propagation();
                ctx.clear_dep_edges(idx);
                ctx.free(head_sub.index());
                return NodeStep::Done(NodeOutput::Err(err));
            }
        };
        ctx.clear_dep_edges(idx);
        ctx.free(head_sub.index());
        let callable = match classify_head(head, type_only) {
            Ok(c) => c,
            Err(e) => return NodeStep::Done(NodeOutput::Err(e)),
        };
        apply_callable(ctx, callable, &expr, idx)
    }
}

/// Branch a resumed head value into a [`ResolvedCallable`], honoring the
/// `type_only` arm pruning. Returns the shape-appropriate `KError` for a
/// non-admitted value (a type-shaped `TypeMismatch` under `type_only`, else a
/// non-callable `DispatchFailed`).
fn classify_head<'run>(
    head: Carried<'run>,
    type_only: bool,
) -> Result<ResolvedCallable<'run>, KError> {
    match head {
        // A runtime value head. A functor (`KFunction` with `is_functor`) is admitted in
        // both modes — its result is a module, so it is the type-shaped head's only
        // function arm. A plain function is admitted only in the non-type mode; under
        // `TypeHeadDeferred` it is the pruned arm and falls through to the `TypeMismatch`.
        Carried::Object(obj) => match obj {
            KObject::KFunction(f, _) if f.is_functor => Ok(ResolvedCallable::Function(f)),
            KObject::KFunction(f, _) if !type_only => Ok(ResolvedCallable::Function(f)),
            other if type_only => Err(KError::new(KErrorKind::TypeMismatch {
                arg: "verb".to_string(),
                expected: "Type".to_string(),
                got: other.summarize(),
            })),
            other => Err(KError::new(KErrorKind::DispatchFailed {
                expr: other.summarize(),
                reason: "head evaluates to a non-callable value".to_string(),
            })),
        },
        // A type-channel head. A bound functor carries its callable on
        // `KType::KFunctor { body: Some(f) }`; calling it yields a module, so it is the
        // `Function` arm in both modes. A bare `:(FUNCTOR …)` annotation (`body: None`) is
        // type-shaped but not invocable; a `SetRef` is a constructor. Anything else is a
        // type-shaped non-callable.
        Carried::Type(kt) => match kt {
            KType::KFunctor { body: Some(f), .. } => Ok(ResolvedCallable::Function(f)),
            KType::KFunctor { body: None, .. } => Err(KError::new(KErrorKind::TypeMismatch {
                arg: "verb".to_string(),
                expected: "constructible Type or bound functor".to_string(),
                got: kt.name(),
            })),
            KType::SetRef { .. } => Ok(ResolvedCallable::Constructor(kt)),
            other if type_only => Err(KError::new(KErrorKind::TypeMismatch {
                arg: "verb".to_string(),
                expected: "Type".to_string(),
                got: other.name(),
            })),
            other => Err(KError::new(KErrorKind::DispatchFailed {
                expr: other.name(),
                reason: "head evaluates to a non-callable value".to_string(),
            })),
        },
    }
}
