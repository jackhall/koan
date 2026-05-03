use std::collections::HashMap;

use crate::parse::kexpression::{ExpressionPart, KExpression, KLiteral};
use crate::parse::quotes::{mask_quotes, QUOTE_PLACEHOLDER};
use crate::parse::tokens::classify_token;
use crate::parse::whitespace::collapse_whitespace;

/// One frame of `build_tree`'s parse stack. Parens open an `Expression` frame whose contents
/// dispatch as a function call; brackets open a `List` frame whose contents become a
/// `KObject::List` value at runtime; braces open a `Dict` frame whose alternating
/// key/value parts (separated by `:`, terminated by `,` or whitespace) become a
/// `KObject::Dict`. Each frame accumulates `ExpressionPart`s the same way — only the wrapper
/// produced when the frame closes differs. The dict-pair state machine lives on `DictFrame`
/// rather than inline here, so `build_tree`'s `:` / `,` / `}` arms read as one-liners.
enum Frame<'a> {
    Expression(KExpression<'a>),
    List(Vec<ExpressionPart<'a>>),
    Dict(DictFrame<'a>),
}

/// In-progress dict literal: completed pairs plus the state of the current pair. Owns its
/// own state machine so character handlers in `build_tree` delegate to `accept_colon`,
/// `accept_comma`, and `finish` instead of pattern-matching the state inline.
struct DictFrame<'a> {
    pairs: Vec<(ExpressionPart<'a>, ExpressionPart<'a>)>,
    state: DictPairState<'a>,
}

/// State of the in-progress key/value pair inside a `DictFrame`. `Empty` is "ready for a
/// fresh key", `Key(parts)` is "accumulating key parts before we see ':'", `Value { key,
/// value }` is "saw ':', now collecting value parts until `,`, `}`, or auto-commit fires".
/// A multi-part key/value collapses via `single_or_wrapped`: one part stays as-is, multiple
/// parts wrap into a sub-expression.
enum DictPairState<'a> {
    Empty,
    Key(Vec<ExpressionPart<'a>>),
    Value {
        key: ExpressionPart<'a>,
        value: Vec<ExpressionPart<'a>>,
    },
}

/// Collapse the buffered parts of one half of a dict pair: a single part is the half
/// directly; multiple parts wrap as a sub-expression so the scheduler dispatches them.
fn single_or_wrapped<'a>(mut parts: Vec<ExpressionPart<'a>>) -> ExpressionPart<'a> {
    if parts.len() == 1 {
        parts.pop().unwrap()
    } else {
        ExpressionPart::expression(parts)
    }
}

/// Auto-commit threshold for `DictFrame`'s value side: any incoming part that could
/// plausibly start a fresh key (i.e. would be a complete dict half on its own). Used to
/// make commas optional — `{a: 1 b: 2}` parses identically to `{a: 1, b: 2}` because the
/// `b` token, arriving while `value = [1]`, triggers the previous pair's commit.
fn is_dict_key_start_part(part: &ExpressionPart<'_>) -> bool {
    matches!(
        part,
        ExpressionPart::Identifier(_)
            | ExpressionPart::Type(_)
            | ExpressionPart::Literal(_)
            | ExpressionPart::Expression(_)
            | ExpressionPart::ListLiteral(_)
            | ExpressionPart::DictLiteral(_)
    )
}

impl<'a> DictFrame<'a> {
    fn new() -> Self {
        Self { pairs: Vec::new(), state: DictPairState::Empty }
    }

    /// Accept an incoming part. Either appends to the current key/value being accumulated,
    /// or — when a value-side already has content and the new part could plausibly start a
    /// fresh key — auto-commits the in-progress pair before opening a new key.
    fn push(&mut self, part: ExpressionPart<'a>) {
        match &mut self.state {
            DictPairState::Empty => {
                self.state = DictPairState::Key(vec![part]);
            }
            DictPairState::Key(parts) => parts.push(part),
            DictPairState::Value { value, .. } => {
                if !value.is_empty() && is_dict_key_start_part(&part) {
                    let prev = std::mem::replace(&mut self.state, DictPairState::Empty);
                    if let DictPairState::Value { key, value } = prev {
                        self.pairs.push((key, single_or_wrapped(value)));
                    }
                    self.state = DictPairState::Key(vec![part]);
                } else {
                    value.push(part);
                }
            }
        }
    }

