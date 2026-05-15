//! Token classification: turn each whitespace-delimited word into an `ExpressionPart`.
//! Recognizes literals (numbers, strings, booleans, `null`), classifies non-literal atoms
//! into keywords / types / identifiers, and desugars compound atoms — member access
//! (`a.b`), indexing (`a[i]`), and prefix negation — into nested `ExpressionPart`s using
//! the `operators` table. Consumed by `expression_tree::build_tree`.
//!
//! See [design/expressions-and-parsing.md](../../design/expressions-and-parsing.md).

use std::iter::Peekable;
use std::str::Chars;
use std::sync::LazyLock;

use regex::Regex;

use crate::runtime::machine::model::is_keyword_token;
use crate::runtime::machine::model::ast::{ExpressionPart, KLiteral, TypeExpr};
use crate::parse::operators::{find_prefix, find_suffix, is_atom_terminator, Operator, OperatorKind};

static FLOAT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[+-]?(\d+\.\d*|\.\d+|\d+)([eE][+-]?\d+)?$").unwrap()
});

/// Convert a single whitespace-delimited token into an `ExpressionPart`. First tries `try_literal`
/// on the whole token (so e.g. `3.14` stays a number rather than being parsed as `(attr 3 14)`);
/// otherwise hands off to `parse_compound` to desugar member access, indexing, and negation into
/// nested expressions.
pub fn classify_token<'a>(tok: String) -> Result<ExpressionPart<'a>, String> {
    if let Some(part) = try_literal(&tok) {
        return Ok(part);
    }
    let mut chars = tok.chars().peekable();
    let part = parse_compound(&mut chars)?;
    if let Some(&c) = chars.peek() {
        return Err(format!("unexpected {:?} in token {:?}", c, tok));
    }
    Ok(part)
}

/// Try to parse `tok` as a recognized literal (`null`, `true`, `false`, or a number matching
/// the `FLOAT` regex). Returns `None` if it isn't one. Shared by `classify_token` and
/// `classify_atom` so both apply the same literal rules.
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

/// Classify a sub-token (the piece between operators inside a compound token) per the token-class
/// rules in [design/type-system.md](../../design/type-system.md#token-classes--the-parser-level-foundation):
///
/// 1. Literal first (`null`, `true`, numbers).
/// 2. `Keyword` per `is_keyword_token` — pure-symbol or ≥2 uppercase letters with no lowercase.
/// 3. `Type` if first char ASCII-uppercase AND at least one lowercase char (`Number`, `Foo`,
///    `OrderedSig`, `KFunction`).
/// 4. Otherwise, if the token starts uppercase but fits neither rule (e.g. `A`, `AB1` — single
///    uppercase letter, or uppercase + digits with no lowercase), it's a `ParseError`.
/// 5. Otherwise `Identifier` (lowercase-leading or `_`-leading names).
///
/// Type and Identifier tokens are validated for character content: Types accept only ASCII
/// letters and digits; Identifiers also accept `_`. Anything else (e.g. `Number>`, `a@b`) is
/// rejected as a glue error so symbols can't sneak into a name. Keywords are exempt since
/// `=`, `->`, and `+` are legitimate keyword shapes.
fn classify_atom<'a>(tok: &str) -> Result<ExpressionPart<'a>, String> {
    if let Some(part) = try_literal(tok) {
        return Ok(part);
    }
    if is_keyword_token(tok) {
        return Ok(ExpressionPart::Keyword(tok.to_string()));
    }
    if is_type_name(tok) {
        if let Some(bad) = tok.chars().find(|c| !c.is_ascii_alphanumeric()) {
            return Err(format!(
                "type name `{tok}` contains invalid character {bad:?}; \
                 type names use only letters and digits",
            ));
        }
        return Ok(ExpressionPart::Type(TypeExpr::leaf(tok.to_string())));
    }
    // Capital-leading tokens that are neither a keyword nor a type are reserved syntactic
    // territory — module-system stage 1 declared the rule explicitly so a single-letter `A`
    // or a `K9` shape can't slip in as an Identifier and silently shadow a future
    // type-position binding.
    if tok.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
        return Err(format!(
            "token `{tok}` starts with an uppercase letter but classifies as neither a \
             keyword (needs ≥2 uppercase letters with no lowercase) nor a type name \
             (needs ≥1 lowercase letter)",
        ));
    }
    if let Some(bad) = tok.chars().find(|c| !c.is_ascii_alphanumeric() && *c != '_') {
        return Err(format!(
            "identifier `{tok}` contains invalid character {bad:?}; \
             identifiers use letters, digits, and `_`",
        ));
    }
    Ok(ExpressionPart::Identifier(tok.to_string()))
}

/// True iff `tok` looks like a type name: first char ASCII-uppercase plus at least one
/// ASCII-lowercase elsewhere. Admits `Number`, `Point`, `MyType`, `Point3D`, `KFunction`;
/// rejects all-caps tokens (caught earlier by `is_keyword_token`) and lowercase-leading
/// tokens (fall through to `Identifier`).
fn is_type_name(tok: &str) -> bool {
    let mut chars = tok.chars();
    let Some(first) = chars.next() else { return false; };
    if !first.is_ascii_uppercase() {
        return false;
    }
    chars.any(|c| c.is_ascii_lowercase())
}

