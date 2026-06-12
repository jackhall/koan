//! `TRY (<expr>) -> :<T> WITH (<branches>)` — runtime error-catching dispatch.
//!
//! `-> :T` is the mandatory declared return type every arm agrees on, checked and
//! re-tagged when the selected arm's value lifts (the `ReturnContract::Arm` carried on
//! the tail). Surface shape otherwise mirrors [`match_case`](super::match_case); arms key
//! on `Ok`, the capitalized `KErrorKind` tag from
//! [`KError::to_tagged`](crate::machine::KError::to_tagged), or `_` (wildcard catching
//! dispatcher-internal kinds without a public tag).
//!
//! `expr` is `KExpression` so the catch path can intercept evaluation — an eager
//! slot would short-circuit through eager-subs dep-error propagation before `TRY`'s
//! body ran. Wiring uses an `add_catch` slot: `<expr>` is sub-dispatched and a
//! finish closure walks `<branches>` against the `Result`, dispatching the matched
//! arm (per-call `CallArena` for `it`) or re-raising on no-match.

use crate::machine::model::types::KKind;
use std::rc::Rc;

use crate::machine::core::LexicalFrame;
use crate::machine::model::{KObject, KType};
use crate::machine::{
    ArgumentBundle, BindingIndex, BodyResult, CallArena, CatchFinish, KError, KErrorKind,
    SchedulerHandle, Scope,
};

use super::branch_walk::{find_branch_body, resolve_arm_return_contract};
use super::{arg, err, kw, sig};
#[cfg(not(feature = "action-harness"))]
use super::register_builtin;
use crate::machine::core::kfunction::body::split_body_statements;
use crate::machine::core::kfunction::body::ReturnContract;

pub fn body<'a, 's>(
    sched: &mut dyn SchedulerHandle<'a, 's>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let expr_inner = match bundle.extract_kexpression_or_shape_error("TRY", "expr") {
        Ok(e) => e,
        Err(e) => return err(e),
    };
    let contract = match resolve_arm_return_contract(
        sched.current_scope(),
        &mut bundle,
        "TRY",
        sched.current_lexical_chain(),
    ) {
        Ok(c) => c,
        Err(e) => return err(e),
    };
    let branches_expr = match bundle.extract_kexpression_or_shape_error("TRY", "branches") {
        Ok(e) => e,
        Err(e) => return err(e),
    };

    // Body runs in a fresh `child_under` scope so a `LET` inside it stays local
    // and reads still chain out to the call-site scope.
    let body_scope: &'a Scope<'a> = sched
        .current_scope()
        .arena
        .alloc_scope(Scope::child_under(sched.current_scope()));
    let sub_ids = sched.enter_block(body_scope.id, vec![expr_inner], body_scope);
    let sub_id = sub_ids[0];
    let outer_frame = sched.current_frame();
    let finish: CatchFinish<'a> = Box::new(move |sched, result| {
        dispatch_branch(sched, result, branches_expr, outer_frame, contract)
    });
    let catch_id = sched.add_catch_here(sub_id, finish);
    BodyResult::DeferTo(catch_id)
}

/// On no match: re-raise the original `KError`, or `ShapeError("TRY missing ok
/// arm")` on the success path without an `ok` or `_` arm.
fn dispatch_branch<'a, 's>(
    sched: &mut dyn SchedulerHandle<'a, 's>,
    result: Result<&'a KObject<'a>, KError>,
    branches_expr: crate::machine::model::ast::KExpression<'a>,
    outer_frame: Option<Rc<CallArena>>,
    contract: ReturnContract<'a>,
) -> BodyResult<'a> {
    // On `ok`, `it` is the bare success value; on error, the per-variant payload
    // Struct unwrapped from `KError::to_tagged`'s Tagged carrier.
    let (tag, it_value, original_err): (String, KObject<'a>, Option<KError>) = match result {
        Ok(v) => ("Ok".to_string(), v.deep_clone(), None),
        Err(e) => {
            let tagged: KObject<'a> = e.to_tagged(sched.current_scope().arena);
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
                    "TRY missing Ok arm".to_string(),
                ))),
            };
        }
        Err(msg) => return err(KError::new(KErrorKind::ShapeError(msg))),
    };

    // Chain the call-site frame per per-call-arena-protocol.md § Outer-frame chain.
    let frame: Rc<CallArena> = CallArena::new(sched.current_scope(), outer_frame);
    // `it` binds at idx 0; the arm body's statements sit at idx >= 1, so the strict
    // `idx < cutoff` rule lets the body see it — same path MATCH's `it` uses. The per-call
    // re-anchor is concentrated in `with_anchored_child`; arm statements dispatch via
    // `add_dispatch_with_chain_in_frame`, which stores `Yoked` and re-projects from the frame
    // cart at the read boundary — so the seed itself fabricates no `&'a`.
    frame.with_anchored_child(|arena, child| {
        let it_obj = arena.alloc_object(it_value);
        let _ = child.bind_value("it".to_string(), it_obj, BindingIndex::value(0));
    });
    // Multi-statement arms (`tag -> ((s_0) ... (s_{N-1}))`) submit the first N-1 as
    // siblings at chain indices `1..N-1` and tail-replace into the last at `N`.
    let arm_scope_id = frame.scope_for_bind().id;
    let statements = split_body_statements(body_expr);
    let n = statements.len();
    if n >= 2 {
        let call_site_chain = sched
            .current_lexical_chain()
            .expect("TRY body runs inside an enter_block / active_chain");
        let mut stmts = statements;
        let last = stmts.pop().expect("n >= 2");
        for (i, stmt) in stmts.into_iter().enumerate() {
            let chain = LexicalFrame::push(Some(call_site_chain.clone()), arm_scope_id, i + 1);
            sched.with_active_frame(frame.clone(), &mut |s| {
                s.add_dispatch_with_chain_in_frame(stmt.clone(), chain.clone());
            });
        }
        BodyResult::tail_with_block_at_index(last, Some(frame), arm_scope_id, n, Some(contract))
    } else {
        let only = statements.into_iter().next().expect("n >= 1");
        BodyResult::tail_with_block(only, Some(frame), arm_scope_id, Some(contract))
    }
}