    /// Handle a `:` — promote the buffered key parts into a finalized key and switch to
    /// accumulating the value side. Errors if no key was buffered or if a `:` arrives
    /// while a value is already being built (one `:` per pair).
    fn accept_colon(&mut self) -> Result<(), String> {
        match std::mem::replace(&mut self.state, DictPairState::Empty) {
            DictPairState::Empty => {
                Err("missing key before ':' in dict literal".to_string())
            }
            DictPairState::Key(parts) if parts.is_empty() => {
                Err("missing key before ':' in dict literal".to_string())
            }
            DictPairState::Key(parts) => {
                self.state = DictPairState::Value {
                    key: single_or_wrapped(parts),
                    value: Vec::new(),
                };
                Ok(())
            }
            DictPairState::Value { key, value } => {
                // Restore for diagnostic context, then error.
                self.state = DictPairState::Value { key, value };
                Err("unexpected ':' inside dict value".to_string())
            }
        }
    }

    /// Handle a `,` — commit the in-progress pair if a value has been collected. Trailing
    /// or repeated commas no-op (`{a: 1,}` and `{a: 1,, b: 2}` both legal); a comma after
    /// a key without a value, or after `:` with no value, errors.
    fn accept_comma(&mut self) -> Result<(), String> {
        match std::mem::replace(&mut self.state, DictPairState::Empty) {
            DictPairState::Empty => Ok(()),
            DictPairState::Key(parts) if parts.is_empty() => Ok(()),
            DictPairState::Key(parts) => {
                self.state = DictPairState::Key(parts);
                Err("key without value in dict literal".to_string())
            }
            DictPairState::Value { value, .. } if value.is_empty() => {
                Err("missing value after ':' in dict literal".to_string())
            }
            DictPairState::Value { key, value } => {
                self.pairs.push((key, single_or_wrapped(value)));
                Ok(())
            }
        }
    }

    /// Handle `}` — commit any in-progress pair and yield the completed pair list. Errors
    /// for a key without `:` or a `:` without a value.
    fn finish(mut self) -> Result<Vec<(ExpressionPart<'a>, ExpressionPart<'a>)>, String> {
        match self.state {
            DictPairState::Empty => {}
            DictPairState::Key(parts) if parts.is_empty() => {}
            DictPairState::Key(_) => {
                return Err("unterminated key in dict literal (missing ':')".to_string());
            }
            DictPairState::Value { value, .. } if value.is_empty() => {
                return Err("missing value after ':' in dict literal".to_string());
            }
            DictPairState::Value { key, value } => {
                self.pairs.push((key, single_or_wrapped(value)));
            }
        }
        Ok(self.pairs)
    }
}

