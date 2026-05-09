//! Shared parser for `(<name>: <Type> <name>: <Type> ...)` schema expressions. Used by both
//! `UNION` (which discards order and converts the result to a `HashMap<tag, KType>`) and
//! `STRUCT` (which keeps the ordered list because positional construction depends on it).
//!
//! The identifier/colon/duplicate-name scaffolding is shared with the value-side named-pair
//! parser via [`crate::parse::parse_triple_list`]; the wrapper here adds the type-side
//! interpretation of the third slot (must be a `Type` token, lowered through
//! [`KType::from_type_expr`] with the caller's resolver).

use super::ktype::KType;
use super::resolver::TypeResolver;
use crate::parse::kexpression::{ExpressionPart, KExpression};
use crate::parse::parse_triple_list;

/// Walk the schema KExpression's parts as repeated `<Identifier(name)> <Keyword(":")>
/// <Type(name)>` triples and assemble the resulting ordered list. Errors with a
/// `ShapeError`-string on any malformed triple, unknown type name, or duplicate field name.
/// `context` is the surface-form name (`UNION schema`, `STRUCT schema`) used in error
/// messages so the caller's diagnostic stays grounded in user-facing syntax. `resolver` is
/// forwarded into `KType::from_type_expr` so module-local / user-defined names can resolve
/// before the builtin name table.
///
/// Thin wrapper over [`parse_triple_list`] that closes over the type-side third-slot
/// interpretation. Callers pass `"UNION schema"` / `"STRUCT schema"` for the context so the
/// generic shared scaffolding's diagnostics still mention "schema" verbatim.
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
