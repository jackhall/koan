use std::collections::HashMap;

use crate::parse::kexpression::{ExpressionPart, KExpression, KLiteral};
use crate::parse::quotes::{mask_quotes, QUOTE_PLACEHOLDER};
use crate::parse::tokens::classify_token;
use crate::parse::whitespace::collapse_whitespace;

/// One frame of `build_tree`'s parse stack. Parens open an `Expression` frame whose contents
/// dispatch as a function call; brackets open a `List` frame whose contents become a
/// `KObject::List` value at runtime. Both frames accumulate `ExpressionPart`s the same way —
/// only the wrapper produced when the frame closes differs.
enum Frame<'a> {
    Expression(KExpression<'a>),
    List(Vec<ExpressionPart<'a>>),
}

impl<'a> Frame<'a> {
    fn push(&mut self, part: ExpressionPart<'a>) {
        match self {
            Frame::Expression(e) => e.parts.push(part),
            Frame::List(items) => items.push(part),
        }
    }
}

/// If `buf` holds a pending token, classify it via `tokens::classify_token` and push the result
/// onto the innermost frame on `stack`. Called by `build_tree` whenever a delimiter ends a run
/// of token characters.
fn flush_token<'a>(stack: &mut [Frame<'a>], buf: &mut String) -> Result<(), String> {
    if !buf.is_empty() {
        let tok = std::mem::take(buf);
        let part = classify_token(tok)?;
        stack.last_mut().unwrap().push(part);
    }
    Ok(())
}

/// Look up the original literal text for a `mask_quotes` placeholder. `inner` is the masked
/// content found between two matching quote characters during `build_tree`.
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

/// Walk a quote-masked, delimited string and assemble it into a nested `KExpression`. Opens a
/// new sub-expression on `(` and closes it on `)`; opens a list literal on `[` (when no
/// pending compound-token chars are buffered) and closes it on `]`. Recovers string literals
/// via `resolve_literal`, and classifies non-quoted runs through `tokens::classify_token`.
///
/// `[` triggers list-literal mode only when `buf` is empty — i.e., at the start of a new
/// whitespace-delimited token. Mid-token `[` (as in `foo[idx]`) stays in `buf` and is handled
/// later by `tokens::parse_compound`'s postfix-indexing operator. Symmetrically, `]` closes
/// the list only when the innermost frame is a `Frame::List`; inside an expression frame it
/// goes to `buf` to support compound tokens.
pub fn build_tree<'a>(masked: &str, quotes: &HashMap<usize, String>) -> Result<KExpression<'a>, String> {
    let mut stack: Vec<Frame<'a>> = vec![Frame::Expression(KExpression { parts: Vec::new() })];
    let mut buf = String::new();
    let mut chars = masked.chars().peekable();
    // Last char consumed from the input — used to enforce that list-literal `[`/`]` are
    // adjacent only to whitespace or matching delimiters, never glued to a token character
    // or string literal.
    let mut prev: Option<char> = None;

    while let Some(c) = chars.next() {
        match c {
            '(' => {
                flush_token(&mut stack, &mut buf)?;
                stack.push(Frame::Expression(KExpression { parts: Vec::new() }));
            }
            ')' => {
                flush_token(&mut stack, &mut buf)?;
                if stack.len() < 2 {
                    return Err("closed paren without matching open paren".to_string());
                }
                match stack.pop().unwrap() {
                    Frame::Expression(complete) => {
                        stack
                            .last_mut()
                            .unwrap()
                            .push(ExpressionPart::Expression(Box::new(complete)));
                    }
                    Frame::List(_) => {
                        return Err("closed paren but innermost frame is a list literal".to_string());
                    }
                }
            }
            // `[` always opens a list literal, but only when the preceding character is a
            // delimiter (whitespace, `(`, `[`, or start-of-input). Glueing a list to a token
            // — `foo[2]`, `"hi"[1]`, `(x)[2]` — is a parse error so list literals can't be
            // mistaken for indexing or string-subscript syntax.
            '[' => {
                if !is_list_open_delimiter(prev) {
                    return Err(format!(
                        "'[' must be preceded by whitespace, '(', or '[' \
                         (got {prev:?}); list literals can't be glued to a token",
                    ));
                }
                flush_token(&mut stack, &mut buf)?;
                stack.push(Frame::List(Vec::new()));
            }
            // `]` closes the innermost list frame. It's an error if no list is open or if
            // the next character isn't a delimiter (whitespace, `)`, `]`, or end-of-input).
            ']' => {
                if !matches!(stack.last(), Some(Frame::List(_))) {
                    return Err("closed bracket without matching open bracket".to_string());
                }
                let next = chars.peek().copied();
                if !is_list_close_delimiter(next) {
                    return Err(format!(
                        "']' must be followed by whitespace, ')', or ']' \
                         (got {next:?}); list literals can't be glued to a token",
                    ));
                }
                flush_token(&mut stack, &mut buf)?;
                let Frame::List(items) = stack.pop().unwrap() else { unreachable!() };
                stack.last_mut().unwrap().push(ExpressionPart::ListLiteral(items));
            }
            '\'' | '"' => {
                flush_token(&mut stack, &mut buf)?;
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
                    .push(ExpressionPart::Literal(KLiteral::String(literal)));
            }
            c if c.is_whitespace() => flush_token(&mut stack, &mut buf)?,
            _ => buf.push(c),
        }
        prev = Some(c);
    }
    flush_token(&mut stack, &mut buf)?;

    if stack.len() > 1 {
        return Err("open paren or bracket without matching close".to_string());
    }
    match stack.pop().unwrap() {
        Frame::Expression(e) => Ok(e),
        Frame::List(_) => Err("top-level frame should be an expression, got a list".to_string()),
    }
}

