//! Head-deferred dispatch shapes — `HeadDeferred` and `TypeHeadDeferred`.
//!
//! Both evaluate the head (`parts[0]`) first as an owned sub-dispatch and park the
//! slot on it; once it resolves, the finish classifies the value and applies it to
//! `parts[1..]` via the shared apply-a-callable tail. The `type_only` flag selects
//! the admitted arm set (see [`classify_head`]):
//!
//! - `HeadDeferred` (`type_only = false`): admits any `KFunction` value or a `SetMember`
//!   constructor.
//! - `TypeHeadDeferred` (head is a `:(...)` sigil, `type_only = true`): admits only a
//!   constructible type — a `SetMember` constructor. A function value or any other type
//!   surfaces a type-shaped `TypeMismatch`.
//!
//! The park/resume pair mirrors `park_on_literal` + the `type_call`
//! head-placeholder resume, no new scheduler primitive.

use crate::machine::core::DepPlacement;
use crate::machine::model::TypeNode;
use crate::machine::model::TypeRegistry;
use crate::machine::model::{Carried, KObject};
use crate::machine::model::{ExpressionPart, KExpression};
use crate::machine::{KError, KErrorKind};
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
        let head_terminal = terminals.owned(0);
        // Adopt the head's carrier copy-free: fold its reach so a callable's captured foreign
        // environment outlives the application, and re-anchor the value at the consumer scope brand.
        let head = ctx.current_scope().adopt_sealed(&head_terminal.delivered);
        let callable = match classify_head(head, type_only, ctx.types()) {
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
    types: &TypeRegistry,
) -> Result<ResolvedCallable<'step>, KError> {
    match head {
        // A function value is the pruned arm under `type_only` — the type-only lane admits no
        // value-channel callable — and falls through to the `TypeMismatch`.
        Carried::Object(obj) => match obj {
            KObject::KFunction(f) if !type_only => Ok(ResolvedCallable::Function(f)),
            other if type_only => Err(KError::new(KErrorKind::TypeMismatch {
                arg: "verb".to_string(),
                expected: "Type".to_string(),
                got: other.summarize(types),
            })),
            other => Err(KError::new(KErrorKind::DispatchFailed {
                expr: other.summarize(types),
                reason: "head evaluates to a non-callable value".to_string(),
            })),
        },
        // A head is resolved before it is classified, so an unlowered name names no callable.
        Carried::UnresolvedType(ti) => Err(KError::new(KErrorKind::UnboundName(ti.render()))),
        // A sealed nominal member is a constructor — the only invocable type identity. Every other
        // type is type-shaped but not invocable.
        Carried::Type(kt) => match types.node(kt) {
            TypeNode::SetMember { .. } => Ok(ResolvedCallable::Constructor { identity: kt }),
            _ if type_only => Err(KError::new(KErrorKind::TypeMismatch {
                arg: "verb".to_string(),
                expected: "Type".to_string(),
                got: kt.name(types),
            })),
            _ => Err(KError::new(KErrorKind::DispatchFailed {
                expr: kt.name(types),
                reason: "head evaluates to a non-callable value".to_string(),
            })),
        },
    }
}
