//! Token classification: turn each whitespace-delimited word into a
//! `Spanned<ExpressionPart>`. Recognizes literals, classifies non-literal atoms into
//! keywords / types / identifiers, and desugars compound atoms (`a.b`, `a[i]`, `!a`)
//! into nested `ExpressionPart`s using the `operators` table.
//!
//! Synthetic operator keywords (ATTR / NOT / TRY) take 1-codepoint trigger spans;
//! mid-token errors attach the enclosing token's span so the message names the
//! offending char while the span pinpoints the token.
//!
//! See [design/expressions-and-parsing.md](../../design/expressions-and-parsing.md).

use std::iter::Peekable;
use std::str::CharIndices;
use std::sync::LazyLock;

use regex::Regex;

use crate::machine::core::source::{Span, Spanned};
use crate::machine::model::ast::{ExpressionPart, KLiteral, TypeExpr};
use crate::machine::model::is_keyword_token;
use crate::machine::KError;
use crate::parse::operators::{find_prefix, find_suffix, is_atom_terminator, SuffixOp, UnaryBuild};

static FLOAT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[+-]?(\d+\.\d*|\.\d+|\d+)([eE][+-]?\d+)?$").unwrap());

/// Whole-token literal match runs first so e.g. `3.14` stays a number rather than
/// being desugared as `(attr 3 14)`. `start` is the token's original-source byte
/// offset, used to compute absolute spans for atoms and operator triggers.
pub fn classify_token<'a>(tok: &str, start: u32) -> Result<Spanned<ExpressionPart<'a>>, KError> {
    let token_span = Span {
        start,
        end: start + tok.len() as u32,
    };
    if let Some(part) = try_literal(tok) {
        return Ok(Spanned::at(part, token_span));
    }
    let mut chars = tok.char_indices().peekable();
    let part = parse_compound(&mut chars, start, token_span)?;
    if let Some(&(_, c)) = chars.peek() {
        return Err(KError::parse(
            format!("unexpected {:?} in token {:?}", c, tok),
            Some(token_span),
        ));
    }
    Ok(part)
}

/// Shared between whole-token and sub-token classification so both apply the same
/// literal rules.
fn try_literal<'a>(tok: &str) -> Option<ExpressionPart<'a>> {
    match tok {
        "null" => return Some(ExpressionPart::Literal(KLiteral::Null)),
        "true" => return Some(ExpressionPart::Literal(KLiteral::Boolean(true))),
        "false" => return Some(ExpressionPart::Literal(KLiteral::Boolean(false))),
        _ => {}
    }
    if FLOAT.is_match(tok) {
        if let Ok(n) = tok.parse::<f64>() {
            return Some(ExpressionPart::Literal(KLiteral::Number(n)));
        }
    }
    None
}

/// Classify a sub-token per the token-class rules in
/// [design/typing/tokens.md](../../design/typing/tokens.md). Capital-leading tokens
/// that match neither the keyword nor the type shape are rejected rather than falling
/// through to Identifier, so a stray `A` or `K9` can't silently shadow a future
/// type-position binding. Types and Identifiers reject non-alphanumeric content so
/// glue like `Number>` or `a@b` errors instead of sneaking through; Keywords are
/// exempt because `=` / `->` / `+` are legitimate keyword shapes.
fn classify_atom<'a>(tok: &str, token_span: Span) -> Result<ExpressionPart<'a>, KError> {
    if let Some(part) = try_literal(tok) {
        return Ok(part);
    }
    if is_keyword_token(tok) {
        return Ok(ExpressionPart::Keyword(tok.to_string()));
    }
    if is_type_name(tok) {
        if let Some(bad) = tok.chars().find(|c| !c.is_ascii_alphanumeric()) {
            return Err(KError::parse(
                format!(
                    "type name `{tok}` contains invalid character {bad:?}; \
                     type names use only letters and digits",
                ),
                Some(token_span),
            ));
        }
        return Ok(ExpressionPart::Type(TypeExpr::leaf(tok.to_string())));
    }
    if tok.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
        return Err(KError::parse(
            format!(
                "token `{tok}` starts with an uppercase letter but classifies as neither a \
                 keyword (needs ≥2 uppercase letters with no lowercase) nor a type name \
                 (needs ≥1 lowercase letter)",
            ),
            Some(token_span),
        ));
    }
    if let Some(bad) = tok
        .chars()
        .find(|c| !c.is_ascii_alphanumeric() && *c != '_')
    {
        return Err(KError::parse(
            format!(
                "identifier `{tok}` contains invalid character {bad:?}; \
                 identifiers use letters, digits, and `_`",
            ),
            Some(token_span),
        ));
    }
    Ok(ExpressionPart::Identifier(tok.to_string()))
}

