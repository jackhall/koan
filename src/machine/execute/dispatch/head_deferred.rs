//! Head-deferred dispatch shapes — `HeadDeferred` and `TypeHeadDeferred`.
//!
//! Both evaluate the head (`parts[0]`) first as a sub-dispatch, parking the slot
//! on it as a single-dep [`NodeWork::DispatchCombine`](super::super::nodes::NodeWork);
//! once it resolves, the finish applies the value to `parts[1..]` via the shared
//! apply-a-callable tail. The `type_only` flag selects the admitted arm set:
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

use crate::machine::core::source::Spanned;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::types::KType;
use crate::machine::model::{Carried, KObject, Parseable};
use crate::machine::{KError, KErrorKind};

use super::super::nodes::{DispatchCombineFinish, NodeOutput, NodeStep};
use super::apply_callable::{apply_callable, ResolvedCallable};
use super::outcome::{DispatchDep, DispatchOutcome};
use super::{harness, DispatchCtx};

/// `HeadDeferred` entry: head is a nested `Expression`, dispatched directly, then
/// applied to `parts[1..]` once it resolves.
pub(in crate::machine::execute) fn initial_expr<'run>(
    ctx: &mut DispatchCtx<'run, '_>,
    expr: KExpression<'run>,
    idx: usize,
) -> NodeStep<'run> {
    let head = match &expr.parts[0].value {
        ExpressionPart::Expression(boxed) => (**boxed).clone(),
        _ => unreachable!("HeadDeferred shape implies nested Expression head"),
    };
    park_on_head(ctx, expr, head, false, idx)
}

/// `TypeHeadDeferred` entry: head is a `:(...)` sigil. Wrap it as a one-part
/// `KExpression` so the type marker survives the sub-dispatch (mirrors
/// `stage_all_eager_parts`).
pub(in crate::machine::execute) fn initial_type<'run>(
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
    park_on_head(ctx, expr, head, true, idx)
}

/// Park the slot on the head sub-dispatch as a single-dep [`DispatchOutcome::Combine`]: the
/// harness submits `head` as an owned dep and parks the slot on it. When the head resolves, the
/// finish classifies it into a [`ResolvedCallable`] and hands off to the shared apply-a-callable
/// tail — which may itself resolve, park, or error, so the `NodeStep`-returning dispatch finish is
/// required (a `BodyResult` Combine finish could not re-park). A dep error short-circuits frameless
/// in `run_dispatch_combine`, so the finish only runs on a resolved head.
fn park_on_head<'run>(
    ctx: &mut DispatchCtx<'run, '_>,
    expr: KExpression<'run>,
    head: KExpression<'run>,
    type_only: bool,
    idx: usize,
) -> NodeStep<'run> {
    let finish: DispatchCombineFinish<'run> = Box::new(move |ctx, results, idx| {
        let callable = match classify_head(results[0], type_only) {
            Ok(c) => c,
            Err(e) => return NodeStep::Done(NodeOutput::Err(e)),
        };
        let outcome = apply_callable(&ctx.read_view(), callable, &expr);
        harness::apply_dispatch_outcome(ctx, outcome, idx)
    });
    let outcome = DispatchOutcome::Combine {
        deps: vec![DispatchDep::Dispatch(head)],
        // The head sub is the only dep; a dep error propagates frameless (the resumed dispatch
        // attaches its own frame), matching the resume behaviour.
        dep_error_frame: None,
        finish,
        free: Vec::new(),
    };
    harness::apply_dispatch_outcome(ctx, outcome, idx)
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
