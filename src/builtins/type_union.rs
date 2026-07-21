//! `:(A | B)` — the untagged structural union type constructor. The `|` operator is a
//! single-member `Unary`-mode operator group, so a run `A | B | C` reduces to the
//! keyword-first call `[Keyword("|"), ListLiteral([A, B, C])]` (see
//! [`operator_chain::reduce_unary`](crate::machine::execute::dispatch::operator_chain)), while a
//! two-member run `A | B` stays a plain keyworded `[A, |, B]` (an operator chain needs at least
//! two operators). Two overloads cover both shapes; each folds its resolved members through
//! [`TypeRegistry::union_of`], so `:(A | A)` collapses to `:A` and member order never matters.
//!
//! Untagged union *instances* need no construction: a `Number` **is** a valid `:(Number | Str)`
//! value with no wrapper. This builtin only constructs the union *type* as a first-class type
//! value.

use crate::machine::model::KExpression;
use crate::machine::model::KKind;
use crate::machine::model::KObject;
use crate::machine::model::KType;
use crate::machine::model::TypeRegistry;
use crate::machine::{arg_object, require_ktype, Action, AwaitContinue, DepPlacement, DepRequest};
use crate::machine::{BindingIndex, Body, KError, KErrorKind, Scope};

use super::op_def::OperatorForm;
use super::resolve_or_await::expect_type_terminal;
use super::{arg, kw, sig};

const MEMBERS_SLOT: &str = "`|` members";

/// The two-member keyworded form `A | B`: both operands ride resolved-type slots (the shared
/// parameterized-type slot shape), so the body reads each member as owned data and composes the
/// union directly — mirroring `parameterized_types::body_map`. The composite allocates into this
/// step's own region through the single type door.
fn body_binary<'a>(ctx: &crate::machine::BodyCtx<'a, '_>) -> Action<'a> {
    let left = crate::try_action!(require_ktype(ctx.args, "left", ctx.types));
    let right = crate::try_action!(require_ktype(ctx.args, "right", ctx.types));
    Action::Done(Ok(ctx
        .ctx
        .type_carried(ctx.types.union_of(vec![left, right]))))
}

/// The reduced `Unary` form `[Keyword("|"), ListLiteral([members...])]`: the list literal arrives
/// raw as a one-per-part `KExpression` (the `:KExpression` slot captures it unevaluated). Each
/// member part is sub-dispatched on its own — a bare type leaf resolves against scope and parks on
/// a forward reference, a `:(...)` member sub-dispatches to its `KType` — so every member-part kind
/// rides the ordinary type-resolution machinery. `expect_type_terminal` clones each resolved member
/// out of its terminal as owned data, and the composite union builds through [`TypeRegistry::union_of`].
fn body_nary<'a>(ctx: &crate::machine::BodyCtx<'a, '_>) -> Action<'a> {
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
        let mut members: Vec<KType> = Vec::with_capacity(count);
        for position in 0..count {
            let kt = crate::try_action!(expect_type_terminal(
                &results,
                position,
                MEMBERS_SLOT,
                fctx.types
            ));
            members.push(kt);
        }
        Action::Done(Ok(fctx.ctx.type_carried(fctx.types.union_of(members))))
    });
    Action::AwaitDeps { deps, finish }
}

/// `|` seeds its triple — the reduced `Unary` form `| [members...]`, the two-member keyworded form
/// `A | B`, and its own single-member `Unary` operator group — through the shared unary-operator
/// door in [`super::op_def`]. The bodies are native: a `KType` composed from owned members, not a
/// synthesized koan AST. A single-member group must never share a group with another operator.
pub fn register<'a>(scope: &'a Scope<'a>, types: &TypeRegistry) {
    super::op_def::register_unary_operator(
        scope,
        "|",
        OperatorForm {
            signature: sig(
                KType::of_kind(KKind::AnyType),
                vec![kw("|"), arg("members", KType::KEXPRESSION)],
            ),
            body: Body::Builtin(body_nary),
        },
        OperatorForm {
            signature: sig(
                KType::of_kind(KKind::AnyType),
                vec![
                    arg("left", KType::of_kind(KKind::AnyType)),
                    kw("|"),
                    arg("right", KType::of_kind(KKind::AnyType)),
                ],
            ),
            body: Body::Builtin(body_binary),
        },
        // A natively seeded builtin has no group context at all.
        false,
        BindingIndex::BUILTIN,
        types,
    )
    .expect("builtin `|` unary-operator seeding must not collide");
}

#[cfg(test)]
mod tests;
