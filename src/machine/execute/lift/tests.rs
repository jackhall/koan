//! Tests for `lift_kobject`, split by lifted KObject variant:
//!
//! - [`kfuture`] — KFuture borrow/anchor arms (parts, bundle.args, parsed-literals).
//! - [`composites`] — List / Dict / Tagged composite recursion and Rc reuse.
//! - [`leaf`] — KFunction, KModule, and primitive (slow-path catch-all) arms.

mod composites;
mod kfuture;
mod leaf;

use super::*;
use crate::machine::model::{KObject, Parseable};
use crate::machine::{CallArena, KError, KErrorKind, ResolveOutcome, Scope};

/// Test-only `(scope, expr) → KFuture` driver for one-shot bind without spinning a
/// `Scheduler`.
pub(super) fn dispatch_for_test<'run>(
    scope: &'run Scope<'run>,
    expr: KExpression<'run>,
) -> Result<KFuture<'run>, KError> {
    let chain = crate::machine::LexicalFrame::detached();
    match scope.resolve_dispatch(&expr, Some(&chain), &[]) {
        ResolveOutcome::Resolved(r) => r.function.bind(expr),
        ResolveOutcome::Ambiguous(n) => Err(KError::new(KErrorKind::AmbiguousDispatch {
            expr: expr.summarize(),
            candidates: n,
        })),
        ResolveOutcome::UnboundName(name) => Err(KError::new(KErrorKind::UnboundName(name))),
        ResolveOutcome::Deferred
        | ResolveOutcome::Unmatched
        | ResolveOutcome::ParkOnProducers(_) => Err(KError::new(KErrorKind::DispatchFailed {
            expr: expr.summarize(),
            reason: "no matching function".to_string(),
        })),
    }
}

/// Stamp a sentinel KFunction into `dying.arena()` so `functions_is_empty()` is false
/// and `lift_kobject` enters the slow path. Side-effect only — the alloc'd ref is
/// discarded; the function lives until `dying`'s arena drops.
pub(super) fn defeat_fast_path(dying: &Rc<CallArena>) {
    use crate::machine::model::{ExpressionSignature, KType, ReturnType, SignatureElement};
    use crate::machine::{Body, KFunction};
    let kf = KFunction::new(
        ExpressionSignature {
            return_type: ReturnType::Resolved(KType::Null),
            elements: vec![SignatureElement::Keyword("__SLOW__".into())],
        },
        Body::Action(|ctx| {
            crate::machine::core::kfunction::action::Action::Done(Ok(
                crate::machine::model::Carried::Object(ctx.scope.arena.alloc_object(KObject::Null)),
            ))
        }),
        dying.scope(),
    );
    let _ = dying.arena().alloc_function(kf);
}

/// A KFunction whose `captured_scope` lives in the dying arena. Caller is responsible
/// for not allocating a separate bait — this KFunction itself defeats `functions_is_empty`.
pub(super) fn alloc_local_kf<'run>(
    dying: &'run Rc<CallArena>,
) -> &'run crate::machine::KFunction<'run> {
    use crate::machine::model::{ExpressionSignature, KType, ReturnType, SignatureElement};
    use crate::machine::{Body, KFunction};
    let kf = KFunction::new(
        ExpressionSignature {
            return_type: ReturnType::Resolved(KType::Null),
            elements: vec![SignatureElement::Keyword("__INNER__".into())],
        },
        Body::Action(|ctx| {
            crate::machine::core::kfunction::action::Action::Done(Ok(
                crate::machine::model::Carried::Object(ctx.scope.arena.alloc_object(KObject::Null)),
            ))
        }),
        dying.scope(),
    );
    dying.arena().alloc_function(kf)
}
