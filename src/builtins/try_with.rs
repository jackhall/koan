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
//! arm (per-call `CallFrame` for `it`) or re-raising on no-match.

use crate::machine::model::types::KKind;

use crate::machine::model::{KObject, KType};
use crate::machine::{KError, KErrorKind, Scope};

use super::branch_walk::find_branch_body;
use super::{arg, kw, sig};

/// Watches `expr` in a fresh `child_under` body scope, then a `Catch` finish walks the arms
/// against the `Result` and tail-replaces into the matched arm (per-call frame with `it` bound)
/// carrying the `-> :T` `Arm` contract, re-raising on no match.
pub fn body<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use super::branch_walk::{arm_tail, resolve_arm_contract};
    use crate::machine::core::kfunction::action::{
        require_kexpression, Action, CatchContinue, Dep, DepPlacement,
    };

    let expr_inner = crate::try_action!(require_kexpression(ctx.args, "TRY", "expr"));
    let contract = crate::try_action!(resolve_arm_contract(ctx, "TRY"));
    let branches_expr = crate::try_action!(require_kexpression(ctx.args, "TRY", "branches"));
    // Body runs in a fresh `child_under` scope so a `LET` inside it stays local and reads still
    // chain out to the call-site scope.
    let body_scope: &'a Scope<'a> = ctx.scope.brand().alloc_scope(Scope::child_under(ctx.scope));
    let outer_frame = ctx.frame.map(|f| f.storage_rc());
    let finish: CatchContinue<'a> = Box::new(move |fctx, result| {
        // On `ok`, `it` is the bare success value; on error, the per-variant payload Struct
        // unwrapped from `KError::to_tagged`'s Tagged carrier.
        // TRY-WITH reads the watched value (relocated into the consumer region) to bind `it` and
        // tail-replaces into the matched branch; it builds no object, so it ignores `ok.carrier`.
        // Alongside the `it` value, capture the scrutinee's reach so the `it` binding stores it: on
        // `ok`, the watched value's own carrier witness; on error, the freshly-built Tagged payload is
        // region-pure (reaches nothing foreign), so the empty set.
        let (tag, it_value, it_witness, original_err): (
            String,
            KObject<'a>,
            crate::machine::FrameSet,
            Option<KError>,
        ) = match result {
            Ok(ok) => (
                "Ok".to_string(),
                ok.value.object().deep_clone(),
                ok.carrier.witness().clone(),
                None,
            ),
            Err(e) => {
                let tagged: KObject<'a> = e.to_tagged(fctx.scope.brand());
                let (tag, payload) = match tagged {
                    KObject::Tagged { tag, value, .. } => (tag, (*value).deep_clone()),
                    _ => unreachable!("KError::to_tagged always returns Tagged"),
                };
                (tag, payload, crate::machine::FrameSet::empty(), Some(e))
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
        arm_tail(
            fctx.scope,
            outer_frame,
            it_value,
            it_witness,
            body_expr,
            contract,
        )
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
            arg("return_type", KType::OfKind(KKind::ProperType)),
            kw("WITH"),
            arg("branches", KType::KExpression),
        ],
    );
    crate::builtins::register_builtin(scope, "TRY", signature, body);
}

#[cfg(test)]
mod tests;
