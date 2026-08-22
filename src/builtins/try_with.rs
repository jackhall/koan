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

use crate::machine::WriteGate;

use crate::machine::model::KKind;

use crate::machine::model::{Carried, KObject, KType, Symbol};
use crate::machine::{DeliveredCarried, KError, KErrorKind, Scope};

use super::branch_walk::find_branch_body_by_tag;
use super::{arg, kw, sig};
use crate::machine::model::RunRegistries;

/// Watches `expr`, then a `Catch` finish walks the arms against the `Result`, tail-replacing
/// into the matched arm under the `-> :T` contract and re-raising on no match.
pub fn body<'a>(ctx: &crate::machine::BodyCtx<'_, 'a, '_>) -> crate::machine::Action<'a> {
    use super::branch_walk::{arm_tail, payload_envelope, resolve_arm_contract};
    use crate::machine::{Action, CatchContinue, DepPlacement, DepRequest, require_kexpression};

    let expr_inner = crate::try_action!(require_kexpression(ctx.args, "TRY", "expr"));
    let contract = crate::try_action!(resolve_arm_contract(ctx, "TRY"));
    let branches_expr = crate::try_action!(require_kexpression(ctx.args, "TRY", "branches"));
    // Body runs in a fresh `child_under` scope so a `LET` inside it stays local and reads still
    // chain out to the call-site scope.
    let body_scope: &'a Scope<'a> = ctx.scope.alloc_child_under();
    let finish: CatchContinue<'a> = Box::new(move |fctx, result| {
        // On success `it` is the watched value, adopted from its sealed carrier at bind time. On
        // error `it` is the per-variant payload unwrapped from `KError::to_tagged` — that `Tagged`
        // now carries a fresh `Record` substrate (born through a fold door), so it travels as a
        // delivered carrier and adopts through the same copied-adoption tier the success arm's
        // watched value already uses.
        // The arm walk matches by bare symbol bits, so the success tag is a probe: `Ok` is a
        // fixed literal of the `Result` shape and needs no intern to be compared.
        let (tag, it_carrier, original_error): (Symbol, DeliveredCarried, Option<KError>) =
            match result {
                Ok(carrier) => (Symbol::of("Ok"), carrier, None),
                Err(e) => {
                    let envelope = e.to_tagged_delivered(fctx.scope, fctx.registries);
                    let tag = envelope.open(|carried| match carried {
                        Carried::Object(KObject::Tagged { tag, .. }) => tag.symbol(),
                        _ => unreachable!("KError::to_tagged always returns Tagged"),
                    });
                    (tag, payload_envelope(&envelope), Some(e))
                }
            };
        let body_expr = match find_branch_body_by_tag(&branches_expr, tag, true) {
            Ok(Some(body)) => body,
            // On no match: re-raise the original `KError`, or `ShapeError` on the success path
            // without an `Ok` or `_` arm.
            Ok(None) => {
                return match original_error {
                    Some(e) => Action::done(Err(e)),
                    None => Action::done(Err(KError::new(KErrorKind::ShapeError(
                        "TRY missing Ok arm".to_string(),
                    )))),
                };
            }
            Err(msg) => return Action::done(Err(KError::new(KErrorKind::ShapeError(msg)))),
        };
        arm_tail(fctx.scope, it_carrier, body_expr, contract, fctx.registries)
    });
    Action::catch(
        DepRequest::Dispatch {
            expr: crate::machine::model::WorkingExpression::from_ast(
                body_scope.brand(),
                expr_inner,
            ),
            placement: DepPlacement::InScope(body_scope),
        },
        finish,
    )
}

pub fn register<'a>(scope: &'a Scope<'a>, registries: &RunRegistries, gate: &mut WriteGate) {
    let signature = sig(
        KType::ANY,
        vec![
            kw("TRY"),
            arg(registries, "expr", KType::KEXPRESSION),
            kw("->"),
            arg(registries, "return_type", KType::of_kind(KKind::ProperType)),
            kw("WITH"),
            arg(registries, "branches", KType::KEXPRESSION),
        ],
    );
    crate::builtins::register_builtin(scope, "TRY", signature, body, registries, gate);
}

#[cfg(test)]
mod tests;
