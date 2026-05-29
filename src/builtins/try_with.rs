//! `TRY (<expr>) WITH (<branches>)` — runtime error-catching dispatch.
//!
//! Surface shape mirrors [`match_case`](super::match_case); arms key on `ok`, the
//! `KErrorKind` tag from [`KError::to_tagged`](crate::machine::KError::to_tagged),
//! or `_` (wildcard catching dispatcher-internal kinds without a public tag).
//!
//! `expr` is `KExpression` so the catch path can intercept evaluation — an eager
//! slot would short-circuit through eager-subs dep-error propagation before `TRY`'s
//! body ran. Wiring uses an `add_catch` slot: `<expr>` is sub-dispatched and a
//! finish closure walks `<branches>` against the `Result`, dispatching the matched
//! arm (per-call `CallArena` for `it`) or re-raising on no-match.

use std::rc::Rc;

use crate::machine::core::LexicalFrame;
use crate::machine::model::{KObject, KType};
use crate::machine::{
    ArgumentBundle, BindingIndex, BodyResult, CallArena, CatchFinish, KError, KErrorKind,
    RuntimeArena, Scope, SchedulerHandle,
};

use crate::machine::core::kfunction::argument_bundle::extract_kexpression;
use crate::machine::core::kfunction::body::split_body_statements;
use super::branch_walk::find_branch_body;
use super::{arg, err, kw, register_builtin, sig};

pub fn body<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let expr_inner = match extract_kexpression(&mut bundle, "expr") {
        Some(e) => e,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "TRY expr slot must be a parenthesized expression".to_string(),
            )));
        }
    };
    let branches_expr = match extract_kexpression(&mut bundle, "branches") {
        Some(e) => e,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "TRY branches slot must be a parenthesized expression".to_string(),
            )));
        }
    };

    // Body runs in a fresh `child_under` scope so a `LET` inside it stays local
    // and reads still chain out to `scope`.
    let body_scope: &'a Scope<'a> = scope.arena.alloc_scope(Scope::child_under(scope));
    let sub_ids = sched.enter_block(body_scope.id, vec![expr_inner], body_scope);
    let sub_id = sub_ids[0];
    let outer_frame = sched.current_frame();
    let finish: CatchFinish<'a> = Box::new(move |scope, sched, result| {
        dispatch_branch(scope, sched, result, branches_expr, outer_frame)
    });
    let catch_id = sched.add_catch(sub_id, scope, finish);
    BodyResult::DeferTo(catch_id)
}

/// On no match: re-raise the original `KError`, or `ShapeError("TRY missing ok
/// arm")` on the success path without an `ok` or `_` arm.
fn dispatch_branch<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    result: Result<&'a KObject<'a>, KError>,
    branches_expr: crate::machine::model::ast::KExpression<'a>,
    outer_frame: Option<Rc<CallArena>>,
) -> BodyResult<'a> {
    // On `ok`, `it` is the bare success value; on error, the per-variant payload
    // Struct unwrapped from `KError::to_tagged`'s Tagged carrier.
    let (tag, it_value, original_err): (String, KObject<'a>, Option<KError>) = match result {
        Ok(v) => ("ok".to_string(), v.deep_clone(), None),
        Err(e) => {
            let tagged: KObject<'a> = e.to_tagged();
            let (tag, payload) = match tagged {
                KObject::Tagged { tag, value, .. } => (tag, (*value).deep_clone()),
                _ => unreachable!("KError::to_tagged always returns Tagged"),
            };
            (tag, payload, Some(e))
        }
    };

    let body_expr = match find_branch_body(&branches_expr, &tag, true) {
        Ok(Some(body)) => body,
        Ok(None) => {
            return match original_err {
                Some(e) => BodyResult::Err(e),
                None => err(KError::new(KErrorKind::ShapeError(
                    "TRY missing ok arm".to_string(),
                ))),
            };
        }
        Err(msg) => return err(KError::new(KErrorKind::ShapeError(msg))),
    };

    // Frame chains `outer_frame` so the call-site arena outlives the child scope's
    // `outer` pointer while the new frame is live (see `match_case::body`).
    let frame: Rc<CallArena> = CallArena::new(scope, outer_frame);
    let arena_ptr: *const RuntimeArena = frame.arena();
    let scope_ptr: *const Scope<'_> = frame.scope();
    // SAFETY: heap-pinning makes both pointers valid for the Rc's lifetime; the re-borrow
    // ends before the `frame` move into `BodyResult::Tail`.
    let inner_arena: &'a RuntimeArena = unsafe { &*(arena_ptr as *const _) };
    let child: &'a Scope<'a> = unsafe { &*(scope_ptr as *const _) };
    let it_obj: &'a KObject<'a> = inner_arena.alloc(it_value);
    // `nominal_binder: true` carves `it` out of the sibling-index cutoff so the
    // arm body can see it — same path MATCH's `it` uses.
    let _ = child.bind_value(
        "it".to_string(),
        it_obj,
        BindingIndex { idx: 0, nominal_binder: true },
    );
    // Multi-statement arms (`tag -> ((s_0) ... (s_{N-1}))`) submit the first N-1 as
    // siblings at chain indices `1..N-1` and tail-replace into the last at `N`.
    let arm_scope_id = child.id;
    let statements = split_body_statements(body_expr);
    let n = statements.len();
    if n >= 2 {
        let call_site_chain = sched
            .current_lexical_chain()
            .expect("TRY body runs inside an enter_block / active_chain");
        let mut stmts = statements;
        let last = stmts.pop().expect("n >= 2");
        for (i, stmt) in stmts.into_iter().enumerate() {
            let chain = LexicalFrame::push(
                Some(call_site_chain.clone()),
                arm_scope_id,
                i + 1,
            );
            sched.with_active_frame(frame.clone(), &mut |s| {
                s.add_dispatch_with_chain(stmt.clone(), child, chain.clone());
            });
        }
        BodyResult::tail_with_block_at_index(last, Some(frame), arm_scope_id, n)
    } else {
        let only = statements.into_iter().next().expect("n >= 1");
        BodyResult::tail_with_block(only, Some(frame), arm_scope_id)
    }
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
        scope,
        "TRY",
        sig(KType::Any, vec![
            kw("TRY"),
            arg("expr", KType::KExpression),
            kw("WITH"),
            arg("branches", KType::KExpression),
        ]),
        body,
    );
}

#[cfg(test)]
mod tests;
