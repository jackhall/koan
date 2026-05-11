//! Shared parser for `(<name>: <Type> <name>: <Type> ...)` schema expressions, used by
//! `UNION` (order discarded into a `HashMap<tag, KType>`) and `STRUCT` (order preserved for
//! positional construction).

use super::ktype::KType;
use super::resolver::TypeResolver;
use crate::parse::{parse_triple_list, ExpressionPart, KExpression};

/// Parse repeated `<Identifier(name)> <Keyword(":")> <Type(name)>` triples into an ordered
/// list. Errors as a `ShapeError`-string on malformed triples, unknown type names, or
/// duplicate field names. `context` (e.g. `"UNION schema"`) is interpolated into diagnostics.
pub fn parse_typed_field_list(
    expr: &KExpression<'_>,
    context: &str,
    resolver: &dyn TypeResolver,
) -> Result<Vec<(String, KType)>, String> {
    parse_triple_list(expr, context, |part, name| match part {
        ExpressionPart::Type(t) => KType::from_type_expr(t, resolver)
            .map_err(|e| format!("{e} in {context} for `{}`", name)),
        other => Err(format!(
            "{context} type for `{}` must be a type name token, got {}",
            name,
            other.summarize()
        )),
    })
}