impl<'a> Frame<'a> {
    fn push(&mut self, part: ExpressionPart<'a>) {
        match self {
            Frame::Expression(e) => e.parts.push(part),
            Frame::List(items) => items.push(part),
            Frame::Dict(d) => d.push(part),
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
/// sub-expression on `(`, closes it on `)`; opens a list literal on `[`, closes it on `]`;
/// opens a dict literal on `{`, closes it on `}`. String literals are restored via
/// `resolve_literal`; other runs go through `classify_token`. Collection-literal brackets
/// must be adjacent only to delimiters — see `check_open_adjacency` / `check_close_adjacency`.
pub fn build_tree<'a>(masked: &str, quotes: &HashMap<usize, String>) -> Result<KExpression<'a>, String> {
    let mut stack: Vec<Frame<'a>> = vec![Frame::Expression(KExpression { parts: Vec::new() })];
    let mut buf = String::new();
    let mut chars = masked.chars().peekable();
    // Last char consumed from the input — used to enforce that collection-literal openings
    // (`[`, `{`) are adjacent only to whitespace or matching delimiters, never glued to a
    // token character or string literal.
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
                    Frame::Dict(_) => {
                        return Err("closed paren but innermost frame is a dict literal".to_string());
                    }
                }
            }
            '[' => {
                check_open_adjacency('[', prev)?;
                flush_token(&mut stack, &mut buf)?;
                stack.push(Frame::List(Vec::new()));
            }
            ']' => {
                if !matches!(stack.last(), Some(Frame::List(_))) {
                    return Err("closed bracket without matching open bracket".to_string());
                }
                check_close_adjacency(']', chars.peek().copied())?;
                flush_token(&mut stack, &mut buf)?;
                let Frame::List(items) = stack.pop().unwrap() else { unreachable!() };
                stack.last_mut().unwrap().push(ExpressionPart::ListLiteral(items));
            }
            '{' => {
                check_open_adjacency('{', prev)?;
                flush_token(&mut stack, &mut buf)?;
                stack.push(Frame::Dict(DictFrame::new()));
            }
            '}' => {
                if !matches!(stack.last(), Some(Frame::Dict(_))) {
                    return Err("closed brace without matching open brace".to_string());
                }
                check_close_adjacency('}', chars.peek().copied())?;
                flush_token(&mut stack, &mut buf)?;
                let Frame::Dict(d) = stack.pop().unwrap() else { unreachable!() };
                stack.last_mut().unwrap().push(ExpressionPart::DictLiteral(d.finish()?));
            }
            // `:` is the key/value separator inside a dict frame. Errors anywhere else.
            ':' => {
                flush_token(&mut stack, &mut buf)?;
                match stack.last_mut() {
                    Some(Frame::Dict(d)) => d.accept_colon()?,
                    _ => return Err("unexpected ':' outside dict literal".to_string()),
                }
            }
            // `,` is whitespace inside a list (no state to maintain) and a pair separator
            // inside a dict (commits an in-progress pair via `DictFrame::accept_comma`).
            // Errors anywhere else (top-level expressions, parens) since `,` has no meaning
            // there.
            ',' => {
                flush_token(&mut stack, &mut buf)?;
                match stack.last_mut() {
                    Some(Frame::List(_)) => {}
                    Some(Frame::Dict(d)) => d.accept_comma()?,
                    _ => return Err("unexpected ',' outside list or dict literal".to_string()),
                }
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
        Frame::Dict(_) => {
            Err("top-level frame should be an expression, got a dict".to_string())
        }
    }
}

/// Enforce that an opening collection bracket (`[` or `{`) is preceded by a delimiter
/// character, not glued to a token. Shared by `[` and `{` in `build_tree`.
fn check_open_adjacency(opener: char, prev: Option<char>) -> Result<(), String> {
    if matches!(prev, None | Some('(' | '[' | '{')) || matches!(prev, Some(c) if c.is_whitespace()) {
        return Ok(());
    }
    Err(format!(
        "'{opener}' must be preceded by whitespace, '(', '[', or '{{' \
         (got {prev:?}); collection literals can't be glued to a token",
    ))
}

/// Enforce that a closing collection bracket (`]` or `}`) is followed by a delimiter
/// character, not glued to a token. Symmetric to `check_open_adjacency`.
fn check_close_adjacency(closer: char, next: Option<char>) -> Result<(), String> {
    if matches!(next, None | Some(')' | ']' | '}')) || matches!(next, Some(c) if c.is_whitespace()) {
        return Ok(());
    }
    Err(format!(
        "'{closer}' must be followed by whitespace, ')', ']', or '}}' \
         (got {next:?}); collection literals can't be glued to a token",
    ))
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

/// Peel one `ExpressionPart`, recursing into `Expression`, `ListLiteral`, and `DictLiteral`
/// children. Dict pairs peel both halves so a sub-expression key/value with a redundant
/// wrapping (e.g. `{((k)): 1}`) collapses the same as it would inside a list or paren group.
fn peel_part<'a>(part: ExpressionPart<'a>) -> ExpressionPart<'a> {
    match part {
        ExpressionPart::Expression(inner) => {
            ExpressionPart::Expression(Box::new(peel_redundant(*inner)))
        }
        ExpressionPart::ListLiteral(items) => {
            ExpressionPart::ListLiteral(items.into_iter().map(peel_part).collect())
        }
        ExpressionPart::DictLiteral(pairs) => ExpressionPart::DictLiteral(
            pairs
                .into_iter()
                .map(|(k, v)| (peel_part(k), peel_part(v)))
                .collect(),
        ),
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
                ExpressionPart::Keyword(s) => format!("t({})", s),
                ExpressionPart::Identifier(s) => format!("t({})", s),
                ExpressionPart::Type(s) => format!("T({})", s),
                ExpressionPart::Expression(e) => describe(e),
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
        // `inf` is lowercase → Identifier; `NaN` is capitalized + has lowercase → Type.
        // Neither classifies as a numeric Literal, which is what this test guards.
        assert_eq!(tree("inf NaN").unwrap(), "[t(inf) T(NaN)]");
    }

