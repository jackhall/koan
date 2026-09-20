//! Tests for `parse`.
//!
//! Each test parses a source snippet and compares the result against an expected
//! shape string produced by the local `describe` helper, which renders an
//! `ExpressionPart` tree as compact `t(...)` / `T(...)` notation.
//!
//! [`properties`] states the parser's laws over random trees rendered under random layouts, in
//! that same notation. The files beside it hold what a law does not state: the diagnostic a
//! mistake reports, and the surface rules a renderer never writes.

mod basics;
mod layout;
mod list_dict;
mod literals;
mod properties;
mod spans;
mod type_sigil;
mod value_sigil;

use super::lower::lower_run_for_tests;
use super::parse;
use crate::memory::program_storage;
use crate::parse::{ExpressionPart, KExpression, KLiteral};
use crate::symbols::SymbolInterner;

pub(super) fn describe(e: &KExpression<'_>, symbols: &SymbolInterner) -> String {
    fn describe_part(p: &ExpressionPart<'_>, symbols: &SymbolInterner) -> String {
        match p {
            ExpressionPart::Keyword(s) => format!("t({})", symbols.render(s.symbol())),
            ExpressionPart::Identifier(v) => format!("t({})", symbols.render(v.symbol())),
            ExpressionPart::Type(t) => format!("T({})", symbols.render(t.symbol())),
            ExpressionPart::Expression(e) => describe(e, symbols),
            // Slice (not trim) to strip exactly one wrapping `[…]` — trim_matches is greedy.
            ExpressionPart::SigiledTypeExpr(e) => {
                let inner = describe(e, symbols);
                let stripped = inner
                    .strip_prefix('[')
                    .and_then(|s| s.strip_suffix(']'))
                    .unwrap_or(&inner);
                format!(":({stripped})")
            }
            ExpressionPart::RecordType(e) => {
                let inner = describe(e, symbols);
                let stripped = inner
                    .strip_prefix('[')
                    .and_then(|s| s.strip_suffix(']'))
                    .unwrap_or(&inner);
                format!(":{{{stripped}}}")
            }
            // The quoted body renders as a nested expression (`#[...]`), so the wrapper the
            // parse-static capture holds is visible in every shape assertion.
            ExpressionPart::QuotedExpression(e) => format!("#{}", describe(e, symbols)),
            ExpressionPart::ListLiteral(items) => {
                let inner: Vec<String> = items.iter().map(|p| describe_part(p, symbols)).collect();
                format!("L[{}]", inner.join(" "))
            }
            ExpressionPart::DictLiteral(pairs) => {
                let inner: Vec<String> = pairs
                    .iter()
                    .map(|(k, v)| {
                        format!(
                            "{}: {}",
                            describe_part(k, symbols),
                            describe_part(v, symbols)
                        )
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
                            symbols.render(name.symbol()),
                            describe_part(v, symbols)
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
        .map(|p| describe_part(&p.value, symbols))
        .collect();
    format!("[{}]", parts.join(" "))
}

/// One line's parts, without the peel a statement gets: the run as written, so an expectation
/// here names the parts a paren or sigil produced rather than what a redundant wrapper collapses
/// to. Rejects an input that is not exactly one line.
pub(super) fn tree(input: &str) -> Result<String, String> {
    let program = program_storage();
    let symbols = SymbolInterner::new();
    lower_run_for_tests(program.brand(), &symbols, input)
        .map(|e| describe(&e, &symbols))
        .map_err(|e| e.to_string())
}

pub(super) fn top(input: &str) -> Result<Vec<String>, String> {
    let program = program_storage();
    let symbols = SymbolInterner::new();
    parse(program.brand(), &symbols, input)
        .map(|exprs| exprs.iter().map(|e| describe(e, &symbols)).collect())
        .map_err(|e| e.to_string())
}
