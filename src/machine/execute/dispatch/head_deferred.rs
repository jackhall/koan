//! Head-deferred dispatch shapes — `HeadDeferred` and `TypeHeadDeferred`.
//!
//! Both evaluate the head (`parts[0]`) first as a sub-dispatch, parking the slot
//! on it as a single-dep [`NodeWork`](super::super::nodes::NodeWork);
//! once it resolves, the finish applies the value to `parts[1..]` via the shared
//! apply-a-callable tail. The `type_only` flag selects the admitted arm set:
//!
//! - `HeadDeferred` (head is a nested `Expression`, `type_only = false`): the
//!   resumed value may be a `KFunction` (functor or not — the `Function` arm), a
//!   bound functor reached through the type table (`Carried::Type(KFunctor { body:
//!   Some })` — also the `Function` arm), or a `Carried::Type(SetRef)` (the
//!   `Constructor` arm); any other value is a non-callable `DispatchFailed`.
//! - `TypeHeadDeferred` (head is a `:(...)` sigil, `type_only = true`): the
//!   resumed value is admitted only when it is type-shaped — a constructible type
//!   (`Constructor`), a functor value (`KFunction` with `is_functor`), or a
//!   type-bound functor (`Carried::Type(KFunctor { body: Some })`), all via the
//!   `Function` arm. A bare functor *annotation* (`KFunctor { body: None }`) is
//!   type-shaped but not invocable and a plain function or non-type value surface
//!   a type-shaped `TypeMismatch`.
//!
//! The head sub-dispatch is an Owned edge; the park/resume pair mirrors
//! `park_on_literal_producer` + the `type_call` head-placeholder resume, no new
//! scheduler primitive.

use crate::machine::core::kfunction::action::DepPlacement;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::types::KType;
use crate::machine::model::{Carried, KObject, Parseable};
use crate::machine::{FrameSet, KError, KErrorKind};
use crate::source::Spanned;

use super::super::TerminalDepFinish;
use super::apply_callable::{apply_callable, ResolvedCallable};
use super::{Await, DepRequest, Outcome};
use crate::scheduler::Deps;

/// `HeadDeferred` entry: head is a nested `Expression`, dispatched directly, then
/// applied to `parts[1..]` once it resolves.
pub(in crate::machine::execute) fn initial_expr<'step>(expr: KExpression<'step>) -> Outcome<'step> {
    let head = match &expr.parts[0].value {
        ExpressionPart::Expression(boxed) => (**boxed).clone(),
        _ => unreachable!("HeadDeferred shape implies nested Expression head"),
    };
    park_on_head(expr, head, false)
}

/// `TypeHeadDeferred` entry: head is a `:(...)` sigil. Wrap it as a one-part
/// `KExpression` so the type marker survives the sub-dispatch (mirrors
/// `stage_all_eager_parts`).
pub(in crate::machine::execute) fn initial_type<'step>(expr: KExpression<'step>) -> Outcome<'step> {
    let head = match &expr.parts[0].value {
        ExpressionPart::SigiledTypeExpr(boxed) => KExpression::new(vec![Spanned::bare(
            ExpressionPart::SigiledTypeExpr(boxed.clone()),
        )]),
        _ => unreachable!("TypeHeadDeferred shape implies SigiledTypeExpr head"),
    };
    park_on_head(expr, head, true)
}

/// Park the slot on the head sub-dispatch as a single-dep [`Outcome::ParkThenContinue`]: the
/// harness submits `head` as an owned dep and parks the slot on it. When the head resolves, the
/// finish classifies it into a [`ResolvedCallable`] and hands off to the shared apply-a-callable
/// tail — which may itself resolve, park, or error, so the re-park-capable dispatch finish (its
/// `Outcome` may be a fresh `ParkThenContinue`/`Redispatch`) is required. A dep error short-circuits
/// frameless in `run_step`, so the finish only runs on a resolved head.
fn park_on_head<'step>(
    expr: KExpression<'step>,
    head: KExpression<'step>,
    type_only: bool,
) -> Outcome<'step> {
    let finish: TerminalDepFinish<'step> = Box::new(move |ctx, terminals| {
        // The head sub is the sole owned dep. Its reach — the regions its computed identity/callable
        // points into — is named on its delivered carrier's witness. A `SetRef` constructor identity
        // threads it to the construction finish (the operand names the identity's own region); a
        // callable ignores it and rides the bind fold below instead.
        let head_terminal = terminals.owned(0);
        let reach = head_terminal.carrier.witness().clone();
        // The resolved callable survives across steps (the apply tail may itself re-park), and owned
        // deps cascade-free on resolve, so relocate the head value into the consumer region. The
        // copy-free carrier-carrying form is the `carrier-carrying-spliced-parts` roadmap item.
        let head = head_terminal.relocate(ctx.current_scope().brand());
        let callable = match classify_head(head, type_only, reach) {
            Ok(c) => c,
            Err(e) => return Outcome::Done(Err(e)),
        };
        // The head resolved to a computed callable (a functor / closure) whose captured region is
        // foreign to this scope and held only on the producer's now-resolving node. Fold its carrier
        // reach into the consumer scope so the captured environment outlives the application: the head
        // value is applied (not embedded in a witnessed result), so its reach rides the bind fold here,
        // read straight off the delivered carrier.
        ctx.current_scope()
            .fold_reach(head_terminal.carrier.witness());
        apply_callable(ctx, callable, &expr)
    });
    // The head sub is the only dep; a dep error propagates frameless (the resumed dispatch
    // attaches its own frame), matching the resume behaviour.
    Await::on(Deps::from_owned([DepRequest::Dispatch {
        expr: head,
        placement: DepPlacement::OwnScope,
    }]))
    .finish_terminal(finish)
}

/// Branch a resumed head value into a [`ResolvedCallable`], honoring the
/// `type_only` arm pruning. Returns the shape-appropriate `KError` for a
/// non-admitted value (a type-shaped `TypeMismatch` under `type_only`, else a
/// non-callable `DispatchFailed`).
fn classify_head<'step>(
    head: Carried<'step>,
    type_only: bool,
    reach: FrameSet,
) -> Result<ResolvedCallable<'step>, KError> {
    match head {
        // A runtime value head. A functor (`KFunction` with `is_functor`) is admitted in
        // both modes — its result is a module, so it is the type-shaped head's only
        // function arm. A plain function is admitted only in the non-type mode; under
        // `TypeHeadDeferred` it is the pruned arm and falls through to the `TypeMismatch`.
        Carried::Object(obj) => match obj {
            KObject::KFunction(f) if f.is_functor => Ok(ResolvedCallable::Function(f)),
            KObject::KFunction(f) if !type_only => Ok(ResolvedCallable::Function(f)),
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
            KType::SetRef { .. } => Ok(ResolvedCallable::Constructor {
                identity: kt,
                reach,
            }),
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
