//! Parameter-name reference scan for FN-def's Stage B param-name detection.
//!
//! Answers: *does this carrier contain any leaf whose name matches one of the FN's
//! parameter names?* A `true` result short-circuits the eager-elaborate path — the
//! parameter is not bound in the FN's outer scope, so the carrier becomes a
//! `ReturnType::Deferred(_)` that re-elaborates per call against the dispatch-boundary
//! scope.

use crate::machine::model::ast::{ExpressionPart, KExpression, TypeName};

pub(super) fn type_expr_references_any(te: &TypeName, param_names: &[String]) -> bool {
    param_names.iter().any(|n| n.as_str() == te.as_str())
}

pub(super) fn kexpression_references_any(expr: &KExpression<'_>, param_names: &[String]) -> bool {
    expr.parts
        .iter()
        .any(|p| part_references_any(&p.value, param_names))
}

fn part_references_any(part: &ExpressionPart<'_>, param_names: &[String]) -> bool {
    match part {
        ExpressionPart::Identifier(name) => param_names.iter().any(|n| n == name),
        ExpressionPart::Type(t) => type_expr_references_any(t, param_names),
        ExpressionPart::Expression(boxed) => kexpression_references_any(boxed, param_names),
        ExpressionPart::SigiledTypeExpr(boxed) => kexpression_references_any(boxed, param_names),
        // A `:{…}` field type can reference a param in a nested sigil (`:{y :Er.Type}`).
        ExpressionPart::RecordType(boxed) => kexpression_references_any(boxed, param_names),
        ExpressionPart::ListLiteral(items) => {
            items.iter().any(|p| part_references_any(p, param_names))
        }
        ExpressionPart::DictLiteral(pairs) => pairs.iter().any(|(k, v)| {
            part_references_any(k, param_names) || part_references_any(v, param_names)
        }),
        // Field names are literal strings, never references; scan the values
        // (e.g. `Er` inside `Set WITH {Elt = Er.Type}`).
        ExpressionPart::RecordLiteral(fields) => fields
            .iter()
            .any(|(_, v)| part_references_any(v, param_names)),
        _ => false,
    }
}
