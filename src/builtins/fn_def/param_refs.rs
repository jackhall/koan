//! Parameter-name reference scan for FN-def's Stage B param-name detection.
//!
//! Answers: *does this carrier contain any leaf whose name matches one of the FN's
//! parameter names?* A `true` result short-circuits the eager-elaborate path — the
//! parameter is not bound in the FN's outer scope, so the carrier becomes a
//! `ReturnType::Deferred(_)` that re-elaborates per call against the dispatch-boundary
//! scope.

use crate::machine::model::labels::TypeSymbol;
use crate::machine::model::{ExpressionPart, KExpression, Symbol};

pub(super) fn type_expr_references_any(te: TypeSymbol, param_names: &[String]) -> bool {
    // A parameter name is a reference, not a declaration, so it probes by bare symbol bits.
    param_names.iter().any(|n| Symbol::of(n) == te.symbol())
}

pub(super) fn kexpression_references_any(expr: &KExpression<'_>, param_names: &[String]) -> bool {
    expr.parts
        .iter()
        .any(|p| part_references_any(p.value, param_names))
}

fn part_references_any(part: ExpressionPart<'_>, param_names: &[String]) -> bool {
    match part {
        ExpressionPart::Identifier(name) => param_names.iter().any(|n| n == name),
        ExpressionPart::Type(t) => type_expr_references_any(t, param_names),
        ExpressionPart::Expression(inner) => kexpression_references_any(&inner, param_names),
        ExpressionPart::SigiledTypeExpr(inner) => kexpression_references_any(&inner, param_names),
        // A `:{…}` field type can reference a param in a nested sigil (`:{y :er.Carrier}`).
        ExpressionPart::RecordType(inner) => kexpression_references_any(&inner, param_names),
        ExpressionPart::ListLiteral(items) => {
            items.iter().any(|p| part_references_any(*p, param_names))
        }
        ExpressionPart::DictLiteral(pairs) => pairs.iter().any(|(k, v)| {
            part_references_any(*k, param_names) || part_references_any(*v, param_names)
        }),
        // Field names are literal strings, never references; scan the values
        // (e.g. `er` inside `Set WITH {Elt = er.Carrier}`).
        ExpressionPart::RecordLiteral(fields) => fields
            .iter()
            .any(|(_, v)| part_references_any(*v, param_names)),
        _ => false,
    }
}
