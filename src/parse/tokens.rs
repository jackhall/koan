//! Token classification: turn each whitespace-delimited word into a
//! `Spanned<ExpressionPart>`. Recognizes literals, classifies non-literal atoms into
//! keywords / types / identifiers, and desugars compound atoms (`a.b`, `a?`)
//! into nested `ExpressionPart`s using the `operators` table.
//!
//! A pure-symbol token that is not a builtin compound trigger (`+`, `|`, `<=`, `==`, `!=`) reaches
//! `classify_atom` and tags as a `Keyword`, so a post-parse chain detector recognizes
//! user operators. Builtin triggers (`.`/`?`) keep their compound desugaring.
//!
//! Synthetic operator keywords (ATTR / TRY) take 1-codepoint trigger spans;
//! mid-token errors attach the enclosing token's span so the message names the
//! offending char while the span pinpoints the token.
//!
//! See [design/expressions-and-parsing.md](../../design/expressions-and-parsing.md).

use std::iter::Peekable;
use std::str::CharIndices;

use crate::machine::KError;
use crate::machine::core::ProgramBrand;
use crate::machine::model::ast::{ExpressionPart, KLiteral};
use crate::machine::model::labels::{KeywordSymbol, LabelInterner, TypeSymbol, ValueSymbol};
use crate::machine::model::{is_keyword_token, is_type_name};
use crate::parse::operators::{SuffixOp, find_suffix, is_atom_terminator};
use crate::source::{Span, Spanned};

/// Whole-token literal match runs first so e.g. `3.14` stays a number rather than
/// being desugared as `(attr 3 14)`. `start` is the token's original-source byte
/// offset, used to compute absolute spans for atoms and operator triggers. Every name the
/// classification keeps is bumped into `brand`'s program storage, so the part borrows nothing from
/// `tok`.
pub fn classify_token<'a>(
    brand: ProgramBrand<'a>,
    labels: &LabelInterner,
    tok: &str,
    start: u32,
) -> Result<Spanned<ExpressionPart<'a>>, KError> {
    let token_span = Span {
        start,
        end: start + tok.len() as u32,
    };
    if let Some(part) = try_literal(tok) {
        return Ok(Spanned::at(part, token_span));
    }
    let mut chars = tok.char_indices().peekable();
    let part = parse_compound(brand, labels, tok, &mut chars, start, token_span)?;
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
    if is_number_shape(tok)
        && let Ok(n) = tok.parse::<f64>()
    {
        return Some(ExpressionPart::Literal(KLiteral::Number(n)));
    }
    None
}

/// Whether `tok` has koan's decimal-number shape: an optional sign, digits with an
/// optional decimal point carrying at least one digit on some side, and an optional
/// exponent. Guards `parse::<f64>`, whose own grammar additionally accepts `inf`,
/// `infinity`, and `NaN` — spellings koan classifies as identifiers instead. Digits are
/// ASCII only, so a token written with non-ASCII digits falls through to `classify_atom`.
fn is_number_shape(tok: &str) -> bool {
    let bytes = tok.as_bytes();
    let mut at = 0;
    if matches!(bytes.first(), Some(b'+' | b'-')) {
        at = 1;
    }
    let integral = take_digits(bytes, &mut at);
    let mut fractional = 0;
    if bytes.get(at) == Some(&b'.') {
        at += 1;
        fractional = take_digits(bytes, &mut at);
    }
    if integral == 0 && fractional == 0 {
        return false;
    }
    if matches!(bytes.get(at), Some(b'e' | b'E')) {
        at += 1;
        if matches!(bytes.get(at), Some(b'+' | b'-')) {
            at += 1;
        }
        if take_digits(bytes, &mut at) == 0 {
            return false;
        }
    }
    at == bytes.len()
}

/// Advance `at` past a run of ASCII digits, reporting how many it consumed.
fn take_digits(bytes: &[u8], at: &mut usize) -> usize {
    let start = *at;
    while bytes.get(*at).is_some_and(u8::is_ascii_digit) {
        *at += 1;
    }
    *at - start
}

