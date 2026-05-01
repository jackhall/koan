use crate::parse::kexpression::ExpressionPart;

type Builder = for<'a> fn(Vec<ExpressionPart<'a>>) -> ExpressionPart<'a>;

/// Distinguishes how `parse_compound` gathers operands around a trigger character.
pub enum OperatorKind {
    /// `<trigger> compound` — builder receives `[expr]`.
    Prefix,
    /// `lhs <trigger> atom` — builder receives `[lhs, rhs]`.
    Infix,
    /// `lhs <trigger>` — builder receives `[lhs]`. Like Rust's `?` operator.
    Suffix,
}

/// One row of the operator table. The `kind` drives operand gathering inside `parse_compound`;
/// `build` determines the shape of the resulting expression.
pub struct Operator {
    pub trigger: char,
    pub kind: OperatorKind,
    pub build: Builder,
}

/// Registry of compound-token operators. `parse_compound` dispatches off this table; to add
/// a new operator, append one row and define its builder fn.
///
/// `[` and `]` are intentionally absent: they're list-literal delimiters handled at the
/// `build_tree` level (`[a b c]` → `ExpressionPart::ListLiteral`), not token-internal
/// operators. Compound indexing like `foo[idx]` is therefore not currently expressible — a
/// future syntax for indexing will be reintroduced separately.
const OPERATORS: &[Operator] = &[
    Operator { trigger: '!', kind: OperatorKind::Prefix, build: build_not  },
    Operator { trigger: '.', kind: OperatorKind::Infix,  build: build_attr },
    Operator { trigger: '?', kind: OperatorKind::Suffix, build: build_try  },
];

fn build_not<'a>(mut ops: Vec<ExpressionPart<'a>>) -> ExpressionPart<'a> {
    let expr = ops.pop().unwrap();
    ExpressionPart::expression(vec![ExpressionPart::Token("not".to_string()), expr])
}

fn build_attr<'a>(mut ops: Vec<ExpressionPart<'a>>) -> ExpressionPart<'a> {
    let rhs = ops.pop().unwrap();
    let lhs = ops.pop().unwrap();
    ExpressionPart::expression(vec![ExpressionPart::Token("attr".to_string()), lhs, rhs])
}

fn build_try<'a>(mut ops: Vec<ExpressionPart<'a>>) -> ExpressionPart<'a> {
    let lhs = ops.pop().unwrap();
    ExpressionPart::expression(vec![ExpressionPart::Token("try".to_string()), lhs])
}

pub fn find_prefix(c: char) -> Option<&'static Operator> {
    OPERATORS.iter().find(|op| op.trigger == c && matches!(op.kind, OperatorKind::Prefix))
}

pub fn find_suffix(c: char) -> Option<&'static Operator> {
    OPERATORS.iter().find(|op| op.trigger == c && !matches!(op.kind, OperatorKind::Prefix))
}

pub fn is_atom_terminator(c: char) -> bool {
    OPERATORS.iter().any(|op| op.trigger == c)
}
