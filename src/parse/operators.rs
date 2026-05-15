//! Operator table driving compound-atom desugaring. Each entry pairs a trigger character
//! with an `OperatorKind` and a builder that wraps surrounding operands into a nested
//! `ExpressionPart`.

use crate::runtime::machine::model::ast::ExpressionPart;

type Builder = for<'a> fn(Vec<ExpressionPart<'a>>) -> ExpressionPart<'a>;

pub enum OperatorKind {
    /// `<trigger> compound` — builder receives `[expr]`.
    Prefix,
    /// `lhs <trigger> atom` — builder receives `[lhs, rhs]`.
    Infix,
    /// `lhs <trigger>` — builder receives `[lhs]`.
    Suffix,
}

pub struct Operator {
    pub trigger: char,
    pub kind: OperatorKind,
    pub build: Builder,
}

/// `[` and `]` are intentionally absent: they're list-literal delimiters handled one level up,
/// not token-internal operators, so compound indexing like `foo[idx]` is not expressible here.
const OPERATORS: &[Operator] = &[
    Operator { trigger: '!', kind: OperatorKind::Prefix, build: build_not  },
    Operator { trigger: '.', kind: OperatorKind::Infix,  build: build_attr },
    Operator { trigger: '?', kind: OperatorKind::Suffix, build: build_try  },
];

fn build_not<'a>(mut ops: Vec<ExpressionPart<'a>>) -> ExpressionPart<'a> {
    let expr = ops.pop().unwrap();
    ExpressionPart::expression(vec![ExpressionPart::Keyword("NOT".to_string()), expr])
}

fn build_attr<'a>(mut ops: Vec<ExpressionPart<'a>>) -> ExpressionPart<'a> {
    let rhs = ops.pop().unwrap();
    let lhs = ops.pop().unwrap();
    ExpressionPart::expression(vec![ExpressionPart::Keyword("ATTR".to_string()), lhs, rhs])
}

fn build_try<'a>(mut ops: Vec<ExpressionPart<'a>>) -> ExpressionPart<'a> {
    let lhs = ops.pop().unwrap();
    ExpressionPart::expression(vec![ExpressionPart::Keyword("TRY".to_string()), lhs])
}

/// Variant view returned by `find_suffix`: restricted to the two kinds that can appear
/// after an atom (`Infix`, `Suffix`). `Prefix` is structurally absent so `parse_compound`'s
/// match is exhaustive without an `unreachable!` arm.
pub enum SuffixOp {
    Infix(Builder),
    Suffix(Builder),
}

pub fn find_prefix(c: char) -> Option<&'static Operator> {
    OPERATORS.iter().find(|op| op.trigger == c && matches!(op.kind, OperatorKind::Prefix))
}

pub fn find_suffix(c: char) -> Option<SuffixOp> {
    OPERATORS
        .iter()
        .find(|op| op.trigger == c)
        .and_then(|op| match op.kind {
            OperatorKind::Prefix => None,
            OperatorKind::Infix => Some(SuffixOp::Infix(op.build)),
            OperatorKind::Suffix => Some(SuffixOp::Suffix(op.build)),
        })
}

pub fn is_atom_terminator(c: char) -> bool {
    OPERATORS.iter().any(|op| op.trigger == c)
}
