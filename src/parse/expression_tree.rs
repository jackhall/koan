use std::collections::HashMap;

use crate::parse::kexpression::{ExpressionPart, KExpression, KLiteral};
use crate::parse::quotes::{mask_quotes, QUOTE_PLACEHOLDER};
use crate::parse::tokens::classify_token;
use crate::parse::whitespace::collapse_whitespace;

use super::dict_literal::DictFrame;

/// One frame of `build_tree`'s parse stack. Parens open an `Expression` frame whose contents
/// dispatch as a function call; brackets open a `List` frame whose contents become a
/// `KObject::List` value at runtime; braces open a `Dict` frame whose alternating
/// key/value parts (separated by `:`, terminated by `,` or whitespace) become a
/// `KObject::Dict`. Each frame accumulates `ExpressionPart`s the same way — only the wrapper
/// produced when the frame closes differs. The dict-pair state machine lives on `DictFrame`
/// (in [super::dict_literal]) rather than inline here, so `build_tree`'s `:` / `,` / `}`
/// arms read as one-liners.
enum Frame<'a> {
    Expression(KExpression<'a>),
    List(Vec<ExpressionPart<'a>>),
    Dict(DictFrame<'a>),
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
            // `:` is the key/value separator inside a dict frame. Outside one, it emits
            // a standalone `Keyword(":")` — the type-annotation separator (`x: Number`)
            // consumed by builtins like `UNION` (and, future-tense, function-signature
            // parameter declarations).
            ':' => {
                flush_token(&mut stack, &mut buf)?;
                match stack.last_mut().unwrap() {
                    Frame::Dict(d) => d.accept_colon()?,
                    frame => frame.push(ExpressionPart::Keyword(":".to_string())),
                }
            }
            // `,` is a pair separator inside a dict (commits an in-progress pair via
            // `DictFrame::accept_comma`) and whitespace-equivalent everywhere else — no-op
            // inside lists and expression frames. The expression-frame allowance lets
            // type-annotation triples (`a: Number, b: Str`) and future function-signature
            // parameter lists use commas as visual separators without changing the parsed
            // shape.
            ',' => {
                flush_token(&mut stack, &mut buf)?;
                match stack.last_mut().unwrap() {
                    Frame::Dict(d) => d.accept_comma()?,
                    Frame::List(_) | Frame::Expression(_) => {}
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