/// Classify a sub-token per the token-class rules in
/// [design/typing/tokens.md](../../design/typing/tokens.md). Capital-leading tokens
/// that match neither the keyword nor the type shape are rejected rather than falling
/// through to Identifier, so a stray `A` or `K9` can't silently shadow a future
/// type-position binding. Types and Identifiers reject non-alphanumeric content so
/// glue like `Number>` or `a@b` errors instead of sneaking through; Keywords are
/// exempt because `=` / `->` / `+` are legitimate keyword shapes.
fn classify_atom<'a>(
    labels: &LabelInterner,
    tok: &str,
    token_span: Span,
) -> Result<ExpressionPart<'a>, KError> {
    if let Some(part) = try_literal(tok) {
        return Ok(part);
    }
    if is_keyword_token(tok) {
        return Ok(ExpressionPart::Keyword(
            KeywordSymbol::declared(tok, labels)
                .expect("is_keyword_token just classified this token as keyword-class"),
        ));
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
        return Ok(ExpressionPart::Type(
            TypeSymbol::declared(tok, labels)
                .expect("is_type_name just classified this token as type-class"),
        ));
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
    Ok(ExpressionPart::Identifier(
        ValueSymbol::declared(tok, labels)
            .expect("a token that is neither keyword-class nor a type name is a value token"),
    ))
}

