//! Parameter‑name reference scan for FN‑def's Stage B param‑name detection.
//!
//! Answers one question: *"does this carrier (a `TypeExpr` or a `KExpression`)
//! contain any leaf whose name matches one of the FN's parameter names?"* A `true`
//! result short‑circuits the eager‑elaborate path at FN‑def time — the parameter
//! is by construction not bound in the FN's outer scope, so eagerly elaborating
//! the carrier would either fail or produce the wrong answer. Instead the carrier
//! becomes a `ReturnType::Deferred(_)` (or a `Deferred` parameter‑type entry)
//! that re‑elaborates per call against the dispatch‑boundary scope.
//!
//! Two surface forms feed in: a `TypeExpr` (overload 1's `TypeExprRef` carrier)
//! and a `KExpression` (overload 2's `KExpression` carrier). The `KExpression`
//! walker descends into `Expression`, `ListLiteral`, and `DictLiteral` parts so
//! the scan sees every parameter-named leaf the body could reference.

use crate::machine::model::ast::{ExpressionPart, KExpression, TypeExpr, TypeParams};

/// True iff `te` contains any leaf name (top‑level or nested through `TypeParams`)
/// that matches one of `param_names`. Drives overload 1's Stage B decision.
pub(super) fn type_expr_references_any(te: &TypeExpr, param_names: &[String]) -> bool {
    if param_names.iter().any(|n| n == &te.name) {
        return true;
    }
    match &te.params {
        TypeParams::None => false,
        TypeParams::List(items) => items.iter().any(|t| type_expr_references_any(t, param_names)),
        TypeParams::Function { args, ret } => {
            args.iter().any(|t| type_expr_references_any(t, param_names))
                || type_expr_references_any(ret, param_names)
        }
    }
}

/// True iff `expr` contains any leaf — `Identifier(name)` or `Type(TypeExpr { name, .. })`,
/// recursing into nested `Expression` / `ListLiteral` / `DictLiteral` parts — matching
/// one of `param_names`. Drives overload 2's Stage B decision.
pub(super) fn kexpression_references_any(
    expr: &KExpression<'_>,
    param_names: &[String],
) -> bool {
    expr.parts.iter().any(|p| part_references_any(&p.value, param_names))
}

fn part_references_any(part: &ExpressionPart<'_>, param_names: &[String]) -> bool {
    match part {
        ExpressionPart::Identifier(name) => param_names.iter().any(|n| n == name),
        ExpressionPart::Type(t) => type_expr_references_any(t, param_names),
        ExpressionPart::Expression(boxed) => kexpression_references_any(boxed, param_names),
        ExpressionPart::ListLiteral(items) => {
            items.iter().any(|p| part_references_any(p, param_names))
        }
        ExpressionPart::DictLiteral(pairs) => pairs.iter().any(|(k, v)| {
            part_references_any(k, param_names) || part_references_any(v, param_names)
        }),
        _ => false,
    }
}
