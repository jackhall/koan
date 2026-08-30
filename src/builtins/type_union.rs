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
use crate::machine::{Action, require_ktype};
use crate::machine::{BindingIndex, Body, KError, KErrorKind, Scope};

use super::op_def::OperatorForm;
use super::{arg, kw};
use crate::machine::model::ReturnType;
use crate::machine::model::RunRegistries;
use crate::witnessed::BumpVec;

// This builtin's slot spellings, minted once and read back by symbol.
crate::slots! { SLOTS { left, members, right } }

/// The union operator's glyph, spelled in Rust source and so declared once.
static UNION_OPERATOR: crate::machine::model::StaticName<crate::machine::model::KeywordSymbol> =
    crate::static_name!(crate::machine::model::KeywordSymbol, "|");

const MEMBERS_SLOT: &str = "`|` members";

/// The two-member keyworded form `A | B`: both operands ride resolved-type slots (the shared
/// parameterized-type slot shape), so the body reads each member as owned data and composes the
/// union directly — mirroring `parameterized_types::body_map`. The composite allocates into this
/// step's own region through the single type door.
fn body_binary<'a>(ctx: &crate::machine::BodyCtx<'_, 'a, '_>) -> Action<'a> {
    let left = crate::try_action!(require_ktype(ctx.args, &SLOTS.left, ctx.registries));
    let right = crate::try_action!(require_ktype(ctx.args, &SLOTS.right, ctx.registries));
    // Two members and no more, so they ride a stack array straight into the union door: the
    // pair is read and canonicalized inside the call and never outlives it.
    Action::done(Ok(ctx
        .ctx
        .type_carried(ctx.types().union_of(&[left, right]))))
}

/// The reduced `Unary` form `[Keyword("|"), ListLiteral([members...])]`: the members ride an
/// ordinary evaluated list, exactly as a user-declared `UNARY OP`'s `operands` slot does. Each
/// element resolves through the list literal's own element scheduling — a bare type leaf resolves
/// against scope and parks on a still-finalizing name, a `:(...)` member rides its own
/// sub-dispatch — so every member kind reaches the body already lowered to a `KType` cell, and the
/// composite union builds through [`TypeRegistry::union_of`].
fn body_nary<'a>(ctx: &crate::machine::BodyCtx<'_, 'a, '_>) -> Action<'a> {
    let substrate = match ctx.args.object(&SLOTS.members) {
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
    // The members are read by the union door below and never leave the step, so they stage on the
    // step scratch. One substrate cell contributes at most one member — the loop either pushes or
    // returns — so the cell count is an exact upper bound and the capacity is taken up front.
    let mut members: BumpVec<'a, KType> = BumpVec::with_capacity_in(substrate.len(), ctx.scratch);
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
    Action::done(Ok(ctx.ctx.type_carried(ctx.types().union_of(&members))))
}

/// `|` seeds its triple — the reduced `Unary` form `| [members...]`, the two-member keyworded form
/// `A | B`, and its own single-member `Unary` operator group — through the shared unary-operator
/// door in [`super::op_def`]. The bodies are native: a `KType` composed from owned members, not a
/// synthesized koan AST. A single-member group must never share a group with another operator.
pub fn register<'a>(scope: &'a Scope<'a>, registries: &RunRegistries, gate: &mut WriteGate) {
    let types = &registries.types;
    let nary_elements = [
        kw(registries, "|"),
        arg(registries, &SLOTS.members, types.list(KType::ANY)),
    ];
    let binary_elements = [
        arg(registries, &SLOTS.left, KType::of_kind(KKind::AnyType)),
        kw(registries, "|"),
        arg(registries, &SLOTS.right, KType::of_kind(KKind::AnyType)),
    ];
    let (_carrier, writes) = super::op_def::register_unary_operator(
        scope,
        registries.labels.record(&UNION_OPERATOR),
        OperatorForm {
            return_type: ReturnType::Resolved(KType::of_kind(KKind::AnyType)),
            elements: &nary_elements,
            body: Body::Builtin(body_nary),
        },
        OperatorForm {
            return_type: ReturnType::Resolved(KType::of_kind(KKind::AnyType)),
            elements: &binary_elements,
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
            .apply(scope, registries, gate)
            .expect("builtin `|` unary-operator seeding must not collide");
    }
}

#[cfg(test)]
mod tests;
