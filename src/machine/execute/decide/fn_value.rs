//! FunctionValueCall dispatch shape.
//!
//! A bare `Identifier` head resolves by scope walk alone, so it gets a
//! fast lane: head resolution runs before any part walk — a value-bound head dispatches the call
//! immediately, an unbound name errors, and a still-finalizing head placeholder parks via a
//! [`park_resume`] closure that re-runs the fast lane on resume.
//!
//! A head is callable whenever it *evaluates* to something callable, so the shape also admits a
//! slot the scheduler staged or filled. That head carries no name to walk, so it takes the general
//! lane ([`head_deferred::defer_head`](super::head_deferred::defer_head)), which evaluates the head
//! and applies its resolved value through the same apply-a-callable tail.

use crate::machine::ProducerId;
use crate::machine::model::{ExpressionPart, WorkingExpression, WorkingPart};
use crate::machine::{DeliveredCarried, KError, KErrorKind, NameLookup};

use super::apply_callable::{ResolvedCallable, apply_callable};
use super::ctx::DecideCtx;
use super::{Outcome, park_resume};

pub(super) fn initial<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    expr: WorkingExpression<'step>,
) -> Outcome<'step> {
    let WorkingPart::Ast(ExpressionPart::Identifier(head)) = expr.parts[0].value else {
        return super::head_deferred::defer_head(ctx, expr);
    };
    let chain = ctx.chain_deref();
    match ctx.current_scope().resolve_value_delivered(head, chain) {
        Some(NameLookup::Bound(delivered)) => dispatch_callable_value(ctx, expr, &delivered),
        // A binder that failed before binding the head propagates its error through the park (the
        // harness rules on an already-terminal producer when it installs); one that bound the head
        // wakes the resume onto the `Bound` arm above.
        Some(NameLookup::Parked(source)) => install_head_park(source, expr, ctx),
        None => Outcome::Done(Err(KError::new(KErrorKind::UnboundName(
            ctx.registries().labels.render(head.symbol()),
        )))),
    }
}

/// Only a `KFunction` is admitted: a Type-token head never lands in `bindings.data` (the
/// token-class partition), so it is classified by its own head shape rather than here. Anything
/// else this lane resolves is a non-callable `TypeMismatch`.
fn dispatch_callable_value<'step>(
    ctx: &DecideCtx<'_, 'step, '_>,
    expr: WorkingExpression<'step>,
    delivered: &DeliveredCarried,
) -> Outcome<'step> {
    // Adopted as a callable rather than read bare: the adopt mints its reach in the calling
    // scope's region and retains it, so the captured foreign environment outlives the application
    // and the callable rides the apply tail with that reach still proven.
    let callable = match ctx.current_scope().adopt_delivered_function(delivered) {
        Some(function) => ResolvedCallable::Function(function),
        None => {
            let got = delivered.open(|live| live.summarize(ctx.registries()));
            return Outcome::Done(Err(KError::new(KErrorKind::TypeMismatch {
                arg: "verb".to_string(),
                expected: "KFunction".to_string(),
                got,
            })));
        }
    };
    apply_callable(ctx, callable, &expr)
}

fn install_head_park<'step>(
    source: ProducerId,
    expr: WorkingExpression<'step>,
    view: &DecideCtx<'_, 'step, '_>,
) -> Outcome<'step> {
    park_resume(
        vec![source],
        view,
        move |ctx: &DecideCtx<'_, 'step, '_>, _idx| initial(ctx, expr),
    )
}
