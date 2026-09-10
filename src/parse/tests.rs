//! Tests for `parse`.
//!
//! Each test parses a source snippet and compares the result against an expected
//! shape string produced by the local `describe` helper, which renders an
//! `ExpressionPart` tree as compact `t(...)` / `T(...)` notation.

mod basics;
mod interning;
mod layout;
mod list_dict;
mod literals;
mod probes;
mod spans;
mod type_sigil;
mod value_sigil;

use super::lower::lower_run_for_tests;
use super::parse;
use crate::memory::program_storage;
use crate::parse::labels::LabelInterner;
use crate::parse::{ExpressionPart, KExpression, KLiteral};

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
                    .map(|(name, v)| {
                        format!(
                            "{} = {}",
                            labels.render(name.symbol()),
                            describe_part(v, labels)
                        )
                    })
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

/// One line's parts, without the peel a statement gets: the run as written, so an expectation
/// here names the parts a paren or sigil produced rather than what a redundant wrapper collapses
/// to. Rejects an input that is not exactly one line.
pub(super) fn tree(input: &str) -> Result<String, String> {
    let program = program_storage();
    let labels = LabelInterner::new();
    lower_run_for_tests(program.brand(), &labels, input)
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
