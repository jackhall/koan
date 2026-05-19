//! Shared `<tag> -> <body>` branch walker for `MATCH` and `TRY-WITH`.
//!
//! Both builtins take a parens-wrapped `KExpression` whose parts are repeated
//! `<Identifier(tag)> <Keyword("->")> <Expression(body)>` triples and pick the body whose
//! tag matches the dispatched value's tag. The walker is shape-only — it doesn't know what
//! the tags mean.
//!
//! `TRY` opts into wildcard `_` matching for hidden / dispatcher-internal error kinds;
//! `MATCH`'s exhaustiveness check is enforced by the caller, not here.

use crate::machine::model::ast::{ExpressionPart, KExpression, KLiteral};

/// Walk `branches` and return the body for the first triple whose tag matches `target_tag`,
/// or — when `allow_wildcard` is true and no exact match was found — the first `_` body.
/// Returns `Ok(None)` if no triple matches, `Err(msg)` on shape mismatch.
///
/// `allow_wildcard`: TRY passes `true`; MATCH passes `false`. The first exact-tag match
/// wins over a later `_`; a `_` placed before exact tags still loses to a same-tag triple
/// later in the body, since the exact-vs-wildcard preference is independent of position.
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
        let tag_name = match tag_part {
            ExpressionPart::Identifier(s) => s.clone(),
            // `true`/`false` are `KLiteral::Boolean` from the parser, not identifiers,
            // but they're the natural tag form for `MATCH` on a `Bool` value. Accept
            // them here so users can write `(true -> ... false -> ...)` directly.
            ExpressionPart::Literal(KLiteral::Boolean(b)) => {
                if *b { "true".to_string() } else { "false".to_string() }
            }
            // `_` is a pure-symbol token, which the lexer classifies as `Keyword` rather
            // than `Identifier` (see `is_keyword_token`). Accept it in the tag position
            // when wildcards are enabled so TRY's `_` arm can be spelled the natural way.
            ExpressionPart::Keyword(s) if allow_wildcard && s == "_" => s.clone(),
            other => {
                return Err(format!(
                    "branch tag must be a bare identifier or boolean literal, got {}",
                    other.summarize()
                ));
            }
        };
        match arrow_part {
            ExpressionPart::Keyword(k) if k == "->" => {}
            other => {
                return Err(format!(
                    "branch separator must be `->`, got {}",
                    other.summarize()
                ));
            }
        }
        let body_expr = match body_part {
            ExpressionPart::Expression(e) => (**e).clone(),
            other => {
                return Err(format!(
                    "branch body must be a parenthesized expression, got {}",
                    other.summarize()
                ));
            }
        };
        // Multi-statement branch desugar: `((s1) (s2) (s3))` becomes a CONS chain so
        // the branch dispatches as a single tail expression. Single-statement bodies
        // pass through unchanged. See [`super::cons`] for the contract.
        let body_expr = super::cons::fold_multi_statement(body_expr);
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
