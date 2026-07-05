//! `TRY (<expr>) -> :<T> WITH (<branches>)` — runtime error-catching dispatch.
//!
//! `-> :T` is the mandatory declared return type every arm agrees on, checked and
//! re-tagged when the selected arm's value lifts (the `ReturnContract::Arm` carried on
//! the tail). Surface shape otherwise mirrors [`match_case`](super::match_case); arms key
//! on `Ok`, the capitalized `KErrorKind` tag from
//! [`KError::to_tagged`](crate::machine::KError::to_tagged), or `_` (wildcard catching
//! dispatcher-internal kinds without a public tag).
//!
//! `expr` is `KExpression` so the catch path can intercept evaluation — an eager slot
//! would short-circuit through eager-subs dep-error propagation before `TRY`'s body ran.

use crate::machine::model::types::KKind;

use crate::machine::model::{KObject, KType};
use crate::machine::{KError, KErrorKind, Scope};

use super::branch_walk::find_branch_body;
use super::{arg, kw, sig};

/// Watches `expr`, then a `Catch` finish walks the arms against the `Result`, tail-replacing
/// into the matched arm under the `-> :T` contract and re-raising on no match.
pub fn body<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use super::branch_walk::{arm_tail, resolve_arm_contract, ItSource};
    use crate::machine::core::kfunction::action::{
        require_kexpression, Action, CatchContinue, DepPlacement, DepRequest,
    };

    let expr_inner = crate::try_action!(require_kexpression(ctx.args, "TRY", "expr"));
    let contract = crate::try_action!(resolve_arm_contract(ctx, "TRY"));
    let branches_expr = crate::try_action!(require_kexpression(ctx.args, "TRY", "branches"));
    // Body runs in a fresh `child_under` scope so a `LET` inside it stays local and reads still
    // chain out to the call-site scope.
    let body_scope: &'a Scope<'a> = ctx.scope.brand().alloc_scope(Scope::child_under(ctx.scope));
    let outer_frame = ctx.frame.map(|f| f.storage_rc());
    let finish: CatchContinue<'a> = Box::new(move |fctx, result| {
        // On success `it` is the watched value, adopted from its sealed carrier at bind time. On
        // error `it` is the per-variant payload unwrapped from `KError::to_tagged`; that Tagged is
        // region-pure, so its reach is the empty set.
        let (tag, it_source, original_error): (String, ItSource<'a>, Option<KError>) = match result
        {
            Ok(carrier) => ("Ok".to_string(), ItSource::Carrier(carrier), None),
            Err(e) => {
                let tagged: KObject<'a> = e.to_tagged(fctx.scope.brand());
                let (tag, payload) = match tagged {
                    KObject::Tagged { tag, value, .. } => (tag, (*value).deep_clone()),
                    _ => unreachable!("KError::to_tagged always returns Tagged"),
                };
                (
                    tag,
                    ItSource::Value {
                        value: payload,
                        reach: crate::machine::CarrierWitness::empty(),
                    },
                    Some(e),
                )
            }
        };
        let body_expr = match find_branch_body(&branches_expr, &tag, true) {
            Ok(Some(body)) => body,
            // On no match: re-raise the original `KError`, or `ShapeError` on the success path
            // without an `Ok` or `_` arm.
            Ok(None) => {
                return match original_error {
                    Some(e) => Action::Done(Err(e)),
                    None => Action::Done(Err(KError::new(KErrorKind::ShapeError(
                        "TRY missing Ok arm".to_string(),
                    )))),
                };
            }
            Err(msg) => return Action::Done(Err(KError::new(KErrorKind::ShapeError(msg)))),
        };
        arm_tail(fctx.scope, outer_frame, it_source, body_expr, contract)
    });
    Action::Catch {
        watched: DepRequest::Dispatch {
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
