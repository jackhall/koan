//! `:(A | B)` — the untagged structural union type constructor. The `|` operator is a
//! single-member `Unary`-mode operator group, so a run `A | B | C` reduces to the
//! keyword-first call `[Keyword("|"), ListLiteral([A, B, C])]` (see
//! [`operator_chain::reduce_unary`](crate::machine::execute::dispatch::operator_chain)), while a
//! two-member run `A | B` stays a plain keyworded `[A, |, B]` (an operator chain needs at least
//! two operators). Two overloads cover both shapes; each folds its resolved members through
//! [`KType::union_of`], so `:(A | A)` collapses to `:A` and member order never matters.
//!
//! Untagged union *instances* need no construction: a `Number` **is** a valid `:(Number | Str)`
//! value with no wrapper. This builtin only constructs the union *type* as a first-class type
//! value.

use crate::machine::core::kfunction::action::{
    arg_object, require_ktype, Action, AwaitContinue, DepPlacement, DepRequest,
};
use crate::machine::core::KoanStepContextExt;
use crate::machine::model::ast::KExpression;
use crate::machine::model::types::KKind;
use crate::machine::model::values::KObject;
use crate::machine::model::KType;
use crate::machine::{DeliveredCarried, KError, KErrorKind, Scope};

use super::resolve_or_await::expect_type_terminal;
use super::{arg, kw, sig};

const MEMBERS_SLOT: &str = "`|` members";

/// The two-member keyworded form `A | B`: both operands ride resolved-type slots (the shared
/// parameterized-type slot shape), so the body folds two `KType`s directly — mirroring
/// `parameterized_types::body_map`.
fn body_binary<'a>(ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>) -> Action<'a> {
    let left = crate::try_action!(require_ktype(ctx.args, "left"));
    let right = crate::try_action!(require_ktype(ctx.args, "right"));
    let carriers: Vec<&DeliveredCarried> = [ctx.arg_carrier("left"), ctx.arg_carrier("right")]
        .into_iter()
        .flatten()
        .collect();
    Action::Done(Ok(ctx
        .ctx
        .alloc_type_with(&carriers, KType::union_of(vec![left, right]))))
}

/// The reduced `Unary` form `[Keyword("|"), ListLiteral([members...])]`: the list literal arrives
/// raw as a one-per-part `KExpression` (the `:KExpression` slot captures it unevaluated). Each
/// member part is sub-dispatched on its own — a bare type leaf resolves against scope and parks on
/// a forward reference, a `:(...)` member sub-dispatches to its `KType` — so every member-part kind
/// rides the ordinary type-resolution machinery. Their resolved `KType`s fold through
/// [`KType::union_of`].
fn body_nary<'a>(ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>) -> Action<'a> {
    let members = match arg_object(ctx.args, "members") {
        Some(KObject::KExpression(e)) => e.clone(),
        _ => {
            return Action::Done(Err(KError::new(KErrorKind::ShapeError(format!(
                "{MEMBERS_SLOT} slot must be a run of type operands",
            )))))
        }
    };
    if members.parts.is_empty() {
        return Action::Done(Err(KError::new(KErrorKind::ShapeError(format!(
            "{MEMBERS_SLOT}: a union needs at least one member",
        )))));
    }
    let count = members.parts.len();
    let deps: Vec<DepRequest<'a>> = members
        .parts
        .into_iter()
        .map(|part| DepRequest::Dispatch {
            expr: KExpression::new(vec![part]),
            placement: DepPlacement::OwnScope,
        })
        .collect();
    let finish: AwaitContinue<'a> = Box::new(move |fctx, results| {
        let mut resolved: Vec<KType<'a>> = Vec::with_capacity(count);
        let mut carriers: Vec<&DeliveredCarried> = Vec::with_capacity(count);
        for position in 0..count {
            let (kt, carrier) =
                crate::try_action!(expect_type_terminal(&results, position, MEMBERS_SLOT));
            resolved.push(kt);
            carriers.push(carrier);
        }
        Action::Done(Ok(fctx
            .ctx
            .alloc_type_with(&carriers, KType::union_of(resolved))))
    });
    Action::AwaitDeps { deps, finish }
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    use crate::machine::model::operators::{OperatorGroup, ReductionMode};
    use crate::machine::BindingIndex;
    use std::collections::HashSet;

    // Two-member keyworded form: `A | B`.
    super::register_builtin(
        scope,
        "|",
        sig(
            KType::OfKind(KKind::AnyType),
            vec![
                arg("left", KType::OfKind(KKind::AnyType)),
                kw("|"),
                arg("right", KType::OfKind(KKind::AnyType)),
            ],
        ),
        body_binary,
    );
    // Reduced `Unary` form: `| [members...]`.
    super::register_builtin(
        scope,
        "|",
        sig(
            KType::OfKind(KKind::AnyType),
            vec![kw("|"), arg("members", KType::KExpression)],
        ),
        body_nary,
    );

    // `|` is its own single-member `Unary` group, registered here so the operator and its target
    // builtin live together. A single-member group must never share a group with another operator.
    let members: HashSet<String> = ["|"].iter().map(|s| s.to_string()).collect();
    let group = scope
        .brand()
        .alloc_operator_group(OperatorGroup::new(members, ReductionMode::Unary));
    scope
        .register_operator_group("|".to_string(), group, BindingIndex::BUILTIN)
        .expect("builtin `|` operator-group seeding must not collide");
}

#[cfg(test)]
mod tests;
