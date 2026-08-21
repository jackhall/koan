//! `:(A | B)` — the untagged structural union type constructor. The `|` operator is a
//! single-member `Unary`-mode operator group, so a run `A | B | C` reduces to the
//! keyword-first call `[Keyword("|"), ListLiteral([A, B, C])]` (see
//! [`operator_chain::reduce_unary`](crate::machine::execute::decide::operator_chain)), while a
//! two-member run `A | B` stays a plain keyworded `[A, |, B]` (an operator chain needs at least
//! two operators). Two overloads cover both shapes; each folds its resolved members through
//! [`TypeRegistry::union_of`], so `:(A | A)` collapses to `:A` and member order never matters.
//!
//! Untagged union *instances* need no construction: a `Number` **is** a valid `:(Number | Str)`
//! value with no wrapper. This builtin only constructs the union *type* as a first-class type
//! value.

use crate::machine::WriteGate;
use crate::machine::model::Held;
use crate::machine::model::KKind;
use crate::machine::model::KObject;
use crate::machine::model::KType;
use crate::machine::{Action, arg_object, require_ktype};
use crate::machine::{BindingIndex, Body, KError, KErrorKind, Scope};

use super::op_def::OperatorForm;
use super::{arg, kw, sig};
use crate::machine::model::RunRegistries;

const MEMBERS_SLOT: &str = "`|` members";

/// The two-member keyworded form `A | B`: both operands ride resolved-type slots (the shared
/// parameterized-type slot shape), so the body reads each member as owned data and composes the
/// union directly — mirroring `parameterized_types::body_map`. The composite allocates into this
/// step's own region through the single type door.
fn body_binary<'a>(ctx: &crate::machine::BodyCtx<'_, 'a, '_>) -> Action<'a> {
    let left = crate::try_action!(require_ktype(ctx.args, "left", ctx.registries));
    let right = crate::try_action!(require_ktype(ctx.args, "right", ctx.registries));
    Action::done(Ok(ctx
        .ctx
        .type_carried(ctx.types().union_of(vec![left, right]))))
}

/// The reduced `Unary` form `[Keyword("|"), ListLiteral([members...])]`: the members ride an
/// ordinary evaluated list, exactly as a user-declared `UNARY OP`'s `operands` slot does. Each
/// element resolves through the list literal's own element scheduling — a bare type leaf resolves
/// against scope and parks on a still-finalizing name, a `:(...)` member rides its own
/// sub-dispatch — so every member kind reaches the body already lowered to a `KType` cell, and the
/// composite union builds through [`TypeRegistry::union_of`].
fn body_nary<'a>(ctx: &crate::machine::BodyCtx<'_, 'a, '_>) -> Action<'a> {
    let substrate = match arg_object(ctx.args, "members") {
        Some(KObject::List(substrate, _)) => *substrate,
        _ => {
            return Action::done(Err(KError::new(KErrorKind::ShapeError(format!(
                "{MEMBERS_SLOT} slot must be a run of type operands",
            )))));
        }
    };
    if substrate.is_empty() {
        return Action::done(Err(KError::new(KErrorKind::ShapeError(format!(
            "{MEMBERS_SLOT}: a union needs at least one member",
        )))));
    }
    let mut members: Vec<KType> = Vec::with_capacity(substrate.len());
    for cell in substrate.elements() {
        match cell {
            Held::Type(kt) => members.push(*kt),
            other => {
                return Action::done(Err(KError::new(KErrorKind::ShapeError(format!(
                    "{MEMBERS_SLOT}: every member must be a type, got `{}`",
                    other.summarize(ctx.registries),
                )))));
            }
        }
    }
    Action::done(Ok(ctx.ctx.type_carried(ctx.types().union_of(members))))
}

/// `|` seeds its triple — the reduced `Unary` form `| [members...]`, the two-member keyworded form
/// `A | B`, and its own single-member `Unary` operator group — through the shared unary-operator
/// door in [`super::op_def`]. The bodies are native: a `KType` composed from owned members, not a
/// synthesized koan AST. A single-member group must never share a group with another operator.
pub fn register<'a>(scope: &'a Scope<'a>, registries: &RunRegistries, gate: &mut WriteGate) {
    let types = &registries.types;
    let (_carrier, writes) = super::op_def::register_unary_operator(
        scope,
        "|",
        OperatorForm {
            signature: sig(
                KType::of_kind(KKind::AnyType),
                vec![kw("|"), arg(registries, "members", types.list(KType::ANY))],
            ),
            body: Body::Builtin(body_nary),
        },
        OperatorForm {
            signature: sig(
                KType::of_kind(KKind::AnyType),
                vec![
                    arg(registries, "left", KType::of_kind(KKind::AnyType)),
                    kw("|"),
                    arg(registries, "right", KType::of_kind(KKind::AnyType)),
                ],
            ),
            body: Body::Builtin(body_binary),
        },
        // A natively seeded builtin has no group context at all.
        false,
        BindingIndex::BUILTIN,
        registries,
    )
    .expect("builtin `|` unary-operator seeding must not collide");
    // Root seeding: a construction-time door, so the writes apply here rather than riding a step.
    for write in writes {
        write
            .apply(scope, gate)
            .expect("builtin `|` unary-operator seeding must not collide");
    }
}

#[cfg(test)]
mod tests;