/// Recursive-descent parser for compound tokens. Strips leading prefix operators, reads an
/// atom, then folds in any infix/postfix suffix operators. Each matched operator's builder
/// constructs the resulting expression — the dispatcher knows operand arity and source per
/// kind, the builder knows the output shape per operator.
fn parse_compound<'a>(chars: &mut Peekable<Chars>) -> Result<ExpressionPart<'a>, String> {
    let mut prefixes: Vec<&Operator> = Vec::new();
    while let Some(&c) = chars.peek() {
        let Some(op) = find_prefix(c) else { break };
        chars.next();
        prefixes.push(op);
    }

    let mut expr = read_atom(chars)?;

    while let Some(&c) = chars.peek() {
        let Some(op) = find_suffix(c) else { break };
        chars.next();
        expr = match op.kind {
            OperatorKind::Infix => {
                let rhs = read_atom(chars)?;
                (op.build)(vec![expr, rhs])
            }
            OperatorKind::Suffix => (op.build)(vec![expr]),
            OperatorKind::Prefix => unreachable!("find_suffix excludes Prefix"),
        };
    }

    for op in prefixes.into_iter().rev() {
        expr = (op.build)(vec![expr]);
    }
    Ok(expr)
}

/// Consume characters from `chars` until the next operator trigger or postfix close char
/// (driven by `OPERATORS`) and classify the run via `classify_atom`. Errors on an empty atom.
fn read_atom<'a>(chars: &mut Peekable<Chars>) -> Result<ExpressionPart<'a>, String> {
    let mut s = String::new();
    while let Some(&c) = chars.peek() {
        if is_atom_terminator(c) {
            break;
        }
        s.push(c);
        chars.next();
    }
    if s.is_empty() {
        return Err(format!("expected identifier, got {:?}", chars.peek()));
    }
    classify_atom(&s)
}

#[cfg(test)]
mod tests {
    use super::classify_token;
    use crate::runtime::machine::model::ast::{ExpressionPart, KLiteral};

    fn describe(p: &ExpressionPart<'_>) -> String {
        match p {
            ExpressionPart::Keyword(s) => format!("t({})", s),
            ExpressionPart::Identifier(s) => format!("t({})", s),
            ExpressionPart::Type(t) => format!("T({})", t.render()),
            ExpressionPart::Expression(e) => {
                let inner: Vec<String> = e.parts.iter().map(describe).collect();
                format!("[{}]", inner.join(" "))
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
        classify_token(tok.to_string()).map(|p| describe(&p))
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
        assert_eq!(
            classify("!!foo").unwrap(),
            "[t(NOT) [t(NOT) t(foo)]]"
        );
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

    // Token-classification tests.

    #[test]
    fn keyword_two_uppercase_no_lowercase() {
        // ≥2 uppercase letters with no lowercase qualifies as a Keyword.
        assert_eq!(classify("LET").unwrap(), "t(LET)");
        assert_eq!(classify("MODULE").unwrap(), "t(MODULE)");
        assert_eq!(classify("FN").unwrap(), "t(FN)");
    }

    #[test]
    fn type_uppercase_first_with_lowercase() {
        // First uppercase + ≥1 lowercase classifies as Type.
        assert_eq!(classify("Number").unwrap(), "T(Number)");
        assert_eq!(classify("OrderedSig").unwrap(), "T(OrderedSig)");
        assert_eq!(classify("KFunction").unwrap(), "T(KFunction)");
    }

    #[test]
    fn single_uppercase_letter_is_parse_error() {
        // `A`, `B`, `Z` etc. — neither keyword (only 1 uppercase) nor type (no lowercase).
        assert!(classify("A").is_err());
        assert!(classify("B").is_err());
        assert!(classify("Z").is_err());
    }

    #[test]
    fn uppercase_with_digits_no_lowercase_is_parse_error() {
        // `K9` — uppercase first, no lowercase. The token-classification rule rejects this rather than letting
        // it fall through to Identifier (which would silently shadow a future type-position
        // binding).
        assert!(classify("K9").is_err());
    }

    #[test]
    fn pure_symbol_token_is_keyword() {
        // Pure-symbol tokens (no letters at all) bypass the alphabetic-letter rule. The
        // ascription operators `:|` / `:!` (assembled at the build_tree layer because `:` and
        // `!` are themselves expression-tree-level delimiters) and the existing `=` / `->`
        // are all keywords.
        assert_eq!(classify("=").unwrap(), "t(=)");
        assert_eq!(classify("->").unwrap(), "t(->)");
    }

    #[test]
    fn ascription_compound_tokens_classify_as_keywords() {
        use crate::runtime::machine::model::is_keyword_token;
        assert!(is_keyword_token(":|"));
        assert!(is_keyword_token(":!"));
    }

    #[test]
    fn lowercase_leading_is_identifier() {
        assert_eq!(classify("foo").unwrap(), "t(foo)");
        assert_eq!(classify("my_var").unwrap(), "t(my_var)");
    }
}
