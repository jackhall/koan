use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

use crate::kexpression::{ExpressionPart, KExpression, KLiteral};
use crate::kobject::KObject;
use crate::parse::quotes::{mask_quotes, QUOTE_PLACEHOLDER};
use crate::parse::whitespace::collapse_whitespace;

static SIGNED_INT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[+-]?\d+$").unwrap());

fn empty_expression() -> KExpression {
    KExpression {
        base: KObject { name: String::new(), remaining_args: HashMap::new() },
        parts: Vec::new(),
    }
}

fn classify_token(tok: String) -> ExpressionPart {
    match tok.as_str() {
        "null" => return ExpressionPart::Literal(KLiteral::Null),
        "true" => return ExpressionPart::Literal(KLiteral::Boolean(true)),
        "false" => return ExpressionPart::Literal(KLiteral::Boolean(false)),
        _ => {}
    }
    if SIGNED_INT.is_match(&tok) {
        if let Ok(n) = tok.parse::<f64>() {
            return ExpressionPart::Literal(KLiteral::Number(n));
        }
    }
    ExpressionPart::Token(tok)
}

fn flush_token(stack: &mut [KExpression], buf: &mut String) {
    if !buf.is_empty() {
        let tok = std::mem::take(buf);
        stack.last_mut().unwrap().parts.push(classify_token(tok));
    }
}

fn resolve_literal(inner: &str, quotes: &HashMap<usize, String>) -> Result<String, String> {
    if inner.is_empty() {
        return Ok(String::new());
    }
    let rest = inner
        .strip_prefix(QUOTE_PLACEHOLDER)
        .ok_or_else(|| format!("unexpected content between quotes: {:?}", inner))?;
    let idx: usize = rest
        .parse()
        .map_err(|_| format!("bad placeholder index in: {:?}", inner))?;
    quotes
        .get(&idx)
        .cloned()
        .ok_or_else(|| format!("unknown placeholder index: {}", idx))
}

pub fn build_tree(masked: &str, quotes: &HashMap<usize, String>) -> Result<KExpression, String> {
    let mut stack: Vec<KExpression> = vec![empty_expression()];
    let mut buf = String::new();
    let mut chars = masked.chars();

    while let Some(c) = chars.next() {
        match c {
            '(' => {
                flush_token(&mut stack, &mut buf);
                stack.push(empty_expression());
            }
            ')' => {
                flush_token(&mut stack, &mut buf);
                if stack.len() < 2 {
                    return Err("closed paren without matching open paren".to_string());
                }
                let complete = stack.pop().unwrap();
                stack
                    .last_mut()
                    .unwrap()
                    .parts
                    .push(ExpressionPart::Expression(Box::new(complete)));
            }
            '\'' | '"' => {
                flush_token(&mut stack, &mut buf);
                let open = c;
                let mut inner = String::new();
                loop {
                    match chars.next() {
                        None => return Err(format!("unclosed quote: {}", open)),
                        Some(ch) if ch == open => break,
                        Some(ch) => inner.push(ch),
                    }
                }
                let literal = resolve_literal(&inner, quotes)?;
                stack
                    .last_mut()
                    .unwrap()
                    .parts
                    .push(ExpressionPart::Literal(KLiteral::String(literal)));
            }
            c if c.is_whitespace() => flush_token(&mut stack, &mut buf),
            _ => buf.push(c),
        }
    }
    flush_token(&mut stack, &mut buf);

    if stack.len() > 1 {
        return Err("open paren without matching closed paren".to_string());
    }
    Ok(stack.pop().unwrap())
}

