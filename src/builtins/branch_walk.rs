//! Shared `<tag> -> <body>` branch walker for `MATCH` and `TRY-WITH`. Shape-only: picks the
//! body whose tag matches a dispatched value's tag without knowing what tags mean.
//!
//! `TRY` opts into wildcard `_` matching for dispatcher-internal error kinds; `MATCH`'s
//! exhaustiveness check is enforced by the caller.

use crate::machine::model::ast::{ExpressionPart, KExpression, KLiteral};

/// Returns the body for the first triple whose tag matches `target_tag`, or — when
/// `allow_wildcard` is true and no exact match was found — the first `_` body. Exact-tag
/// matches always win over `_`, regardless of source order.
pub(crate) fn find_branch_body<'a>(
    branches: &KExpression<'a>,
    target_tag: &str,
    allow_wildcard: bool,
) -> Result<Option<KExpression<'a>>, String> {
    let parts = &branches.parts;
    if !parts.len().is_multiple_of(3) {
        return Err(format!(
            "branches must be `<tag> -> <body>` triples; got {} parts (not a multiple of 3)",
            parts.len()
        ));
    }
    let mut wildcard_body: Option<KExpression<'a>> = None;
    let mut i = 0;
    while i < parts.len() {
        let tag_part = &parts[i];
        let arrow_part = &parts[i + 1];
        let body_part = &parts[i + 2];
        let tag_name = match &tag_part.value {
            ExpressionPart::Identifier(s) => s.clone(),
            // Booleans parse as `KLiteral::Boolean`, not identifiers; accept them so
            // `MATCH` on a `Bool` can spell its arms `true ->` / `false ->`.
            ExpressionPart::Literal(KLiteral::Boolean(b)) => {
                if *b {
                    "true".to_string()
                } else {
                    "false".to_string()
                }
            }
            // `_` is a pure-symbol token classified as `Keyword`, not `Identifier`.
            ExpressionPart::Keyword(s) if allow_wildcard && s == "_" => s.clone(),
            other => {
                return Err(format!(
                    "branch tag must be a bare identifier or boolean literal, got {}",
                    other.summarize()
                ));
            }
        };
        match &arrow_part.value {
            ExpressionPart::Keyword(k) if k == "->" => {}
            other => {
                return Err(format!(
                    "branch separator must be `->`, got {}",
                    other.summarize()
                ));
            }
        }
        let body_expr = match &body_part.value {
            ExpressionPart::Expression(e) => (**e).clone(),
            other => {
                return Err(format!(
                    "branch body must be a parenthesized expression, got {}",
                    other.summarize()
                ));
            }
        };
        if tag_name == target_tag {
            return Ok(Some(body_expr));
        }
        if allow_wildcard && tag_name == "_" && wildcard_body.is_none() {
            wildcard_body = Some(body_expr);
        }
        i += 3;
    }
    Ok(wildcard_body)
}
