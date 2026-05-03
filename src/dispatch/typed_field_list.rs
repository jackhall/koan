//! Shared parser for `(<name>: <Type> <name>: <Type> ...)` schema expressions. Used by both
//! `UNION` (which discards order and converts the result to a `HashMap<tag, KType>`) and
//! `STRUCT` (which keeps the ordered list because positional construction depends on it).

use crate::dispatch::kfunction::KType;
use crate::parse::kexpression::{ExpressionPart, KExpression};

/// Walk the schema KExpression's parts as repeated `<Identifier(name)> <Keyword(":")>
/// <Type(name)>` triples and assemble the resulting ordered list. Errors with a
/// `ShapeError`-string on any malformed triple, unknown type name, or duplicate field name.
/// `context` is the surface-form name (`UNION`, `STRUCT`) used in error messages so the
/// caller's diagnostic stays grounded in user-facing syntax.
pub fn parse_typed_field_list(
    expr: &KExpression<'_>,
    context: &str,
) -> Result<Vec<(String, KType)>, String> {
    let parts = &expr.parts;
    if parts.len() % 3 != 0 {
        return Err(format!(
            "{context} schema must be `<name>: <Type>` triples; got {} parts (not a multiple of 3)",
            parts.len()
        ));
    }
    let mut fields: Vec<(String, KType)> = Vec::with_capacity(parts.len() / 3);
    let mut i = 0;
    while i < parts.len() {
        let name = match &parts[i] {
            ExpressionPart::Identifier(s) => s.clone(),
            other => {
                return Err(format!(
                    "{context} schema name must be a bare identifier, got {}",
                    other.summarize()
                ));
            }
        };
        match &parts[i + 1] {
            ExpressionPart::Keyword(k) if k == ":" => {}
            other => {
                return Err(format!(
                    "{context} schema separator must be `:`, got {}",
                    other.summarize()
                ));
            }
        }
        let type_name = match &parts[i + 2] {
            ExpressionPart::Type(s) => s.clone(),
            other => {
                return Err(format!(
                    "{context} schema type for `{}` must be a type name token, got {}",
                    name,
                    other.summarize()
                ));
            }
        };
        let ktype = KType::from_name(&type_name).ok_or_else(|| {
            format!(
                "unknown type name `{}` in {context} schema for `{}`",
                type_name, name
            )
        })?;
        if fields.iter().any(|(n, _)| n == &name) {
            return Err(format!("duplicate name `{}` in {context} schema", name));
        }
        fields.push((name, ktype));
        i += 3;
    }
    Ok(fields)
}