pub fn parse(input: &str) -> Result<KExpression, String> {
    let (masked, quotes) = mask_quotes(input);
    let collapsed = collapse_whitespace(&masked)?;
    build_tree(&collapsed, &quotes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::quotes::mask_quotes;

    fn describe(e: &KExpression) -> String {
        let parts: Vec<String> = e
            .parts
            .iter()
            .map(|p| match p {
                ExpressionPart::Token(s) => format!("t({})", s),
                ExpressionPart::Expression(e) => describe(e),
                ExpressionPart::Literal(KLiteral::String(s)) => format!("s({})", s),
                ExpressionPart::Literal(KLiteral::Number(n)) => format!("n({})", n),
                ExpressionPart::Literal(KLiteral::Boolean(b)) => format!("b({})", b),
                ExpressionPart::Literal(KLiteral::Null) => "null".to_string(),
            })
            .collect();
        format!("[{}]", parts.join(" "))
    }

    fn tree(input: &str) -> Result<String, String> {
        let (masked, dict) = mask_quotes(input);
        build_tree(&masked, &dict).map(|e| describe(&e))
    }

    #[test]
    fn empty_input() {
        assert_eq!(tree("").unwrap(), "[]");
    }

    #[test]
    fn single_token() {
        assert_eq!(tree("foo").unwrap(), "[t(foo)]");
    }

    #[test]
    fn split_on_whitespace() {
        assert_eq!(tree("hi there").unwrap(), "[t(hi) t(there)]");
    }

    #[test]
    fn runs_of_whitespace_collapse() {
        assert_eq!(tree("  hi   there  ").unwrap(), "[t(hi) t(there)]");
    }

    #[test]
    fn empty_parens() {
        assert_eq!(tree("()").unwrap(), "[[]]");
    }

    #[test]
    fn flat_parens() {
        assert_eq!(tree("(hi there)").unwrap(), "[[t(hi) t(there)]]");
    }

    #[test]
    fn siblings_and_groups() {
        assert_eq!(
            tree("hey (whoever you are) look at").unwrap(),
            "[t(hey) [t(whoever) t(you) t(are)] t(look) t(at)]"
        );
    }

    #[test]
    fn two_paren_groups() {
        assert_eq!(
            tree("hey (whoever you are) look at (that over there)").unwrap(),
            "[t(hey) [t(whoever) t(you) t(are)] t(look) t(at) [t(that) t(over) t(there)]]"
        );
    }

    #[test]
    fn nested_parens() {
        assert_eq!(
            tree("hey (whoever you are) look at (whatever (that over there) is)").unwrap(),
            "[t(hey) [t(whoever) t(you) t(are)] t(look) t(at) [t(whatever) [t(that) t(over) t(there)] t(is)]]"
        );
    }

    #[test]
    fn adjacent_paren_groups() {
        assert_eq!(
            tree("hey (whoever you are)(hello in this language)").unwrap(),
            "[t(hey) [t(whoever) t(you) t(are)] [t(hello) t(in) t(this) t(language)]]"
        );
    }

    #[test]
    fn deeply_nested() {
        assert_eq!(
            tree("hey (whoever (I think) you are (when I remember) now) look at").unwrap(),
            "[t(hey) [t(whoever) [t(I) t(think)] t(you) t(are) [t(when) t(I) t(remember)] t(now)] t(look) t(at)]"
        );
    }

    #[test]
    fn string_literal() {
        assert_eq!(tree(r#"say "hello""#).unwrap(), "[t(say) s(hello)]");
    }

    #[test]
    fn empty_string_literal() {
        assert_eq!(tree(r#""""#).unwrap(), "[s()]");
    }

    #[test]
    fn literal_inside_parens() {
        assert_eq!(
            tree(r#"print ("hello" to 'world')"#).unwrap(),
            "[t(print) [s(hello) t(to) s(world)]]"
        );
    }

    #[test]
    fn literal_adjacent_to_token() {
        assert_eq!(tree(r#"foo"bar"baz"#).unwrap(), "[t(foo) s(bar) t(baz)]");
    }

    #[test]
    fn integer_literal() {
        assert_eq!(tree("42").unwrap(), "[n(42)]");
    }

    #[test]
    fn signed_integers() {
        assert_eq!(tree("-5 +7 0 42").unwrap(), "[n(-5) n(7) n(0) n(42)]");
    }

    #[test]
    fn floats_and_scientific_stay_tokens() {
        assert_eq!(
            tree("3.14 1e3 -2.5e-2").unwrap(),
            "[t(3.14) t(1e3) t(-2.5e-2)]"
        );
    }

    #[test]
    fn bool_and_null_literals() {
        assert_eq!(
            tree("true false null").unwrap(),
            "[b(true) b(false) null]"
        );
    }

    #[test]
    fn inf_and_nan_stay_tokens() {
        assert_eq!(tree("inf NaN").unwrap(), "[t(inf) t(NaN)]");
    }

    #[test]
    fn capitalized_keywords_stay_tokens() {
        assert_eq!(
            tree("True False Null NULL").unwrap(),
            "[t(True) t(False) t(Null) t(NULL)]"
        );
    }

    #[test]
    fn mixed_expression() {
        assert_eq!(
            tree(r#"(set x 42) (set flag true) (set name "bob")"#).unwrap(),
            "[[t(set) t(x) n(42)] [t(set) t(flag) b(true)] [t(set) t(name) s(bob)]]"
        );
    }

    #[test]
    fn identifiers_with_digits_stay_tokens() {
        assert_eq!(tree("x1 foo2bar").unwrap(), "[t(x1) t(foo2bar)]");
    }

    #[test]
    fn close_without_open_errors() {
        assert!(tree(")(").is_err());
        assert!(tree("has closed) paren only").is_err());
        assert!(tree("two (closed one) open)").is_err());
    }

    #[test]
    fn open_without_close_errors() {
        assert!(tree("has (open paren only").is_err());
        assert!(tree("(two (open one closed)").is_err());
    }
}
