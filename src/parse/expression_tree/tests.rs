//! Tests for `expression_tree::parse`.
//!
//! Each test parses a source snippet and compares the result against an expected
//! shape string produced by the local `describe` helper, which renders an
//! `ExpressionPart` tree as compact `t(...)` / `T(...)` notation.

mod basics;
mod interning;
mod list_dict;
mod literals;
mod probes;
mod spans;
mod type_sigil;
mod value_sigil;

use super::{build_tree, parse};
use crate::machine::core::program_storage;
use crate::machine::model::ast::{ExpressionPart, KExpression, KLiteral};
use crate::machine::model::labels::LabelInterner;
use crate::parse::quotes::mask_quotes;

pub(super) fn describe(e: &KExpression<'_>, labels: &LabelInterner) -> String {
    fn describe_part(p: &ExpressionPart<'_>, labels: &LabelInterner) -> String {
        match p {
            ExpressionPart::Keyword(s) => format!("t({})", labels.render(s.symbol())),
            ExpressionPart::Identifier(v) => format!("t({})", labels.render(v.symbol())),
            ExpressionPart::Type(t) => format!("T({})", labels.render(t.symbol())),
            ExpressionPart::Expression(e) => describe(e, labels),
            // Slice (not trim) to strip exactly one wrapping `[…]` — trim_matches is greedy.
            ExpressionPart::SigiledTypeExpr(e) => {
                let inner = describe(e, labels);
                let stripped = inner
                    .strip_prefix('[')
                    .and_then(|s| s.strip_suffix(']'))
                    .unwrap_or(&inner);
                format!(":({stripped})")
            }
            ExpressionPart::RecordType(e) => {
                let inner = describe(e, labels);
                let stripped = inner
                    .strip_prefix('[')
                    .and_then(|s| s.strip_suffix(']'))
                    .unwrap_or(&inner);
                format!(":{{{stripped}}}")
            }
            // The quoted body renders as a nested expression (`#[...]`), so the wrapper the
            // parse-static capture holds is visible in every shape assertion.
            ExpressionPart::QuotedExpression(e) => format!("#{}", describe(e, labels)),
            ExpressionPart::ListLiteral(items) => {
                let inner: Vec<String> = items.iter().map(|p| describe_part(p, labels)).collect();
                format!("L[{}]", inner.join(" "))
            }
            ExpressionPart::DictLiteral(pairs) => {
                let inner: Vec<String> = pairs
                    .iter()
                    .map(|(k, v)| {
                        format!("{}: {}", describe_part(k, labels), describe_part(v, labels))
                    })
                    .collect();
                format!("D{{{}}}", inner.join(", "))
            }
            ExpressionPart::RecordLiteral(fields) => {
                let inner: Vec<String> = fields
                    .iter()
                    .map(|(name, v)| format!("{} = {}", name, describe_part(v, labels)))
                    .collect();
                format!("R{{{}}}", inner.join(", "))
            }
            ExpressionPart::Literal(KLiteral::String(s)) => format!("s({})", s),
            ExpressionPart::Literal(KLiteral::Number(n)) => format!("n({})", n),
            ExpressionPart::Literal(KLiteral::Boolean(b)) => format!("b({})", b),
            ExpressionPart::Literal(KLiteral::Null) => "null".to_string(),
        }
    }
    let parts: Vec<String> = e
        .parts
        .iter()
        .map(|p| describe_part(&p.value, labels))
        .collect();
    format!("[{}]", parts.join(" "))
}

pub(super) fn tree(input: &str) -> Result<String, String> {
    let program = program_storage();
    let (masked, dict) = mask_quotes(input);
    let labels = LabelInterner::new();
    build_tree(program.brand(), &labels, &masked, &dict)
        .map(|e| describe(&e, &labels))
        .map_err(|e| e.to_string())
}

pub(super) fn top(input: &str) -> Result<Vec<String>, String> {
    let program = program_storage();
    let labels = LabelInterner::new();
    parse(program.brand(), &labels, input)
        .map(|exprs| exprs.iter().map(|e| describe(e, &labels)).collect())
        .map_err(|e| e.to_string())
}
