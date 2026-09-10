//! The part views the machine's field-list walkers read both expression families through.
//!
//! [`ExpressionPart`](crate::parse::ast::ExpressionPart) and
//! [`WorkingPart`](super::working::WorkingPart) each report themselves in one vocabulary, so a
//! walker over a parsed field list and one over a self-reference-threaded field list is written
//! once. The structural classification itself — [`PartClass`], [`DispatchShape`] and the node cache
//! — is the parser's, in [`crate::parse::ast::shape`].

use crate::machine::model::RunRegistries;
use crate::machine::model::SplicedCell;
use crate::parse::ast::{KExpression, PartClass};
use crate::parse::labels::{TypeSymbol, ValueSymbol};

use super::working::WorkingExpression;

/// One `<name> <slot>` position of a field list, viewed through what the field-list elaborator does
/// with it. The two part families answer in this one vocabulary, so the walker in
/// [`typed_field_list`](crate::machine::model::types) is written once and reads a parsed field list
/// and a self-reference-threaded one through the same arms.
///
/// The `Ast*` and `Threaded*` pairs are the same syntax at two stages: a `:(…)` or `:{…}` whose
/// co-declared references are still bare names, and one whose references the sigil-body rewrite has
/// already sealed into [`Resolved`](FieldSlot::Resolved) cells.
pub enum FieldSlot<'a> {
    /// A bare identifier — a field or parameter name, minted by the parse that classified it.
    Name(ValueSymbol),
    /// A type name token: a field's declared type, or a capitalized field / variant name.
    Type(TypeSymbol),
    /// `:(…)` still holding parsed AST — thread its co-declared references, then sub-dispatch it.
    AstSigil(&'a KExpression<'a>),
    /// `:{…}` still holding parsed AST — elaborate its field list inline.
    AstRecord(&'a KExpression<'a>),
    /// A sigil body already threaded — sub-dispatch it as it stands.
    ThreadedSigil(&'a WorkingExpression<'a>),
    /// A record body already threaded — elaborate its field list inline.
    ThreadedRecord(&'a WorkingExpression<'a>),
    /// A resolved carrier the threading wrote in: a co-declared sibling's handle.
    Resolved(SplicedCell<'a>),
    /// Any other shape, which no field-list position accepts.
    Other,
}

/// A part of either expression family, viewed through the classifications the shared readers need:
/// [`class`](Part::class) for dispatch shape, bucket key and operator probe, and
/// [`field_slot`](Part::field_slot) for the field-list walk. Implemented by
/// [`ExpressionPart`](super::ExpressionPart) and [`WorkingPart`](super::WorkingPart).
pub trait Part<'a>: Copy {
    fn class(&self) -> PartClass;

    /// This part read as a field-list position. See [`FieldSlot`].
    fn field_slot(&self) -> FieldSlot<'a>;

    /// Surface rendering, for the field walker's shape diagnostics, written straight into `f`.
    /// Takes the whole run bundle, not just its interner: a resolved
    /// [`Spliced`](super::WorkingPart::Spliced) slot renders the *type* of the argument it
    /// carries, and a type names itself through the type registry.
    ///
    /// The AST family renders through the interner alone, so
    /// [`ExpressionPart::write_summary`](super::ExpressionPart::write_summary) keeps that narrower
    /// signature and its impl here forwards `&registries.labels`. Parse reaches that inherent view
    /// before a run frame exists — the interner it fills is the one the run frame later adopts —
    /// so the AST view cannot take the bundle.
    ///
    /// A write signature rather than a view-returning one because the trait is used through
    /// `&dyn`-shaped generic walkers: a borrowed view would need an associated type per
    /// implementor, where one formatter argument stays object-safe and composes the same.
    fn write_summary(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        registries: &RunRegistries,
    ) -> std::fmt::Result;
}

/// A [`Part::write_summary`] view — the `format!`-embeddable form the field walker's diagnostics
/// use, so a part's surface lands in the message's own buffer.
pub fn part_summary<'x, 'a, P: Part<'a>>(
    part: &'x P,
    registries: &'x RunRegistries,
) -> PartSummary<'x, P> {
    PartSummary { part, registries }
}

pub struct PartSummary<'x, P> {
    part: &'x P,
    registries: &'x RunRegistries,
}

impl<'a, P: Part<'a>> std::fmt::Display for PartSummary<'_, P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.part.write_summary(f, self.registries)
    }
}
