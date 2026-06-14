//! FunctionValueCall dispatch shape.
//!
//! Head resolution runs before any part walk: a value-bound head dispatches the call
//! immediately, an unbound name errors, and a still-finalizing head placeholder parks via a
//! [`park_resume`] closure that re-runs the fast lane on resume.

use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::KObject;
use crate::machine::model::Parseable;
use crate::machine::{KError, KErrorKind, NodeId, Resolution};

use super::super::nodes::NodeOutput;
use super::apply_callable::{apply_callable, ResolvedCallable};
use super::ctx::SchedulerView;
use super::{park_resume, Outcome};

pub(super) fn initial<'run>(
    ctx: &SchedulerView<'run, '_>,
    expr: KExpression<'run>,
) -> Outcome<'run> {
    let head = match &expr.parts[0].value {
        ExpressionPart::Identifier(n) => n.clone(),
        _ => unreachable!("FunctionValueCall shape implies Identifier head"),
    };
    let chain = ctx.chain_deref();
    match ctx.current_scope().resolve_with_chain(&head, chain) {
        Resolution::Value(obj) => dispatch_callable_value(ctx, expr, obj),
        Resolution::Placeholder(producer) => install_head_park(producer, expr),
        Resolution::UnboundName => {
            Outcome::Done(NodeOutput::Err(KError::new(KErrorKind::UnboundName(head))))
        }
    }
}

/// Resolve the already-bound head value to a [`ResolvedCallable`] and hand
/// off to the shared apply-a-callable tail. The head is a value-bound
/// lowercase identifier, so only a `KFunction` (functor or not) is callable —
/// the partition invariant keeps a type out of `bindings.data`, so a
/// constructor-typed head reaches dispatch through the type channel
/// (`HeadDeferred`), never here. Anything else is a non-callable `TypeMismatch`.
fn dispatch_callable_value<'run>(
    ctx: &SchedulerView<'run, '_>,
    expr: KExpression<'run>,
    head_obj: &'run KObject<'run>,
) -> Outcome<'run> {
    let callable = match head_obj {
        KObject::KFunction(f, _) => ResolvedCallable::Function(f),
        other => {
            return Outcome::Done(NodeOutput::Err(KError::new(KErrorKind::TypeMismatch {
                arg: "verb".to_string(),
                expected: "KFunction or Type".to_string(),
                got: other.summarize(),
            })))
        }
    };
    apply_callable(ctx, callable, &expr)
}

/// Park the whole call on its still-finalizing head `producer` and re-run the fast lane on
/// resume. The carrier surfaces the original (unspliced) call expression for the drain-end
/// deadlock summary.
fn install_head_park<'run>(producer: NodeId, expr: KExpression<'run>) -> Outcome<'run> {
    let carrier = expr.summarize();
    park_resume(
        vec![producer],
        Some(carrier),
        Box::new(move |ctx, _idx| initial(ctx, expr)),
    )
}
