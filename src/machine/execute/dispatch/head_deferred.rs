//! Head-deferred dispatch shapes — `HeadDeferred` and `TypeHeadDeferred`.
//!
//! Both evaluate the head (`parts[0]`) first as an owned sub-dispatch and park the
//! slot on it; once it resolves, the finish classifies the value and applies it to
//! `parts[1..]` via the shared apply-a-callable tail. The `type_only` flag selects
//! the admitted arm set (see [`classify_head`]):
//!
//! - `HeadDeferred` (`type_only = false`): admits any `KFunction`, a type-bound
//!   functor, or a `SetRef` constructor.
//! - `TypeHeadDeferred` (head is a `:(...)` sigil, `type_only = true`): admits only
//!   type-shaped heads — a `SetRef` constructor, a functor, or a type-bound functor.
//!   A plain function or a bare functor annotation surfaces a type-shaped
//!   `TypeMismatch`.
//!
//! The park/resume pair mirrors `park_on_literal` + the `type_call`
//! head-placeholder resume, no new scheduler primitive.

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

/// Park the slot on the head sub-dispatch. When the head resolves, the finish classifies it into a
/// [`ResolvedCallable`] and hands off to the shared apply-a-callable tail; that tail may itself
/// re-park, so the finish must be re-park-capable. A dep error short-circuits frameless in
/// `run_step`, so the finish only runs on a resolved head.
fn park_on_head<'step>(
    expr: KExpression<'step>,
    head: KExpression<'step>,
    type_only: bool,
) -> Outcome<'step> {
    let finish: TerminalDepFinish<'step> = Box::new(move |ctx, terminals| {
        // `reach` names the regions the resolved identity points into: a `SetRef` constructor threads
        // it to the construction finish (the operand names the identity's own region); a callable
        // ignores it and rides the `adopt_sealed` below. Collapsed to a plain reach set — the
        // identity is region-resident, so its witness carries only frame reach, no owned backing.
        let head_terminal = terminals.owned(0);
        let reach = head_terminal.delivered.liveness_frameset();
        // Adopt the head's carrier copy-free: fold its reach so a callable's captured foreign
        // environment outlives the application, and re-anchor the value at the consumer scope brand.
        let head = ctx.current_scope().adopt_sealed(&head_terminal.delivered);
        let callable = match classify_head(head, type_only, reach) {
            Ok(c) => c,
            Err(e) => return Outcome::Done(Err(e)),
        };
        apply_callable(ctx, callable, &expr)
    });
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
        // A functor's result is a module, so it is admitted in both modes; a plain function is the
        // pruned arm under `type_only` and falls through to the `TypeMismatch`.
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
        // A type-bound functor (`body: Some`) yields a module, so it is admitted in both modes; a
        // bare functor annotation (`body: None`) is type-shaped but not invocable; a `SetRef` is a
        // constructor.
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