    #[test]
    fn capitalized_names_classify_as_types_all_caps_as_keyword() {
        assert_eq!(
            tree("True False Null NULL").unwrap(),
            "[T(True) T(False) T(Null) t(NULL)]"
        );
    }

    #[test]
    fn camelcase_type_names_classify_as_types() {
        assert_eq!(
            tree("Number MyType KFunction Point3D").unwrap(),
            "[T(Number) T(MyType) T(KFunction) T(Point3D)]"
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
    fn list_literal_with_commas() {
        // Commas inside a list act as whitespace.
        assert_eq!(tree("[1, 2, 3]").unwrap(), "[L[n(1) n(2) n(3)]]");
    }

    #[test]
    fn list_with_and_without_commas_match() {
        assert_eq!(tree("[1, 2, 3]").unwrap(), tree("[1 2 3]").unwrap());
    }

    #[test]
    fn list_literal_with_trailing_comma() {
        assert_eq!(tree("[1, 2,]").unwrap(), "[L[n(1) n(2)]]");
    }

    #[test]
    fn list_literal_with_mixed_separators() {
        assert_eq!(tree("[1 , 2 ,3]").unwrap(), "[L[n(1) n(2) n(3)]]");
    }

    #[test]
    fn adjacent_brackets_in_nested_list_are_fine() {
        // `[[1 2]]` is two `[` then two `]` — each `[` is preceded by `(` or `[`, and each
        // `]` is followed by `]` or `)`. All adjacency rules satisfied.
        assert_eq!(tree("[[1 2]]").unwrap(), "[L[L[n(1) n(2)]]]");
    }

    // --- Dict literal tests ---

    #[test]
    fn empty_dict_literal() {
        assert_eq!(tree("{}").unwrap(), "[D{}]");
    }

    #[test]
    fn single_pair_dict() {
        assert_eq!(tree("{a: 1}").unwrap(), "[D{t(a): n(1)}]");
    }

    #[test]
    fn two_pairs_with_comma() {
        assert_eq!(
            tree("{a: 1, b: 2}").unwrap(),
            "[D{t(a): n(1), t(b): n(2)}]",
        );
    }

    #[test]
    fn two_pairs_without_comma() {
        // Auto-commit rule: `b` arriving while value=[1] commits the prior pair.
        assert_eq!(
            tree("{a: 1 b: 2}").unwrap(),
            "[D{t(a): n(1), t(b): n(2)}]",
        );
    }

    #[test]
    fn comma_and_no_comma_produce_identical_dict() {
        assert_eq!(tree("{a: 1, b: 2}").unwrap(), tree("{a: 1 b: 2}").unwrap());
    }

    #[test]
    fn string_key_dict() {
        assert_eq!(
            tree(r#"{"a": 1, "b": 2}"#).unwrap(),
            "[D{s(a): n(1), s(b): n(2)}]",
        );
    }

    #[test]
    fn number_and_bool_keys_dict() {
        assert_eq!(
            tree("{1: a, true: b}").unwrap(),
            "[D{n(1): t(a), b(true): t(b)}]",
        );
    }

    #[test]
    fn multi_part_value_in_parens() {
        assert_eq!(
            tree("{a: (foo bar)}").unwrap(),
            "[D{t(a): [t(foo) t(bar)]}]",
        );
    }

    #[test]
    fn nested_dict_in_dict() {
        assert_eq!(
            tree("{a: {b: 1}}").unwrap(),
            "[D{t(a): D{t(b): n(1)}}]",
        );
    }

    #[test]
    fn nested_list_in_dict() {
        assert_eq!(
            tree("{a: [1 2]}").unwrap(),
            "[D{t(a): L[n(1) n(2)]}]",
        );
    }

    #[test]
    fn nested_dict_in_list() {
        assert_eq!(
            tree("[{a: 1} {b: 2}]").unwrap(),
            "[L[D{t(a): n(1)} D{t(b): n(2)}]]",
        );
    }

    #[test]
    fn sub_expression_as_key() {
        assert_eq!(
            tree("{(name): 1}").unwrap(),
            "[D{[t(name)]: n(1)}]",
        );
    }

    #[test]
    fn sub_expression_as_value() {
        assert_eq!(
            tree("{a: (LET y = 7)}").unwrap(),
            "[D{t(a): [t(LET) t(y) t(=) n(7)]}]",
        );
    }

    #[test]
    fn trailing_comma_allowed() {
        assert_eq!(tree("{a: 1,}").unwrap(), "[D{t(a): n(1)}]");
    }

    #[test]
    fn unbalanced_colon_errors() {
        // Second `:` inside the same value position is rejected.
        assert!(tree("{a: 1: 2}").is_err());
    }

    #[test]
    fn key_without_value_errors() {
        assert!(tree("{a:}").is_err());
    }

    #[test]
    fn key_without_colon_errors() {
        assert!(tree("{a 1}").is_err());
    }

    #[test]
    fn colon_outside_dict_errors() {
        assert!(tree("a: 1").is_err());
    }

    #[test]
    fn comma_outside_dict_errors() {
        assert!(tree("a, b").is_err());
    }

    #[test]
    fn unclosed_dict_errors() {
        assert!(tree("{a: 1").is_err());
    }

    #[test]
    fn close_brace_without_open_errors() {
        assert!(tree("a}").is_err());
    }

    #[test]
    fn open_brace_glued_to_token_errors() {
        assert!(tree("foo{a: 1}").is_err());
    }

    #[test]
    fn close_brace_glued_to_token_errors() {
        assert!(tree("{a: 1}bar").is_err());
    }

    #[test]
    fn multi_part_value_without_parens_errors() {
        // `{a: foo bar}` parses key=`a`, value=`foo`, auto-commits — then `bar` starts a new
        // key and `}` closes with that key unterminated. The constraint is intentional:
        // dict values are single-token unless parenthesized, mirroring list elements.
        assert!(tree("{a: foo bar}").is_err());
    }

    #[test]
    fn multiline_dict_via_top_level_pipeline() {
        // Multi-line dict goes through the full `parse` pipeline since `collapse_whitespace`
        // is the part that handles continuation. `tree` skips that step so we use `top`.
        assert_eq!(
            top("LET d = {\n  a: 1\n  b: 2\n}").unwrap(),
            vec!["[t(LET) t(d) t(=) D{t(a): n(1), t(b): n(2)}]"],
        );
    }
}
