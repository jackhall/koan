//! Dispatch-shape classification: read-only view of how a `KFunction`'s
//! signature matches the expression a slot is dispatching for late dispatch.
//!
//! The "bare-name" predicate ([`is_bare_name`]) is the load-bearing shape concept the auto-wrap
//! rail turns on.

use crate::machine::model::types::carried_channel_ktype;
use crate::machine::model::{Argument, KType, SignatureElement, TypeRegistry};
use crate::machine::model::{WorkingExpression, WorkingPart};
use crate::parse::ExpressionPart;

use super::KFunction;
use crate::machine::model::RunRegistries;
use crate::memory::{BumpAllocator, BumpVec};

/// The auto-wrap classification produced by [`KFunction::classify_for_pick`]: the bare-Identifier /
/// bare-Type parts in non-literal-name slots, to resolve as sub-Dispatches. A literal-name slot
/// (`KType::Identifier`, `KType::NameToken`, `KType::TypeNameToken`, or a union with such a
/// member) is excluded: its whole content *is* the token — a declaration's name, or name-data the
/// body reads — so the token rides to the bind unresolved. A kind expectation asks for a type
/// value, not a name, so a bare name at one wraps and resolves like any other.
///
/// Bump-hosted in the scratch arena the classifier was handed, so a classification costs no heap
/// allocation.
pub type WrapIndices<'s> = BumpVec<'s, usize>;

impl<'a> KFunction<'a> {
    /// Which slots of `expr` this signature auto-wraps — see [`WrapIndices`].
    ///
    /// `parked` marks the slots whose bare name resolved to a still-finalizing producer. One of
    /// those is wrapped only if the pre-admission park would have waited on it: at a
    /// [park-exempt slot](WorkingExpression::park_exempt_slot) the lane deliberately did *not*
    /// wait, so there is no resolved cell to splice and the token rides raw to the body that owns
    /// it. Wrapping there would splice a hole and deadlock the declaration group besides.
    pub fn classify_for_pick<'e, 's>(
        &self,
        expr: &WorkingExpression<'e>,
        parked: &[bool],
        types: &TypeRegistry,
        scratch: BumpAllocator<'s>,
    ) -> WrapIndices<'s> {
        let mut wrap_indices: WrapIndices<'s> =
            BumpVec::with_capacity_in(expr.parts.len(), scratch);
        for (i, (el, part)) in self
            .signature
            .elements()
            .iter()
            .zip(expr.parts.iter())
            .enumerate()
        {
            let SignatureElement::Argument(arg) = el else {
                continue;
            };
            if !is_bare_name(&part.value) {
                continue;
            }
            // A literal-name slot owns its token; every other slot's bare name resolves.
            if arg.ktype.owns_bare_name(types) {
                continue;
            }
            // The one parked slot that keeps its raw token: strict admission took it on shape
            // because the binder body owns that name, so there is no resolved cell to splice.
            if expr.body_owns_parked_name(
                i,
                &part.value,
                arg.ktype,
                parked.get(i).copied().unwrap_or(false),
                types,
            ) {
                continue;
            }
            wrap_indices.push(i);
        }
        wrap_indices
    }
}

/// Whether `part` satisfies `arg`'s declared parameter type. An AST slot classifies by part shape
/// ([`KType::accepts_part`](crate::machine::model::KType::accepts_part)); a resolved sub-result
/// classifies by the carrier resting in its cell, opened at that cell's own brand. A synthesized
/// nested node and a staging hole denote no value yet, so neither satisfies a slot.
pub fn slot_admits(arg: &Argument, part: &WorkingPart<'_>, registries: &RunRegistries) -> bool {
    let types = &registries.types;
    match part {
        WorkingPart::Ast(ast) => arg.matches(ast, types),
        WorkingPart::Spliced { cell, .. } => arg.ktype.accepts_cell(cell, registries),
        WorkingPart::Expression(_) | WorkingPart::RecordType(_) | WorkingPart::StagedSlot => false,
    }
}

/// The type an argument part carries at a **quantified** slot: what the unifier reads the
/// declared position against. A callable answers on the channel the slot declares — a shape slot
/// reads its registered shape, every other slot the lambda type it reports as a value — so the
/// two channels stay apart here exactly as they do in
/// [`accepts_carried`](crate::machine::model::KType::accepts_carried). A part the scheduler has
/// not resolved to a value denotes no type yet, so it solves nothing.
pub fn carried_slot_ktype(
    part: &WorkingPart<'_>,
    declared: KType,
    registries: &RunRegistries,
) -> Option<KType> {
    match part {
        WorkingPart::Ast(ast) => KType::slot_ktype(ast, &registries.types),
        WorkingPart::Spliced { cell, .. } => Some(crate::machine::model::read_resting(cell, |c| {
            carried_channel_ktype(c, declared, registries)
        })),
        WorkingPart::Expression(_) | WorkingPart::RecordType(_) | WorkingPart::StagedSlot => None,
    }
}

/// True iff `part` is the "bare-name" shape — a bare `Identifier` or a leaf
/// `Type`-token. Both name-shaped parts ride the same auto-wrap and
/// dispatch-park rails, so the symmetry is load-bearing for `LET T = Number`
/// vs `LET y = z` walking identical scheduler paths.
fn is_bare_name(part: &WorkingPart<'_>) -> bool {
    matches!(
        part.as_ast(),
        Some(ExpressionPart::Identifier(_) | ExpressionPart::Type(_))
    )
}
