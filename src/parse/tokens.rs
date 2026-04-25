use std::iter::Peekable;
use std::str::Chars;
use std::sync::LazyLock;

use regex::Regex;

use crate::parse::kexpression::{ExpressionPart, KLiteral};
use crate::parse::operators::{find_prefix, find_suffix, is_atom_terminator, Operator, OperatorKind};

static FLOAT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[+-]?(\d+\.\d*|\.\d+|\d+)([eE][+-]?\d+)?$").unwrap()
});

/// Convert a single whitespace-delimited token into an `ExpressionPart`. First tries `try_literal`
/// on the whole token (so e.g. `3.14` stays a number rather than being parsed as `(attr 3 14)`);
/// otherwise hands off to `parse_compound` to desugar member access, indexing, and negation into
/// nested expressions.
pub fn classify_token(tok: String) -> Result<ExpressionPart, String> {
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
fn try_literal(tok: &str) -> Option<ExpressionPart> {
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

/// Classify a sub-token (the piece between operators inside a compound token): literal if
/// possible, otherwise a `Token`. Used by `read_atom`.
fn classify_atom(tok: &str) -> ExpressionPart {
    try_literal(tok).unwrap_or_else(|| ExpressionPart::Token(tok.to_string()))
}

/// Recursive-descent parser for compound tokens. Strips leading prefix operators, reads an
/// atom, then folds in any infix/postfix suffix operators. Each matched operator's builder
/// constructs the resulting expression — the dispatcher knows operand arity and source per
/// kind, the builder knows the output shape per operator.
fn parse_compound(chars: &mut Peekable<Chars>) -> Result<ExpressionPart, String> {
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
            OperatorKind::Postfix { close } => {
                let inner = parse_compound(chars)?;
                if chars.next() != Some(close) {
                    return Err(format!("unclosed {}", op.trigger));
                }
                (op.build)(vec![expr, inner])
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
fn read_atom(chars: &mut Peekable<Chars>) -> Result<ExpressionPart, String> {
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
    Ok(classify_atom(&s))
}

#[cfg(test)]
mod tests {
    use super::classify_token;
    use crate::parse::kexpression::{ExpressionPart, KLiteral};

    fn describe(p: &ExpressionPart) -> String {
        match p {
            ExpressionPart::Token(s) => format!("t({})", s),
            ExpressionPart::Expression(e) => {
                let inner: Vec<String> = e.parts.iter().map(describe).collect();
                format!("[{}]", inner.join(" "))
            }
            ExpressionPart::Literal(KLiteral::String(s)) => format!("s({})", s),
            ExpressionPart::Literal(KLiteral::Number(n)) => format!("n({})", n),
            ExpressionPart::Literal(KLiteral::Boolean(b)) => format!("b({})", b),
            ExpressionPart::Literal(KLiteral::Null) => "null".to_string(),
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
        assert_eq!(classify("foo.bar").unwrap(), "[t(attr) t(foo) t(bar)]");
    }

    #[test]
    fn chained_attr_access() {
        assert_eq!(
            classify("foo.bar.baz").unwrap(),
            "[t(attr) [t(attr) t(foo) t(bar)] t(baz)]"
        );
    }

    #[test]
    fn index_access() {
        assert_eq!(classify("foo[2]").unwrap(), "[t(foo) t(at) n(2)]");
    }

    #[test]
    fn chained_index_access() {
        assert_eq!(
            classify("foo[2][3]").unwrap(),
            "[[t(foo) t(at) n(2)] t(at) n(3)]"
        );
    }

    #[test]
    fn negation() {
        assert_eq!(classify("!foo").unwrap(), "[t(not) t(foo)]");
    }

    #[test]
    fn double_negation() {
        assert_eq!(
            classify("!!foo").unwrap(),
            "[t(not) [t(not) t(foo)]]"
        );
    }

    #[test]
    fn negation_over_attr() {
        assert_eq!(
            classify("!foo.bar").unwrap(),
            "[t(not) [t(attr) t(foo) t(bar)]]"
        );
    }

    #[test]
    fn attr_then_index() {
        assert_eq!(
            classify("foo.bar[2]").unwrap(),
            "[[t(attr) t(foo) t(bar)] t(at) n(2)]"
        );
    }

    #[test]
    fn index_contains_attr() {
        assert_eq!(
            classify("foo[bar.baz]").unwrap(),
            "[t(foo) t(at) [t(attr) t(bar) t(baz)]]"
        );
    }

    #[test]
    fn nested_indexing() {
        assert_eq!(
            classify("foo[bar[2]]").unwrap(),
            "[t(foo) t(at) [t(bar) t(at) n(2)]]"
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
        assert_eq!(classify("3.foo").unwrap(), "[t(attr) n(3) t(foo)]");
    }

    #[test]
    fn unclosed_bracket_errors() {
        assert!(classify("foo[2").is_err());
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
        assert_eq!(classify("foo?").unwrap(), "[t(try) t(foo)]");
    }

    #[test]
    fn chained_suffix() {
        assert_eq!(classify("foo??").unwrap(), "[t(try) [t(try) t(foo)]]");
    }

    #[test]
    fn suffix_after_attr() {
        assert_eq!(
            classify("foo.bar?").unwrap(),
            "[t(try) [t(attr) t(foo) t(bar)]]"
        );
    }

    #[test]
    fn negation_over_suffix() {
        assert_eq!(classify("!foo?").unwrap(), "[t(not) [t(try) t(foo)]]");
    }

    #[test]
    fn leading_suffix_errors() {
        assert!(classify("?foo").is_err());
    }
}
