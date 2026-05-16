//! Operator table driving compound-atom desugaring. Each entry pairs a trigger character
//! with an `OperatorKind` whose arity-typed builder constructs the resulting expression.

use crate::machine::model::ast::ExpressionPart;

pub type UnaryBuild  = for<'a> fn(ExpressionPart<'a>) -> ExpressionPart<'a>;
pub type BinaryBuild = for<'a> fn(ExpressionPart<'a>, ExpressionPart<'a>) -> ExpressionPart<'a>;

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

/// `[` and `]` are intentionally absent: they're list-literal delimiters handled one level up,
/// not token-internal operators, so compound indexing like `foo[idx]` is not expressible here.
const OPERATORS: &[Operator] = &[
    Operator { trigger: '!', kind: OperatorKind::Prefix(build_not)  },
    Operator { trigger: '.', kind: OperatorKind::Infix(build_attr)  },
    Operator { trigger: '?', kind: OperatorKind::Suffix(build_try)  },
];

fn build_not<'a>(expr: ExpressionPart<'a>) -> ExpressionPart<'a> {
    ExpressionPart::expression(vec![ExpressionPart::Keyword("NOT".to_string()), expr])
}

fn build_attr<'a>(lhs: ExpressionPart<'a>, rhs: ExpressionPart<'a>) -> ExpressionPart<'a> {
    ExpressionPart::expression(vec![ExpressionPart::Keyword("ATTR".to_string()), lhs, rhs])
}

fn build_try<'a>(lhs: ExpressionPart<'a>) -> ExpressionPart<'a> {
    ExpressionPart::expression(vec![ExpressionPart::Keyword("TRY".to_string()), lhs])
}

/// Variant view returned by `find_suffix`: restricted to the two kinds that can appear
/// after an atom (`Infix`, `Suffix`). `Prefix` is structurally absent so `parse_compound`'s
/// match is exhaustive without an `unreachable!` arm.
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
            OperatorKind::Infix(b)  => Some(SuffixOp::Infix(b)),
            OperatorKind::Suffix(b) => Some(SuffixOp::Suffix(b)),
        })
}

pub fn is_atom_terminator(c: char) -> bool {
    OPERATORS.iter().any(|op| op.trigger == c)
}
