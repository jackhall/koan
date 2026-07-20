//! The await-body-then-seal protocol combinator: a declaration builtin (SIG, MODULE,
//! RECURSIVE TYPES) mints a child scope, dispatches its body block against it as an
//! `InScope` dep, and finishes by capturing the populated scope into a declaration value
//! bound in the parent scope. This file owns the protocol's moving parts — the `AwaitDeps`
//! envelope, the `InScope` placement, and the close-before-capture ordering — so a caller
//! states only its declaration-specific finish. Pattern precedent:
//! [`resolve_or_await`](super::resolve_or_await).

use crate::machine::model::KExpression;
use crate::machine::Scope;
use crate::machine::{Action, AwaitContinue, DepPlacement, DepRequest, FinishCtx};

/// Whether the combinator seals the child scope's reach-set before running the finish.
///
/// `SealBeforeFinish`: every bind into the child resolved with the awaited deps and the
/// finish only reads it — close first, so the sealed reach rides any value that captures
/// the scope. `LeaveOpen`: the caller's finish does not capture the scope into an escaping
/// value (RECURSIVE TYPES: members ride the `RecursiveGroupWindow`, not the scope), so the scope
/// stays open.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChildScopeSeal {
    SealBeforeFinish,
    LeaveOpen,
}

/// Dispatch `body` against `child` (one owned sub-slot per top-level statement, per
/// `DepPlacement::InScope`), then run `finish` — after closing `child` when `seal` says so.
pub(crate) fn await_body_in_scope<'a>(
    child: &'a Scope<'a>,
    body: KExpression<'a>,
    seal: ChildScopeSeal,
    finish: impl for<'r> FnOnce(&FinishCtx<'a, 'r>) -> Action<'a> + 'a,
) -> Action<'a> {
    let continuation: AwaitContinue<'a> = Box::new(move |fctx, _results| {
        if seal == ChildScopeSeal::SealBeforeFinish {
            child.close();
        }
        finish(fctx)
    });
    Action::AwaitDeps {
        deps: vec![DepRequest::Dispatch {
            expr: body,
            placement: DepPlacement::InScope(child),
        }],
        finish: continuation,
    }
}