/// Predicate for the character preceding a list-literal `[`. List literals must stand alone
/// as their own whitespace-separated unit, not be glued to identifiers, strings, or
/// closing parens.
fn is_list_open_delimiter(prev: Option<char>) -> bool {
    matches!(prev, None | Some('(' | '['))
        || matches!(prev, Some(c) if c.is_whitespace())
}

/// Predicate for the character following a list-literal `]`. Symmetric to
/// `is_list_open_delimiter`: a list's right boundary must give way to whitespace, a
/// closing paren / bracket, or end-of-input.
fn is_list_close_delimiter(next: Option<char>) -> bool {
    matches!(next, None | Some(')' | ']'))
        || matches!(next, Some(c) if c.is_whitespace())
}

/// Strip redundant single-`Expression` wrappers from `expr` and each of its sub-expressions
/// so that `((foo bar))` and `(foo bar)` produce the same shape — extra parens shouldn't
/// change what gets dispatched downstream. Recurses into list literals so nested
/// sub-expressions inside a list are peeled the same way.
fn peel_redundant<'a>(mut expr: KExpression<'a>) -> KExpression<'a> {
    while expr.parts.len() == 1 && matches!(expr.parts[0], ExpressionPart::Expression(_)) {
        if let Some(ExpressionPart::Expression(inner)) = expr.parts.pop() {
            expr = *inner;
        }
    }
    expr.parts = expr.parts.into_iter().map(peel_part).collect();
    expr
}

/// Peel one `ExpressionPart`, recursing into `Expression` and `ListLiteral` children.
fn peel_part<'a>(part: ExpressionPart<'a>) -> ExpressionPart<'a> {
    match part {
        ExpressionPart::Expression(inner) => {
            ExpressionPart::Expression(Box::new(peel_redundant(*inner)))
        }
        ExpressionPart::ListLiteral(items) => {
            ExpressionPart::ListLiteral(items.into_iter().map(peel_part).collect())
        }
        other => other,
    }
}

