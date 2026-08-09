//! The await-body-then-seal protocol combinator: a declaration builtin (SIG, MODULE) mints a child
//! scope, dispatches its body block against it as an `InScope` dep, and finishes by capturing the
//! populated scope into a declaration value bound in the parent scope. This file owns the
//! protocol's moving parts — the `AwaitDeps` envelope, the `InScope` placement, and the
//! close-before-capture ordering — so a caller states only its declaration-specific finish. Pattern
//! precedent: [`resolve_or_await`](super::resolve_or_await).

use crate::machine::model::KExpression;
use crate::machine::Scope;
use crate::machine::{Action, AwaitContinue, DepPlacement, FinishCtx, OwnedDispatch};
use crate::scheduler::Deps;

/// Dispatch `body` against `child` (one owned sub-slot per top-level statement, per
/// `DepPlacement::InScope`), then run `finish`. The child closes first: every bind into it resolved
/// with the awaited deps and the finish only reads it, so the sealed reach rides any value that
/// captures the scope.
pub(crate) fn await_body_in_scope<'a>(
    child: &'a Scope<'a>,
    body: KExpression<'a>,
    finish: impl for<'r> FnOnce(&FinishCtx<'a, 'r>) -> Action<'a> + 'a,
) -> Action<'a> {
    let continuation: AwaitContinue<'a> = Box::new(move |fctx, _results| {
        child.close();
        finish(fctx)
    });
    Action::await_deps(
        Deps::from_owned([OwnedDispatch {
            expr: crate::machine::model::WorkingExpression::from_ast(child.brand(), body),
            placement: DepPlacement::InScope(child),
        }]),
        continuation,
    )
}