/// First char ASCII-uppercase plus at least one ASCII-lowercase elsewhere.
fn is_type_name(tok: &str) -> bool {
    let mut chars = tok.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_uppercase() {
        return false;
    }
    chars.any(|c| c.is_ascii_lowercase())
}

/// Recursive-descent parser for compound tokens. Each matched operator's builder owns
/// the output shape; the dispatcher just knows arity. Operator triggers take a
/// 1-codepoint span at their position so error messages can point at the trigger char.
fn parse_compound<'a>(
    chars: &mut Peekable<CharIndices>,
    start: u32,
    token_span: Span,
) -> Result<Spanned<ExpressionPart<'a>>, KError> {
    let mut prefixes: Vec<(UnaryBuild, Span)> = Vec::new();
    while let Some(&(ci, c)) = chars.peek() {
        let Some(build) = find_prefix(c) else { break };
        chars.next();
        let trigger = trigger_span(start, ci, c);
        prefixes.push((build, trigger));
    }

    let mut expr = read_atom(chars, start, token_span)?;

    while let Some(&(ci, c)) = chars.peek() {
        let Some(op) = find_suffix(c) else { break };
        chars.next();
        let trigger = trigger_span(start, ci, c);
        expr = match op {
            SuffixOp::Infix(build) => {
                let rhs = read_atom(chars, start, token_span)?;
                build(expr, rhs, trigger)
            }
            SuffixOp::Suffix(build) => build(expr, trigger),
        };
    }

    for (build, trigger) in prefixes.into_iter().rev() {
        expr = build(expr, trigger);
    }
    Ok(expr)
}

fn trigger_span(token_start: u32, ci: usize, c: char) -> Span {
    let start = token_start + ci as u32;
    Span {
        start,
        end: start + c.len_utf8() as u32,
    }
}

/// Errors on an empty atom — operators must have an atom between them.
fn read_atom<'a>(
    chars: &mut Peekable<CharIndices>,
    token_start: u32,
    token_span: Span,
) -> Result<Spanned<ExpressionPart<'a>>, KError> {
    let atom_start_ci = match chars.peek() {
        Some(&(ci, _)) => ci,
        None => {
            return Err(KError::parse(
                "expected identifier, got end of token",
                Some(token_span),
            ));
        }
    };
    let mut s = String::new();
    let mut end_ci = atom_start_ci;
    while let Some(&(ci, c)) = chars.peek() {
        if is_atom_terminator(c) {
            break;
        }
        s.push(c);
        chars.next();
        end_ci = ci + c.len_utf8();
    }
    if s.is_empty() {
        let next = chars.peek().map(|&(_, c)| c);
        return Err(KError::parse(
            format!("expected identifier, got {:?}", next),
            Some(token_span),
        ));
    }
    let span = Span {
        start: token_start + atom_start_ci as u32,
        end: token_start + end_ci as u32,
    };
    classify_atom(&s, token_span).map(|part| Spanned::at(part, span))
}

#[cfg(test)]
mod tests {
    use super::classify_token;
    use crate::machine::model::ast::{ExpressionPart, KLiteral};

    fn describe(p: &ExpressionPart<'_>) -> String {
        match p {
            ExpressionPart::Keyword(s) => format!("t({})", s),
            ExpressionPart::Identifier(s) => format!("t({})", s),
            ExpressionPart::Type(t) => format!("T({})", t.render()),
            ExpressionPart::Expression(e) => {
                let inner: Vec<String> = e.parts.iter().map(|p| describe(&p.value)).collect();
                format!("[{}]", inner.join(" "))
            }
            ExpressionPart::SigiledTypeExpr(e) => {
                let inner: Vec<String> = e.parts.iter().map(|p| describe(&p.value)).collect();
                format!(":({})", inner.join(" "))
            }
            ExpressionPart::Literal(KLiteral::String(s)) => format!("s({})", s),
            ExpressionPart::Literal(KLiteral::Number(n)) => format!("n({})", n),
            ExpressionPart::Literal(KLiteral::Boolean(b)) => format!("b({})", b),
            ExpressionPart::Literal(KLiteral::Null) => "null".to_string(),
            ExpressionPart::Future(_) => "future".to_string(),
            ExpressionPart::ListLiteral(items) => {
                let inner: Vec<String> = items.iter().map(describe).collect();
                format!("L[{}]", inner.join(" "))
            }
            ExpressionPart::DictLiteral(pairs) => {
                let inner: Vec<String> = pairs
                    .iter()
                    .map(|(k, v)| format!("{}: {}", describe(k), describe(v)))
                    .collect();
                format!("D{{{}}}", inner.join(", "))
            }
        }
    }