/// `Action`-harness twin of [`body`]: watches `expr` in a fresh `child_under` body scope, then a
/// `Catch` finish walks the arms against the `Result` and tail-replaces into the matched arm (per-
/// call frame with `it` bound) carrying the `-> :T` `Arm` contract, re-raising on no match.
#[cfg(feature = "action-harness")]
pub fn body_action<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use super::branch_walk::{arm_tail, resolve_arm_contract};
    use crate::machine::core::kfunction::action::{require_kexpression, Action, CatchCont, Dep, DepPlacement};

    let expr_inner = match require_kexpression(ctx.args, "TRY", "expr") {
        Ok(e) => e,
        Err(e) => return Action::Done(Err(e)),
    };
    let contract = match resolve_arm_contract(ctx, "TRY") {
        Ok(c) => c,
        Err(e) => return Action::Done(Err(e)),
    };
    let branches_expr = match require_kexpression(ctx.args, "TRY", "branches") {
        Ok(e) => e,
        Err(e) => return Action::Done(Err(e)),
    };
    // Body runs in a fresh `child_under` scope so a `LET` inside it stays local and reads still
    // chain out to the call-site scope.
    let body_scope: &'a Scope<'a> = ctx.scope.arena.alloc_scope(Scope::child_under(ctx.scope));
    let outer_frame = ctx.frame.map(Rc::clone);
    let finish: CatchCont<'a> = Box::new(move |fctx, result| {
        // On `ok`, `it` is the bare success value; on error, the per-variant payload Struct
        // unwrapped from `KError::to_tagged`'s Tagged carrier.
        let (tag, it_value, original_err): (String, KObject<'a>, Option<KError>) = match result {
            Ok(v) => ("Ok".to_string(), v.deep_clone(), None),
            Err(e) => {
                let tagged: KObject<'a> = e.to_tagged(fctx.scope.arena);
                let (tag, payload) = match tagged {
                    KObject::Tagged { tag, value, .. } => (tag, (*value).deep_clone()),
                    _ => unreachable!("KError::to_tagged always returns Tagged"),
                };
                (tag, payload, Some(e))
            }
        };
        let body_expr = match find_branch_body(&branches_expr, &tag, true) {
            Ok(Some(body)) => body,
            // On no match: re-raise the original `KError`, or `ShapeError` on the success path
            // without an `Ok` or `_` arm.
            Ok(None) => {
                return match original_err {
                    Some(e) => Action::Done(Err(e)),
                    None => Action::Done(Err(KError::new(KErrorKind::ShapeError(
                        "TRY missing Ok arm".to_string(),
                    )))),
                };
            }
            Err(msg) => return Action::Done(Err(KError::new(KErrorKind::ShapeError(msg)))),
        };
        arm_tail(fctx.scope, outer_frame, it_value, body_expr, contract)
    });
    Action::Catch {
        watched: Dep::Dispatch {
            expr: expr_inner,
            placement: DepPlacement::InScope(body_scope),
        },
        finish,
    }
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    let signature = sig(
        KType::Any,
        vec![
            kw("TRY"),
            arg("expr", KType::KExpression),
            kw("->"),
            arg("return_type", KType::OfKind(KKind::Proper)),
            kw("WITH"),
            arg("branches", KType::KExpression),
        ],
    );
    #[cfg(feature = "action-harness")]
    crate::builtins::register_action_builtin(scope, "TRY", signature, body_action);
    #[cfg(not(feature = "action-harness"))]
    register_builtin(scope, "TRY", signature, body);
}

#[cfg(test)]
mod tests;
