//! `TRY (<expr>) -> :<T> WITH (<branches>)` — runtime error-catching dispatch.
//!
//! `-> :T` is the mandatory declared return type every arm agrees on, checked and
//! re-tagged when the selected arm's tail completes (the `ReturnContract::Arm` carried on
//! the tail). Surface shape otherwise mirrors [`match_case`](super::match_case): arms name
//! members of a fixed slate — `Result`'s `Ok`, every kind member of the
//! [`KError`](super::error_union) union, and the `_` default arm — walked by
//! [`find_branch_body_for_member`], the same member walk `MATCH … OVER` selects through.
//!
//! An arm set need not cover the slate: an error no arm names re-raises the original
//! [`KError`], which is what makes `_` optional rather than mandatory.
//!
//! `expr` is `KExpression` so the catch path can intercept evaluation — an eager slot
//! would short-circuit through eager-subs dep-error propagation before `TRY`'s body ran.

use crate::machine::WriteGate;

use crate::machine::model::KKind;

use crate::machine::model::KType;
use crate::machine::{DeliveredCarried, KError, KErrorKind, Scope};

use super::branch_walk::find_branch_body_for_member;
use super::{arg, arg_labeled, kw, sig};
use crate::machine::model::RunRegistries;

// This builtin's slot spellings, minted once and read back by symbol.
crate::slots! { SLOTS { branches, expr, return_type } }

/// The member slate a TRY arm head names: `Result`'s `Ok` first, then every kind member of the
/// `KError` union in declaration order. Read off the two registered prelude types, so an arm head
/// and the member a lowered outcome carries are the same handle.
///
/// The slate crosses the finish as a host-region slice — `Copy`, so the finish stays on the
/// bumped tier.
fn member_slate<'a>(scope: &'a Scope<'a>, registries: &RunRegistries) -> &'a [KType] {
    let types = &registries.types;
    let result = scope
        .resolve_type(crate::builtins::result::RESULT.symbol())
        .expect("Result must be registered before TRY");
    let kerror = scope
        .resolve_type(crate::machine::core::kerror::KERROR.symbol())
        .expect("KError must be registered before TRY");
    let ok = types
        .union_member_named(result, crate::builtins::result::OK.symbol().symbol())
        .expect("Result declares an `Ok` member");
    // The slate is read under one borrow of the `KError` node and filled straight into the scope's
    // region: a TRY body runs per step, so staging it through an owned run would put a heap
    // allocation and a regrow on every iteration of a loop that carries one.
    types.with_node(kerror, |node| {
        let crate::machine::model::TypeNode::Union { members } = node else {
            panic!("KError is registered as a union")
        };
        // `Ok` at index 0, then the kind members in declaration order — an exact-size run, which
        // is what fills a region slice without staging.
        scope
            .brand()
            .allocator()
            .slice_from_iter((0..members.len() + 1).map(|at| match at {
                0 => ok,
                _ => members[at - 1],
            }))
    })
}

/// Watches `expr`, then a `Catch` finish walks the arms against the outcome's own member,
/// tail-replacing into the matched arm under the `-> :T` contract and re-raising on no match.
pub fn body<'a>(ctx: &crate::machine::BodyCtx<'_, 'a, '_>) -> crate::machine::Action<'a> {
    use super::branch_walk::{arm_tail, payload_envelope, resolve_arm_contract};
    use crate::machine::{Action, DepPlacement, DepRequest, require_kexpression};

    let expr_inner = crate::try_action!(require_kexpression(ctx.args, "TRY", &SLOTS.expr));
    let contract = crate::try_action!(resolve_arm_contract(ctx, "TRY"));
    let branches_expr = crate::try_action!(require_kexpression(ctx.args, "TRY", &SLOTS.branches));
    let members = member_slate(ctx.scope, ctx.registries);
    // Body runs in a fresh `child_under` scope so a `LET` inside it stays local and reads still
    // chain out to the call-site scope.
    let body_scope: &'a Scope<'a> = ctx.scope.alloc_child_under();
    // Every capture — the branches expression, the arm contract, the member slate — is `Copy`, so
    // the finish erases onto the bumped tier: a TRY costs no heap allocation for its recovery.
    let finish = move |fctx: &crate::machine::FinishCtx<'a, '_>,
                       result: Result<DeliveredCarried, KError>| {
        // On success `it` is the watched value, adopted from its sealed carrier at bind time, and
        // the selected member is `Ok`. On error `it` is the kind's payload record unwrapped from
        // the lowered `Wrapped` — a fresh `Record` substrate born through a fold door, so it
        // travels as a delivered carrier and adopts through the same copied-adoption tier the
        // success arm's watched value already uses — and the selected member is the carrier's own
        // identity, so selection is a handle compare with nothing to re-derive.
        let (selected, it_carrier, original_error): (KType, DeliveredCarried, Option<KError>) =
            match result {
                Ok(carrier) => (members[0], carrier, None),
                Err(e) => {
                    let envelope = e.to_wrapped_delivered(fctx.scope, fctx.registries);
                    let member = envelope.open(|carried| match carried {
                        crate::machine::model::Carried::Object(object) => object.ktype(),
                        _ => unreachable!("KError::to_wrapped always returns an object"),
                    });
                    (member, payload_envelope(&envelope), Some(e))
                }
            };
        let body_expr = match find_branch_body_for_member(
            &branches_expr,
            members,
            selected,
            "TRY",
            fctx.registries,
            fctx.scratch,
        ) {
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
    };
    Action::catch(
        DepRequest::Dispatch {
            expr: crate::machine::model::WorkingExpression::from_ast(
                body_scope.brand(),
                expr_inner,
            ),
            placement: DepPlacement::InScope(body_scope),
        },
        ctx.brand(),
        finish,
    )
}

pub fn register<'a>(scope: &'a Scope<'a>, registries: &RunRegistries, gate: &mut WriteGate) {
    let signature = sig(
        KType::ANY,
        vec![
            kw(registries, "TRY"),
            arg(registries, &SLOTS.expr, KType::KEXPRESSION),
            kw(registries, "->"),
            arg_labeled(
                registries,
                &SLOTS.return_type,
                KType::of_kind(KKind::ProperType),
                "TRY return type",
            ),
            kw(registries, "WITH"),
            arg(registries, &SLOTS.branches, KType::KEXPRESSION),
        ],
    );
    crate::builtins::register_builtin(scope, signature, body, registries, gate);
}

#[cfg(test)]
mod tests;
