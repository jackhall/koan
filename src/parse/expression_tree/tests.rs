//! Tests for `expression_tree::parse`.
//!
//! Each test parses a source snippet and compares the result against an expected
//! shape string produced by the local `describe` helper, which renders an
//! `ExpressionPart` tree as compact `t(...)` / `T(...)` notation.

mod basics;
mod list_dict;
mod literals;
mod spans;
mod type_sigil;
mod value_sigil;

use super::{build_tree, parse};
use crate::machine::model::ast::{ExpressionPart, KExpression, KLiteral};
use crate::parse::quotes::mask_quotes;

pub(super) fn describe(e: &KExpression<'_>) -> String {
    fn describe_part(p: &ExpressionPart<'_>) -> String {
        match p {
            ExpressionPart::Keyword(s) => format!("t({})", s),
            ExpressionPart::Identifier(s) => format!("t({})", s),
            ExpressionPart::Type(t) => format!("T({})", t.render()),
            ExpressionPart::Expression(e) => describe(e),
            // Slice (not trim) to strip exactly one wrapping `[…]` — trim_matches is greedy.
            ExpressionPart::SigiledTypeExpr(e) => {
                let inner = describe(e);
                let stripped = inner
                    .strip_prefix('[')
                    .and_then(|s| s.strip_suffix(']'))
                    .unwrap_or(&inner);
                format!(":({stripped})")
            }
            ExpressionPart::RecordType(e) => {
                let inner = describe(e);
                let stripped = inner
                    .strip_prefix('[')
                    .and_then(|s| s.strip_suffix(']'))
                    .unwrap_or(&inner);
                format!(":{{{stripped}}}")
            }
            ExpressionPart::ListLiteral(items) => {
                let inner: Vec<String> = items.iter().map(describe_part).collect();
                format!("L[{}]", inner.join(" "))
            }
            ExpressionPart::DictLiteral(pairs) => {
                let inner: Vec<String> = pairs
                    .iter()
                    .map(|(k, v)| format!("{}: {}", describe_part(k), describe_part(v)))
                    .collect();
                format!("D{{{}}}", inner.join(", "))
            }
            ExpressionPart::RecordLiteral(fields) => {
                let inner: Vec<String> = fields
                    .iter()
                    .map(|(name, v)| format!("{} = {}", name, describe_part(v)))
                    .collect();
                format!("R{{{}}}", inner.join(", "))
            }
            ExpressionPart::Literal(KLiteral::String(s)) => format!("s({})", s),
            ExpressionPart::Literal(KLiteral::Number(n)) => format!("n({})", n),
            ExpressionPart::Literal(KLiteral::Boolean(b)) => format!("b({})", b),
            ExpressionPart::Literal(KLiteral::Null) => "null".to_string(),
            ExpressionPart::Future(_) => "future".to_string(),
        }
    }
    let parts: Vec<String> = e.parts.iter().map(|p| describe_part(&p.value)).collect();
    format!("[{}]", parts.join(" "))
}

pub(super) fn tree(input: &str) -> Result<String, String> {
    let (masked, dict) = mask_quotes(input);
    build_tree(&masked, &dict)
        .map(|e| describe(&e))
        .map_err(|e| e.to_string())
}

pub(super) fn top(input: &str) -> Result<Vec<String>, String> {
    parse(input)
        .map(|exprs| exprs.iter().map(describe).collect())
        .map_err(|e| e.to_string())
}