/// Top-level parse pipeline: mask string literals, collapse indentation into parens, build
/// the expression tree, then peel redundant single-expression wrappers. Returns one
/// `KExpression` per top-level line; the single public entry point users of `parse` should
/// call.
pub fn parse<'a>(input: &str) -> Result<Vec<KExpression<'a>>, String> {
    let (masked, quotes) = mask_quotes(input);
    let collapsed = collapse_whitespace(&masked)?;
    let root = build_tree(&collapsed, &quotes)?;
    root.parts
        .into_iter()
        .map(|part| match part {
            ExpressionPart::Expression(e) => Ok(peel_redundant(*e)),
            other => Err(format!("unexpected top-level part: {:?}", other)),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::quotes::mask_quotes;

    fn describe(e: &KExpression<'_>) -> String {
        fn describe_part(p: &ExpressionPart<'_>) -> String {
            match p {
                ExpressionPart::Token(s) => format!("t({})", s),
                ExpressionPart::Expression(e) => describe(e),
                ExpressionPart::ListLiteral(items) => {
                    let inner: Vec<String> = items.iter().map(describe_part).collect();
                    format!("L[{}]", inner.join(" "))
                }
                ExpressionPart::Literal(KLiteral::String(s)) => format!("s({})", s),
                ExpressionPart::Literal(KLiteral::Number(n)) => format!("n({})", n),
                ExpressionPart::Literal(KLiteral::Boolean(b)) => format!("b({})", b),
                ExpressionPart::Literal(KLiteral::Null) => "null".to_string(),
                ExpressionPart::Future(_) => "future".to_string(),
            }
        }
        let parts: Vec<String> = e.parts.iter().map(describe_part).collect();
        format!("[{}]", parts.join(" "))
    }

    fn tree(input: &str) -> Result<String, String> {
        let (masked, dict) = mask_quotes(input);
        build_tree(&masked, &dict).map(|e| describe(&e))
    }

    fn top(input: &str) -> Result<Vec<String>, String> {
        parse(input).map(|exprs| exprs.iter().map(describe).collect())
    }

    #[test]
    fn parse_single_line_has_no_top_level_wrapper() {
        assert_eq!(top("foo bar").unwrap(), vec!["[t(foo) t(bar)]"]);
    }

    #[test]
    fn parse_multiple_lines_are_siblings() {
        assert_eq!(top("foo\nbar").unwrap(), vec!["[t(foo)]", "[t(bar)]"]);
    }

    #[test]
    fn parse_peels_top_level_redundant_parens() {
        assert_eq!(top("(foo bar)").unwrap(), top("foo bar").unwrap());
    }

    #[test]
    fn parse_peels_multiple_redundant_layers() {
        assert_eq!(top("(((foo bar)))").unwrap(), vec!["[t(foo) t(bar)]"]);
    }

    #[test]
    fn parse_peels_redundant_wrappers_inside_subexpressions() {
        // The inner `((bar baz))` collapses to `(bar baz)` — a sub-expression with one
        // wrapping layer, not two — so peel doesn't change argument arity.
        assert_eq!(
            top("foo ((bar baz))").unwrap(),
            top("foo (bar baz)").unwrap(),
        );
    }

    #[test]
    fn parse_keeps_meaningful_subexpression_parens() {
        // A single set of parens around an argument is meaningful structure, not redundancy.
        assert_eq!(
            top("foo (bar baz)").unwrap(),
            vec!["[t(foo) [t(bar) t(baz)]]"],
        );
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
    fn floats_and_scientific_are_number_literals() {
        assert_eq!(
            tree("3.14 1e3 -2.5e-2").unwrap(),
            "[n(3.14) n(1000) n(-0.025)]"
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

    #[test]
    fn empty_list_literal() {
        assert_eq!(tree("[]").unwrap(), "[L[]]");
    }

    #[test]
    fn flat_list_literal() {
        assert_eq!(tree("[1 2 3]").unwrap(), "[L[n(1) n(2) n(3)]]");
    }

    #[test]
    fn list_literal_with_identifiers_and_strings() {
        assert_eq!(
            tree(r#"[a "hi" 4]"#).unwrap(),
            "[L[t(a) s(hi) n(4)]]",
        );
    }

    #[test]
    fn nested_list_literal() {
        assert_eq!(
            tree("[[1 2] [3 4]]").unwrap(),
            "[L[L[n(1) n(2)] L[n(3) n(4)]]]",
        );
    }

    #[test]
    fn list_inside_paren_expression() {
        assert_eq!(
            tree("(LET xs = [1 2 3])").unwrap(),
            "[[t(LET) t(xs) t(=) L[n(1) n(2) n(3)]]]",
        );
    }

    #[test]
    fn paren_expression_inside_list() {
        // Sub-expressions inside list literals stay as Expression elements; the scheduler is
        // responsible for resolving them at runtime via the Aggregate node path.
        assert_eq!(
            tree("[(LET x = 1) y]").unwrap(),
            "[L[[t(LET) t(x) t(=) n(1)] t(y)]]",
        );
    }

    #[test]
    fn open_bracket_without_close_errors() {
        assert!(tree("[1 2 3").is_err());
    }

    #[test]
    fn close_bracket_without_open_errors() {
        assert!(tree("1 2]").is_err());
    }

    #[test]
    fn open_bracket_glued_to_token_errors() {
        // List literals must stand alone — `foo[2]` is no longer valid (was compound
        // indexing). The user must write `foo [2]` if they actually want a sibling list.
        assert!(tree("foo[2]").is_err());
    }

    #[test]
    fn close_bracket_glued_to_token_errors() {
        assert!(tree("[1 2]bar").is_err());
    }

    #[test]
    fn open_bracket_glued_to_close_paren_errors() {
        // `(x)[2]` is also forbidden: the result of a paren-expression can't be glued to a
        // list literal.
        assert!(tree("(x)[2]").is_err());
    }

    #[test]
    fn close_bracket_glued_to_open_paren_errors() {
        assert!(tree("[1](2)").is_err());
    }

    #[test]
    fn open_bracket_after_string_errors() {
        assert!(tree(r#""hi"[1]"#).is_err());
    }

    #[test]
    fn list_after_whitespace_is_fine() {
        assert_eq!(tree("foo [2]").unwrap(), "[t(foo) L[n(2)]]");
    }

    #[test]
    fn adjacent_brackets_in_nested_list_are_fine() {
        // `[[1 2]]` is two `[` then two `]` — each `[` is preceded by `(` or `[`, and each
        // `]` is followed by `]` or `)`. All adjacency rules satisfied.
        assert_eq!(tree("[[1 2]]").unwrap(), "[L[L[n(1) n(2)]]]");
    }
}
