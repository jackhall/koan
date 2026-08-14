//! FunctionValueCall dispatch shape.
//!
//! Head resolution runs before any part walk: a value-bound head dispatches the call
//! immediately, an unbound name errors, and a still-finalizing head placeholder parks via a
//! [`park_resume`] closure that re-runs the fast lane on resume.

use crate::machine::core::ProducerId;
use crate::machine::model::{ExpressionPart, WorkingExpression, WorkingPart};
use crate::machine::{DeliveredCarried, KError, KErrorKind, NameLookup};

use super::apply_callable::{ResolvedCallable, apply_callable};
use super::ctx::SchedulerView;
use super::{Outcome, park_resume};

pub(super) fn initial<'step>(
    ctx: &SchedulerView<'_, 'step, '_>,
    expr: WorkingExpression<'step>,
) -> Outcome<'step> {
    let head = match expr.parts[0].value {
        WorkingPart::Ast(ExpressionPart::Identifier(n)) => n,
        _ => unreachable!("FunctionValueCall shape implies Identifier head"),
    };
    let chain = ctx.chain_deref();
    match ctx.current_scope().resolve_value_delivered(head, chain) {
        // The head is **adopted** into the calling scope's region rather than read bare: the adopt
        // mints the callable's reach there and retains it, so the captured foreign environment
        // outlives the application and the re-anchored value is valid at `'step`. Same door the
        // deferred-head lane takes (`head_deferred::classify_head`).
        Some(NameLookup::Bound(delivered)) => dispatch_callable_value(ctx, expr, &delivered),
        // Head placeholder: park on the binder's claim and re-run the fast lane on resume. A binder
        // that failed before binding the head propagates its error through this park (the harness
        // rules on an already-terminal producer when it installs); one that bound the head wakes
        // the resume onto the `Bound` arm above.
        Some(NameLookup::Parked(source)) => install_head_park(source, expr),
        None => Outcome::Done(Err(KError::new(KErrorKind::UnboundName(head.to_string())))),
    }
}

/// Resolve the already-bound head value to a [`ResolvedCallable`] and hand
/// off to the shared apply-a-callable tail. The head is a value-bound
/// lowercase identifier, so only a `KFunction` is callable —
/// the partition invariant keeps a type out of `bindings.data`, so a
/// constructor-typed head reaches dispatch through the type channel
/// (`HeadDeferred`), never here. Anything else is a non-callable `TypeMismatch`.
fn dispatch_callable_value<'step>(
    ctx: &SchedulerView<'_, 'step, '_>,
    expr: WorkingExpression<'step>,
    delivered: &DeliveredCarried,
) -> Outcome<'step> {
    // The head is **adopted** into the calling scope's region as a callable rather than read bare:
    // the adopt mints its reach there and retains it, so the captured foreign environment outlives
    // the application and the callable rides the apply tail with that reach still proven.
    let callable = match ctx.current_scope().adopt_delivered_function(delivered) {
        Some(function) => ResolvedCallable::Function(function),
        None => {
            let got = delivered.open(|live| live.summarize(ctx.types()));
            return Outcome::Done(Err(KError::new(KErrorKind::TypeMismatch {
                arg: "verb".to_string(),
                expected: "KFunction or Type".to_string(),
                got,
            })));
        }
    };
    apply_callable(ctx, callable, &expr)
}

/// Park the whole call on its head's still-finalizing binder edge `source` and re-run the fast
/// lane on resume. The carrier surfaces the original (unspliced) call expression for the drain-end
/// deadlock summary.
fn install_head_park<'step>(source: ProducerId, expr: WorkingExpression<'step>) -> Outcome<'step> {
    let carrier = expr.summarize();
    park_resume(
        vec![source],
        Some(carrier),
        Box::new(move |ctx, _idx| initial(ctx, expr)),
    )
}
