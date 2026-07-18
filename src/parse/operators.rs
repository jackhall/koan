//! Operator table driving compound-atom desugaring. Each entry pairs a
//! trigger character with an arity-typed builder.
//!
//! Builders receive their operand(s) plus the trigger's span and return a
//! `Spanned<ExpressionPart>` covering the full operand range. The inner
//! synthetic `Keyword("ATTR"|"TRY")` carries the 1-codepoint trigger
//! span so diagnostics can point at the exact operator character.

use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::source::{self, Span, Spanned};

pub type UnaryBuild = for<'a> fn(Spanned<ExpressionPart<'a>>, Span) -> Spanned<ExpressionPart<'a>>;
pub type BinaryBuild = for<'a> fn(
    Spanned<ExpressionPart<'a>>,
    Spanned<ExpressionPart<'a>>,
    Span,
) -> Spanned<ExpressionPart<'a>>;

pub enum OperatorKind {
    /// `lhs <trigger> atom` — builder takes lhs and rhs.
    Infix(BinaryBuild),
    /// `lhs <trigger>` — builder takes the single operand.
    Suffix(UnaryBuild),
}

pub struct Operator {
    pub trigger: char,
    pub kind: OperatorKind,
}

/// `[` and `]` are absent: they're list-literal delimiters handled one level
/// up, so compound indexing like `foo[idx]` isn't expressible here.
const OPERATORS: &[Operator] = &[
    Operator {
        trigger: '.',
        kind: OperatorKind::Infix(build_attr),
    },
    Operator {
        trigger: '?',
        kind: OperatorKind::Suffix(build_try),
    },
];

fn build_attr<'a>(
    lhs: Spanned<ExpressionPart<'a>>,
    rhs: Spanned<ExpressionPart<'a>>,
    trigger: Span,
) -> Spanned<ExpressionPart<'a>> {
    let start = lhs.span.map(|s| s.start).unwrap_or(trigger.start);
    let end = rhs.span.map(|s| s.end).unwrap_or(trigger.end);
    let outer = Span { start, end };
    // A Type-classed field is a type member (`M.T`), so the access is a type operation:
    // wrap it `SigiledTypeExpr` so its result flows into a `ProperType` / `Type` slot. A
    // value field (lowercase `Identifier`, e.g. `M.x`) stays the value-context `Expression`.
    let type_context = matches!(rhs.value, ExpressionPart::Type(_));
    let kw = Spanned::at(ExpressionPart::Keyword("ATTR".to_string()), trigger);
    let kexp = KExpression::build(vec![kw, lhs, rhs], Some(outer), source::current());
    let part = if type_context {
        ExpressionPart::SigiledTypeExpr(Box::new(kexp))
    } else {
        ExpressionPart::Expression(Box::new(kexp))
    };
    Spanned::at(part, outer)
}

fn build_try<'a>(lhs: Spanned<ExpressionPart<'a>>, trigger: Span) -> Spanned<ExpressionPart<'a>> {
    let start = lhs.span.map(|s| s.start).unwrap_or(trigger.start);
    let outer = Span {
        start,
        end: trigger.end,
    };
    let kw = Spanned::at(ExpressionPart::Keyword("TRY".to_string()), trigger);
    let kexp = KExpression::build(vec![kw, lhs], Some(outer), source::current());
    Spanned::at(ExpressionPart::Expression(Box::new(kexp)), outer)
}

/// Variant view returned by `find_suffix`: the operator kinds that appear after an atom.
/// Mirrors [`OperatorKind`] one-to-one now that every operator is a suffix form.
pub enum SuffixOp {
    Infix(BinaryBuild),
    Suffix(UnaryBuild),
}

pub fn find_suffix(c: char) -> Option<SuffixOp> {
    OPERATORS
        .iter()
        .find(|op| op.trigger == c)
        .map(|op| match op.kind {
            OperatorKind::Infix(b) => SuffixOp::Infix(b),
            OperatorKind::Suffix(b) => SuffixOp::Suffix(b),
        })
}

pub fn is_atom_terminator(c: char) -> bool {
    OPERATORS.iter().any(|op| op.trigger == c)
}
