//! Operator table driving compound-atom desugaring. Each entry pairs a
//! trigger character with an arity-typed builder.
//!
//! Builders receive their operand(s) plus the trigger's span and return a
//! `Spanned<ExpressionPart>` covering the full operand range. The inner
//! synthetic `Keyword("ATTR"|"NOT"|"TRY")` carries the 1-codepoint trigger
//! span so diagnostics can point at the exact operator character.

use crate::source::{self, Span, Spanned};
use crate::machine::model::ast::{ExpressionPart, KExpression};

pub type UnaryBuild = for<'a> fn(Spanned<ExpressionPart<'a>>, Span) -> Spanned<ExpressionPart<'a>>;
pub type BinaryBuild = for<'a> fn(
    Spanned<ExpressionPart<'a>>,
    Spanned<ExpressionPart<'a>>,
    Span,
) -> Spanned<ExpressionPart<'a>>;

pub enum OperatorKind {
    /// `<trigger> compound` — builder takes the single operand.
    Prefix(UnaryBuild),
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
        trigger: '!',
        kind: OperatorKind::Prefix(build_not),
    },
    Operator {
        trigger: '.',
        kind: OperatorKind::Infix(build_attr),
    },
    Operator {
        trigger: '?',
        kind: OperatorKind::Suffix(build_try),
    },
];

fn build_prefix<'a>(
    keyword: &'static str,
    operand: Spanned<ExpressionPart<'a>>,
    trigger: Span,
) -> Spanned<ExpressionPart<'a>> {
    let operand_end = operand.span.map(|s| s.end).unwrap_or(trigger.end);
    let outer = Span {
        start: trigger.start,
        end: operand_end,
    };
    let kw = Spanned::at(ExpressionPart::Keyword(keyword.to_string()), trigger);
    let kexp = KExpression::build(vec![kw, operand], Some(outer), source::current());
    Spanned::at(ExpressionPart::Expression(Box::new(kexp)), outer)
}

fn build_not<'a>(expr: Spanned<ExpressionPart<'a>>, trigger: Span) -> Spanned<ExpressionPart<'a>> {
    build_prefix("NOT", expr, trigger)
}

fn build_attr<'a>(
    lhs: Spanned<ExpressionPart<'a>>,
    rhs: Spanned<ExpressionPart<'a>>,
    trigger: Span,
) -> Spanned<ExpressionPart<'a>> {
    let start = lhs.span.map(|s| s.start).unwrap_or(trigger.start);
    let end = rhs.span.map(|s| s.end).unwrap_or(trigger.end);
    let outer = Span { start, end };
    // A Type-classed field is a type member (`M.T`), so the access is a type operation:
    // wrap it `SigiledTypeExpr` so its result flows into a `TypeExprRef` / `Type` slot. A
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

/// Variant view returned by `find_suffix`: only the kinds that can appear
/// after an atom. `Prefix` is structurally absent so `parse_compound`'s match
/// is exhaustive without an `unreachable!` arm.
pub enum SuffixOp {
    Infix(BinaryBuild),
    Suffix(UnaryBuild),
}

pub fn find_prefix(c: char) -> Option<UnaryBuild> {
    OPERATORS.iter().find_map(|op| match op.kind {
        OperatorKind::Prefix(b) if op.trigger == c => Some(b),
        _ => None,
    })
}

pub fn find_suffix(c: char) -> Option<SuffixOp> {
    OPERATORS
        .iter()
        .find(|op| op.trigger == c)
        .and_then(|op| match op.kind {
            OperatorKind::Prefix(_) => None,
            OperatorKind::Infix(b) => Some(SuffixOp::Infix(b)),
            OperatorKind::Suffix(b) => Some(SuffixOp::Suffix(b)),
        })
}

pub fn is_atom_terminator(c: char) -> bool {
    OPERATORS.iter().any(|op| op.trigger == c)
}
