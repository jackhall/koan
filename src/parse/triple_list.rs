//! Generic walker for `<Identifier> : <slot>` triple lists. Hosts the shared
//! length-/identifier-/colon-/dup-name scaffolding for named-value pairs and typed
//! field lists; the per-slot interpretation is supplied by a `parse_third` closure.

use crate::ast::{ExpressionPart, KExpression};

/// Walk `expr.parts` as repeated `<Identifier(name)> <Keyword(":")> <slot>` triples and
/// return an ordered list of `(name, T)` pairs.
///
/// `context` is a surface-form description (`"struct construction"`, `"UNION schema"`, ...)
/// woven into every error message so the diagnostic stays grounded in user-facing syntax.
/// Errors are stringly-typed `Err(String)` so this helper stays free of a
/// `KError`/`KErrorKind` dependency; callers wrap them in `KErrorKind::ShapeError`.
///
/// Empty `parts` yields an empty `Vec` — supports zero-arg calls like `f ()` and empty
/// schemas like `STRUCT Empty = ()`.
pub fn parse_triple_list<'a, T>(
    expr: &KExpression<'a>,
    context: &str,
    mut parse_third: impl FnMut(&ExpressionPart<'a>, &str) -> Result<T, String>,
) -> Result<Vec<(String, T)>, String> {
    let parts = &expr.parts;
    if !parts.len().is_multiple_of(3) {
        return Err(format!(
            "{context} must be `<name>: <slot>` triples; got {} parts (not a multiple of 3)",
            parts.len(),
        ));
    }
    let mut out: Vec<(String, T)> = Vec::with_capacity(parts.len() / 3);
    let mut i = 0;
    while i < parts.len() {
        let name = match &parts[i] {
            ExpressionPart::Identifier(s) => s.clone(),
            other => {
                return Err(format!(
                    "{context} name must be a bare identifier, got {}",
                    other.summarize(),
                ));
            }
        };
        match &parts[i + 1] {
            ExpressionPart::Keyword(k) if k == ":" => {}
            other => {
                return Err(format!(
                    "{context} separator must be `:`, got {}",
                    other.summarize(),
                ));
            }
        }
        if out.iter().any(|(n, _)| n == &name) {
            return Err(format!("duplicate name `{}` in {context}", name));
        }
        let third = parse_third(&parts[i + 2], &name)?;
        out.push((name, third));
        i += 3;
    }
    Ok(out)
}
