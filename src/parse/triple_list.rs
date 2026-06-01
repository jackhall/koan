//! Generic walkers for ordered `<Identifier> ... <slot>` field/argument lists.
//!
//! Two shapes share the scaffolding — identifier validation, duplicate-name detection,
//! and length / separator structure — and the per-slot interpretation is supplied by a
//! `parse_third` / `parse_second` closure:
//!
//! - [`parse_pair_list`]: `<Identifier> <slot>` PAIRS — used for typed field declarations
//!   (STRUCT, SIG, FN signature). The Design-B type sigil consumes the `:`, so a typed
//!   parameter `xs :Number` lands as `[Identifier("xs"), Type(Number)]`.
//! - [`parse_keyword_triple_list`]: `<Identifier> <Keyword(sep)> <slot>` TRIPLES — used
//!   for named-value pairs (`Point (x = 3, y = 4)`, function calls `f (a = 1, b = 2)`).

use crate::machine::model::ast::{ExpressionPart, KExpression};

/// `context` is woven into every error message; `sep` is the expected keyword text
/// between name and slot (typically `"="`). Empty `parts` yields an empty `Vec` so
/// zero-arg calls like `f ()` are handled here.
pub fn parse_keyword_triple_list<'a, T>(
    expr: &KExpression<'a>,
    context: &str,
    sep: &str,
    mut parse_third: impl FnMut(&ExpressionPart<'a>, &str) -> Result<T, String>,
) -> Result<Vec<(String, T)>, String> {
    let parts = &expr.parts;
    if !parts.len().is_multiple_of(3) {
        return Err(format!(
            "{context} must be `<name> {sep} <slot>` triples; got {} parts (not a multiple of 3)",
            parts.len(),
        ));
    }
    let mut out: Vec<(String, T)> = Vec::with_capacity(parts.len() / 3);
    let mut i = 0;
    while i < parts.len() {
        let name = match &parts[i].value {
            ExpressionPart::Identifier(s) => s.clone(),
            other => {
                return Err(format!(
                    "{context} name must be a bare identifier, got {}",
                    other.summarize(),
                ));
            }
        };
        match &parts[i + 1].value {
            ExpressionPart::Keyword(k) if k == sep => {}
            other => {
                return Err(format!(
                    "{context} separator must be `{sep}`, got {}",
                    other.summarize(),
                ));
            }
        }
        if out.iter().any(|(n, _)| n == &name) {
            return Err(format!("duplicate name `{}` in {context}", name));
        }
        let third = parse_third(&parts[i + 2].value, &name)?;
        out.push((name, third));
        i += 3;
    }
    Ok(out)
}

/// `context` is woven into error messages. Empty `parts` yields an empty `Vec`.
pub fn parse_pair_list<'a, T>(
    expr: &KExpression<'a>,
    context: &str,
    mut parse_slot: impl FnMut(&ExpressionPart<'a>, &str) -> Result<T, String>,
) -> Result<Vec<(String, T)>, String> {
    let parts = &expr.parts;
    if !parts.len().is_multiple_of(2) {
        return Err(format!(
            "{context} must be `<name> <slot>` pairs; got {} parts (not a multiple of 2)",
            parts.len(),
        ));
    }
    let mut out: Vec<(String, T)> = Vec::with_capacity(parts.len() / 2);
    let mut i = 0;
    while i < parts.len() {
        let name = match &parts[i].value {
            ExpressionPart::Identifier(s) => s.clone(),
            other => {
                return Err(format!(
                    "{context} name must be a bare identifier, got {}",
                    other.summarize(),
                ));
            }
        };
        if out.iter().any(|(n, _)| n == &name) {
            return Err(format!("duplicate name `{}` in {context}", name));
        }
        let slot = parse_slot(&parts[i + 1].value, &name)?;
        out.push((name, slot));
        i += 2;
    }
    Ok(out)
}