    fn classify(tok: &str) -> Result<String, String> {
        classify_token(tok, 0)
            .map(|s| describe(&s.value))
            .map_err(|e| e.to_string())
    }

    #[test]
    fn plain_identifier() {
        assert_eq!(classify("foo").unwrap(), "t(foo)");
    }

    #[test]
    fn plain_number() {
        assert_eq!(classify("42").unwrap(), "n(42)");
    }

    #[test]
    fn plain_boolean() {
        assert_eq!(classify("true").unwrap(), "b(true)");
    }

    #[test]
    fn plain_null() {
        assert_eq!(classify("null").unwrap(), "null");
    }

    #[test]
    fn attr_access() {
        assert_eq!(classify("foo.bar").unwrap(), "[t(ATTR) t(foo) t(bar)]");
    }

    #[test]
    fn chained_attr_access() {
        assert_eq!(
            classify("foo.bar.baz").unwrap(),
            "[t(ATTR) [t(ATTR) t(foo) t(bar)] t(baz)]"
        );
    }

    #[test]
    fn negation() {
        assert_eq!(classify("!foo").unwrap(), "[t(NOT) t(foo)]");
    }

    #[test]
    fn double_negation() {
        assert_eq!(classify("!!foo").unwrap(), "[t(NOT) [t(NOT) t(foo)]]");
    }

    #[test]
    fn negation_over_attr() {
        assert_eq!(
            classify("!foo.bar").unwrap(),
            "[t(NOT) [t(ATTR) t(foo) t(bar)]]"
        );
    }

    #[test]
    fn decimal_number_is_literal() {
        assert_eq!(classify("3.14").unwrap(), "n(3.14)");
    }

    #[test]
    fn scientific_number_is_literal() {
        assert_eq!(classify("1e3").unwrap(), "n(1000)");
        assert_eq!(classify("-2.5e-2").unwrap(), "n(-0.025)");
    }

    #[test]
    fn attr_wins_when_rhs_not_numeric() {
        assert_eq!(classify("3.foo").unwrap(), "[t(ATTR) n(3) t(foo)]");
    }

    #[test]
    fn dangling_dot_errors() {
        assert!(classify("foo.").is_err());
    }

    #[test]
    fn leading_dot_errors() {
        assert!(classify(".foo").is_err());
    }

    #[test]
    fn bare_bang_errors() {
        assert!(classify("!").is_err());
    }

    #[test]
    fn suffix_try() {
        assert_eq!(classify("foo?").unwrap(), "[t(TRY) t(foo)]");
    }

    #[test]
    fn chained_suffix() {
        assert_eq!(classify("foo??").unwrap(), "[t(TRY) [t(TRY) t(foo)]]");
    }

    #[test]
    fn suffix_after_attr() {
        assert_eq!(
            classify("foo.bar?").unwrap(),
            "[t(TRY) [t(ATTR) t(foo) t(bar)]]"
        );
    }

    #[test]
    fn negation_over_suffix() {
        assert_eq!(classify("!foo?").unwrap(), "[t(NOT) [t(TRY) t(foo)]]");
    }

    #[test]
    fn leading_suffix_errors() {
        assert!(classify("?foo").is_err());
    }

    #[test]
    fn keyword_two_uppercase_no_lowercase() {
        assert_eq!(classify("LET").unwrap(), "t(LET)");
        assert_eq!(classify("MODULE").unwrap(), "t(MODULE)");
        assert_eq!(classify("FN").unwrap(), "t(FN)");
    }

    #[test]
    fn type_uppercase_first_with_lowercase() {
        assert_eq!(classify("Number").unwrap(), "T(Number)");
        assert_eq!(classify("OrderedSig").unwrap(), "T(OrderedSig)");
        assert_eq!(classify("KFunction").unwrap(), "T(KFunction)");
    }

    #[test]
    fn single_uppercase_letter_is_parse_error() {
        assert!(classify("A").is_err());
        assert!(classify("B").is_err());
        assert!(classify("Z").is_err());
    }

    #[test]
    fn uppercase_with_digits_no_lowercase_is_parse_error() {
        assert!(classify("K9").is_err());
    }

    #[test]
    fn pure_symbol_token_is_keyword() {
        assert_eq!(classify("=").unwrap(), "t(=)");
        assert_eq!(classify("->").unwrap(), "t(->)");
    }

    #[test]
    fn ascription_compound_tokens_classify_as_keywords() {
        use crate::machine::model::is_keyword_token;
        assert!(is_keyword_token(":|"));
        assert!(is_keyword_token(":!"));
    }

    #[test]
    fn lowercase_leading_is_identifier() {
        assert_eq!(classify("foo").unwrap(), "t(foo)");
        assert_eq!(classify("my_var").unwrap(), "t(my_var)");
    }
}
