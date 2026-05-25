//! `TRY (<expr>) WITH (<branches>)` — runtime error-catching dispatch.
//!
//! Surface shape mirrors [`match_case`](super::match_case): branches are repeated
//! `<tag> -> <body>` triples walked by the shared [`branch_walk`](super::branch_walk).
//! The decisive difference is the tag space: `ok` for the success path, the
//! `KErrorKind` variant names for the user-facing error subset, and `_` as a wildcard
//! that catches anything else (including dispatcher-internal kinds that have no public
//! tag). Per-arm `it` shape is documented on the roadmap and pinned by [`KError::to_tagged`].
//!
//! Both slots are `KExpression` (lazy). `expr` must be lazy so the catching path can
//! intercept its evaluation — an eager slot would short-circuit through `run_bind`'s
//! dep-error propagation before `TRY`'s body ever ran. The catching wiring is a
//! [`NodeWork::Catch`](crate::machine::execute::Scheduler) slot (`add_catch`): TRY
//! schedules `<expr>` as a sub-dispatch, then registers a finish closure that walks
//! `<branches>` against the resolved `Result`. The closure either dispatches the picked
//! arm (per-call `CallArena` for `it`, mirrored from MATCH) or re-raises the original
//! `KError` on no-match.

use std::rc::Rc;

use crate::machine::model::{KObject, KType};
use crate::machine::{
    ArgumentBundle, BodyResult, CallArena, CatchFinish, KError, KErrorKind, RuntimeArena, Scope,
    SchedulerHandle,
};

use crate::machine::core::kfunction::argument_bundle::extract_kexpression;
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

    let sub_id = sched.add_dispatch(expr_inner, scope);
    let outer_frame = sched.current_frame();
    let finish: CatchFinish<'a> = Box::new(move |scope, sched, result| {
        dispatch_branch(scope, sched, result, branches_expr, outer_frame)
    });
    let catch_id = sched.add_catch(sub_id, scope, finish);
    BodyResult::DeferTo(catch_id)
}

/// Pick the branch matching `result`'s tag (`ok` on success, the `KErrorKind` variant
/// name on failure, or `_` wildcard) and dispatch it as a tail expression with `it`
/// bound to the per-arm payload. On no match: re-raise the original `KError` for the
/// error path, or synthesize a `ShapeError("TRY missing ok arm")` for the success path
/// without an `ok` or `_` arm.
fn dispatch_branch<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    result: Result<&'a KObject<'a>, KError>,
    branches_expr: crate::machine::model::ast::KExpression<'a>,
    outer_frame: Option<Rc<CallArena>>,
) -> BodyResult<'a> {
    // Compute (tag, it_value, original_err) once. On `ok`, `it` is the bare success
    // value; on error, `it` is the per-variant payload Struct extracted from
    // `KError::to_tagged`'s wrapping Tagged carrier.
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

    // Per-call frame for `it`, mirrored from `match_case::body`. The frame chains the
    // outer call-site frame on its `outer_frame` so the call-site arena outlives the
    // child scope's `outer` pointer while the new frame is live.
    let frame: Rc<CallArena> = CallArena::new(scope, outer_frame);
    let arena_ptr: *const RuntimeArena = frame.arena();
    let scope_ptr: *const Scope<'_> = frame.scope();
    // SAFETY: heap-pinning makes both pointers valid for the Rc's lifetime; the re-borrow
    // ends before the `frame` move into `BodyResult::Tail`.
    let inner_arena: &'a RuntimeArena = unsafe { &*(arena_ptr as *const _) };
    let child: &'a Scope<'a> = unsafe { &*(scope_ptr as *const _) };
    // Bind `it` into the per-call child scope. Dispatch resolves every `Identifier("it")`
    // in the branch body — including those reached via EVAL of a top-level-`#`-quote — by
    // walking from the per-call child to its outer chain. Pinned by
    // `it_resolves_via_scope_for_eval_of_top_level_quoted_reference`.
    let it_obj: &'a KObject<'a> = inner_arena.alloc(it_value);
    let _ = child.bind_value("it".to_string(), it_obj);
    let _ = sched;
    BodyResult::Tail { expr: body_expr, frame: Some(frame), function: None }
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
