//! Generic walker for `<Identifier> : <slot>` triple lists. Two consumers — the named-value
//! pair parser used by struct construction / first-class function calls
//! ([`dispatch::values::named_pairs::parse_named_value_pairs`]) and the typed-field-list
//! parser used by `STRUCT` / `UNION` schemas
//! ([`dispatch::types::typed_field_list::parse_typed_field_list`]) — share identical
//! length-/identifier-/colon-/dup-name scaffolding and differ only in how they interpret the
//! third slot. This helper hosts the scaffolding once and accepts a per-call `parse_third`
//! closure for the per-slot interpretation, so the two callers reduce to thin wrappers.
//!
//! Placement under `parse/` rather than alongside either consumer because both consumers
//! live in different `dispatch::` submodules and `parse/` already hosts the shared
//! expression-walking primitives (`expression_tree`, `kexpression`). Keeping the helper
//! parser-side avoids pulling either consumer's imports into the other.

use crate::parse::kexpression::{ExpressionPart, KExpression};

/// Walk `expr.parts` as repeated `<Identifier(name)> <Keyword(":")> <slot>` triples and
/// return an ordered list of `(name, T)` pairs. `parse_third` interprets the third slot of
/// each triple — the value-side caller closes over `Ok(part.clone())`, the type-side caller
/// closes over `KType::from_type_expr` (with the resolver captured by reference).
///
/// `context` is a surface-form description (`"struct construction"`, `"UNION schema"`, ...)
/// woven into every error message so the diagnostic stays grounded in user-facing syntax.
/// Errors are stringly-typed `Err(String)` because both consumers wrap them in
/// `KErrorKind::ShapeError` at their call sites — keeping this helper string-only spares it
/// a `KError`/`KErrorKind` dependency it doesn't otherwise need.
///
/// Empty `parts` yields an empty `Vec` — supports zero-arg calls like `f ()` and empty
/// schemas like `STRUCT Empty = ()`.
pub fn parse_triple_list<'a, T>(
    expr: &KExpression<'a>,
    context: &str,
    parse_third: impl Fn(&ExpressionPart<'a>, &str) -> Result<T, String>,
) -> Result<Vec<(String, T)>, String> {
    let parts = &expr.parts;
    if parts.len() % 3 != 0 {
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