/// Recursive-descent parser for compound tokens. Each matched operator's builder owns
/// the output shape; the dispatcher just knows arity. Operator triggers take a
/// 1-codepoint span at their position so error messages can point at the trigger char.
fn parse_compound<'a>(
    brand: ProgramBrand<'a>,
    labels: &LabelInterner,
    tok: &str,
    chars: &mut Peekable<CharIndices>,
    start: u32,
    token_span: Span,
) -> Result<Spanned<ExpressionPart<'a>>, KError> {
    let mut expr = read_atom(labels, tok, chars, start, token_span)?;

    while let Some(&(ci, c)) = chars.peek() {
        let Some(op) = find_suffix(c) else { break };
        chars.next();
        let trigger = trigger_span(start, ci, c);
        expr = match op {
            SuffixOp::Infix(build) => {
                let rhs = read_atom(labels, tok, chars, start, token_span)?;
                build(brand, labels, expr, rhs, trigger)
            }
            SuffixOp::Suffix(build) => build(brand, labels, expr, trigger),
        };
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

/// Errors on an empty atom — operators must have an atom between them. The atom is a
/// verbatim contiguous run of `tok`, so classification borrows the slice between the
/// offsets the terminator walk computes; nothing is copied.
fn read_atom<'a>(
    labels: &LabelInterner,
    tok: &str,
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
    let mut end_ci = atom_start_ci;
    while let Some(&(ci, c)) = chars.peek() {
        if is_atom_terminator(c) {
            break;
        }
        chars.next();
        end_ci = ci + c.len_utf8();
    }
    let atom = &tok[atom_start_ci..end_ci];
    if atom.is_empty() {
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
    classify_atom(labels, atom, token_span).map(|part| Spanned::at(part, span))
}

#[cfg(test)]
mod tests {
    use super::classify_token;
    use crate::machine::core::program_storage;
    use crate::machine::model::ast::{ExpressionPart, KLiteral};
    use crate::machine::model::labels::LabelInterner;

    fn describe(p: &ExpressionPart<'_>, labels: &LabelInterner) -> String {
        match p {
            ExpressionPart::Keyword(symbol) => format!("t({})", labels.render(symbol.symbol())),
            ExpressionPart::Identifier(v) => format!("t({})", labels.render(v.symbol())),
            ExpressionPart::Type(t) => format!("T({})", labels.render(t.symbol())),
            ExpressionPart::Expression(e) => {
                let inner: Vec<String> =
                    e.parts.iter().map(|p| describe(&p.value, labels)).collect();
                format!("[{}]", inner.join(" "))
            }
            ExpressionPart::SigiledTypeExpr(e) => {
                let inner: Vec<String> =
                    e.parts.iter().map(|p| describe(&p.value, labels)).collect();
                format!(":({})", inner.join(" "))
            }
            ExpressionPart::RecordType(e) => {
                let inner: Vec<String> =
                    e.parts.iter().map(|p| describe(&p.value, labels)).collect();
                format!(":{{{}}}", inner.join(" "))
            }
            ExpressionPart::QuotedExpression(e) => {
                let inner: Vec<String> =
                    e.parts.iter().map(|p| describe(&p.value, labels)).collect();
                format!("#({})", inner.join(" "))
            }
            ExpressionPart::Literal(KLiteral::String(s)) => format!("s({})", s),
            ExpressionPart::Literal(KLiteral::Number(n)) => format!("n({})", n),
            ExpressionPart::Literal(KLiteral::Boolean(b)) => format!("b({})", b),
            ExpressionPart::Literal(KLiteral::Null) => "null".to_string(),
            ExpressionPart::ListLiteral(items) => {
                let inner: Vec<String> = items.iter().map(|p| describe(p, labels)).collect();
                format!("L[{}]", inner.join(" "))
            }
            ExpressionPart::DictLiteral(pairs) => {
                let inner: Vec<String> = pairs
                    .iter()
                    .map(|(k, v)| format!("{}: {}", describe(k, labels), describe(v, labels)))
                    .collect();
                format!("D{{{}}}", inner.join(", "))
            }
            ExpressionPart::RecordLiteral(fields) => {
                let inner: Vec<String> = fields
                    .iter()
                    .map(|(name, v)| {
                        format!("{} = {}", labels.render(name.symbol()), describe(v, labels))
                    })
                    .collect();
                format!("R{{{}}}", inner.join(", "))
            }
        }
    }

    fn classify(tok: &str) -> Result<String, String> {
        let program = program_storage();
        let labels = LabelInterner::new();
        classify_token(program.brand(), &labels, tok, 0)
            .map(|s| describe(&s.value, &labels))
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
    fn bang_prefix_does_not_desugar() {
        // `!` is not a compound trigger, so `!foo` is an invalid-character identifier, not `NOT foo`.
        assert!(classify("!foo").is_err());
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
    fn bang_and_bang_equals_are_keywords() {
        // `!` is not a compound trigger, so pure-symbol tokens beginning with it classify as
        // operator keywords: `!=` is the not-equal operator, and a bare `!` is an operator keyword
        // (unbound, so it fails at dispatch, not at tokenization).
        assert_eq!(classify("!=").unwrap(), "t(!=)");
        assert_eq!(classify("!").unwrap(), "t(!)");
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
        assert_eq!(classify("Ordered").unwrap(), "T(Ordered)");
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
    fn operator_tokens_classify_as_keywords() {
        // A whitespace-delimited operator token (single- or multi-char) that is not
        // a builtin compound trigger reaches `classify_atom` and tags as a keyword,
        // so a post-parse chain detector can recognize it.
        assert_eq!(classify("+").unwrap(), "t(+)");
        assert_eq!(classify("|").unwrap(), "t(|)");
        assert_eq!(classify("-").unwrap(), "t(-)");
        assert_eq!(classify("*").unwrap(), "t(*)");
        assert_eq!(classify("<=").unwrap(), "t(<=)");
        assert_eq!(classify(">>").unwrap(), "t(>>)");
    }

    #[test]
    fn attr_trigger_stays_on_builtin_path_inside_operand() {
        // `b.c` is one whitespace-delimited token; the `.` builtin trigger desugars it
        // to an ATTR compound so an enclosing `a + b.c` sees `b.c` as one operand rather
        // than splitting on `.`.
        assert_eq!(classify("b.c").unwrap(), "[t(ATTR) t(b) t(c)]");
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
