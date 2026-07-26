//! FunctionValueCall dispatch shape.
//!
//! Head resolution runs before any part walk: a value-bound head dispatches the call
//! immediately, an unbound name errors, and a still-finalizing head placeholder parks via a
//! [`park_resume`] closure that re-runs the fast lane on resume.

use crate::machine::model::{Carried, KObject};
use crate::machine::model::{ExpressionPart, KExpression};
use crate::machine::{KError, KErrorKind, NameLookup, NodeId};

use super::apply_callable::{apply_callable, ResolvedCallable};
use super::ctx::SchedulerView;
use super::{park_resume, Outcome, ProducerStanding};

pub(super) fn initial<'step>(
    ctx: &SchedulerView<'step, '_>,
    expr: KExpression<'step>,
) -> Outcome<'step> {
    let head = match &expr.parts[0].value {
        ExpressionPart::Identifier(n) => n.clone(),
        _ => unreachable!("FunctionValueCall shape implies Identifier head"),
    };
    let chain = ctx.chain_deref();
    match ctx.current_scope().resolve_value_delivered(&head, chain) {
        // The head is **adopted** into the calling scope's region rather than read bare: the adopt
        // mints the callable's reach there and retains it, so the captured foreign environment
        // outlives the application and the re-anchored value is valid at `'step`. Same door the
        // deferred-head lane takes (`head_deferred::classify_head`).
        Some(NameLookup::Bound(delivered)) => {
            let head_value = ctx.current_scope().adopt_sealed(&delivered);
            dispatch_callable_value(ctx, expr, head_value)
        }
        // Head placeholder. `Errored` means the binder failed before binding the head, so the name
        // never became a value — propagate. `Ready` means the producer finalized without binding the
        // head as a value, so the name is unbound. `Park` re-runs the fast lane on resume.
        Some(NameLookup::Parked(producer)) => match ctx.producer_standing(producer) {
            ProducerStanding::Errored(e) => Outcome::Done(Err(e.clone_for_propagation())),
            ProducerStanding::Ready => {
                Outcome::Done(Err(KError::new(KErrorKind::UnboundName(head))))
            }
            ProducerStanding::Park => install_head_park(producer, expr),
        },
        None => Outcome::Done(Err(KError::new(KErrorKind::UnboundName(head)))),
    }
}

/// Resolve the already-bound head value to a [`ResolvedCallable`] and hand
/// off to the shared apply-a-callable tail. The head is a value-bound
/// lowercase identifier, so only a `KFunction` is callable —
/// the partition invariant keeps a type out of `bindings.data`, so a
/// constructor-typed head reaches dispatch through the type channel
/// (`HeadDeferred`), never here. Anything else is a non-callable `TypeMismatch`.
fn dispatch_callable_value<'step>(
    ctx: &SchedulerView<'step, '_>,
    expr: KExpression<'step>,
    head_value: Carried<'step>,
) -> Outcome<'step> {
    let callable = match head_value {
        Carried::Object(KObject::KFunction(f)) => ResolvedCallable::Function(f),
        other => {
            return Outcome::Done(Err(KError::new(KErrorKind::TypeMismatch {
                arg: "verb".to_string(),
                expected: "KFunction or Type".to_string(),
                got: other.summarize(ctx.types()),
            })))
        }
    };
    apply_callable(ctx, callable, &expr)
}

/// Park the whole call on its still-finalizing head `producer` and re-run the fast lane on
/// resume. The carrier surfaces the original (unspliced) call expression for the drain-end
/// deadlock summary.
fn install_head_park<'step>(producer: NodeId, expr: KExpression<'step>) -> Outcome<'step> {
    let carrier = expr.summarize();
    park_resume(
        vec![producer],
        Some(carrier),
        Box::new(move |ctx, _idx| initial(ctx, expr)),
    )
}
