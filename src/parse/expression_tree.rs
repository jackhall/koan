//! Top-level parser: runs the `quote-mask → whitespace-collapse → tokenize →
//! tree-build` pipeline and returns a `KExpression`. Dict-pair state lives on
//! `dict_literal::DictFrame` and type-parameter folding lives on
//! `type_frame::TypeFrame` so framing arms here stay one-liners.
//!
//! See [design/expressions-and-parsing.md](../../design/expressions-and-parsing.md).

use std::collections::HashMap;

use crate::parse::kexpression::{ExpressionPart, KExpression, KLiteral, TypeParams};
use crate::parse::quotes::{mask_quotes, QUOTE_PLACEHOLDER};
use crate::parse::tokens::classify_token;
use crate::parse::whitespace::collapse_whitespace;

use super::dict_literal::DictFrame;
use super::type_frame::TypeFrame;

/// One frame of `build_tree`'s parse stack. `Expression::head` is `Some` only when the frame
/// was opened by a `#(...)` / `$(...)` sigil; on close such a frame yields the
/// `(QUOTE <body>)` / `(EVAL <body>)` AST shape rather than a bare `Expression` part.
enum Frame<'a> {
    Expression {
        expr: KExpression<'a>,
        head: Option<&'static str>,
    },
    List(Vec<ExpressionPart<'a>>),
    Dict(DictFrame<'a>),
    Type(TypeFrame<'a>),
}

impl<'a> Frame<'a> {
    fn push(&mut self, part: ExpressionPart<'a>) {
        match self {
            Frame::Expression { expr, .. } => expr.parts.push(part),
            Frame::List(items) => items.push(part),
            Frame::Dict(d) => d.push(part),
            Frame::Type(tf) => tf.parts.push(part),
        }
    }

    /// Returns `None` for `Dict` (its state machine doesn't expose a flat last-part), which
    /// makes `<` after a Type inside a dict frame degrade to `Keyword("<")`.
    fn last_part(&self) -> Option<&ExpressionPart<'a>> {
        match self {
            Frame::Expression { expr, .. } => expr.parts.last(),
            Frame::List(items) => items.last(),
            Frame::Type(tf) => tf.parts.last(),
            Frame::Dict(_) => None,
        }
    }

    /// Symmetric to `last_part`: `Dict` returns `None`.
    fn pop_last_part(&mut self) -> Option<ExpressionPart<'a>> {
        match self {
            Frame::Expression { expr, .. } => expr.parts.pop(),
            Frame::List(items) => items.pop(),
            Frame::Type(tf) => tf.parts.pop(),
            Frame::Dict(_) => None,
        }
    }
}

fn flush_token<'a>(stack: &mut [Frame<'a>], buf: &mut String) -> Result<(), String> {
    if !buf.is_empty() {
        let tok = std::mem::take(buf);
        let part = classify_token(tok)?;
        stack.last_mut().unwrap().push(part);
    }
    Ok(())
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

pub fn build_tree<'a>(masked: &str, quotes: &HashMap<usize, String>) -> Result<KExpression<'a>, String> {
    let mut stack: Vec<Frame<'a>> = vec![Frame::Expression {
        expr: KExpression { parts: Vec::new() },
        head: None,
    }];
    let mut buf = String::new();
    let mut chars = masked.chars().peekable();
    // Used by adjacency checks for `[`, `{`, `<`, `>`.
    let mut prev: Option<char> = None;
    // `Some(c)` after consuming a `#`/`$` while waiting for the mandatory `(`. Any other arm
    // rejects it — sigils are paren-only (`#foo`, `# (foo)`, `#42` are parse errors).
    let mut pending_sigil: Option<char> = None;

    while let Some(c) = chars.next() {
        match c {
            '#' | '$' => {
                assert_no_pending(&pending_sigil, c)?;
                // Reject `foo#(...)` here so the diagnostic points at the sigil; otherwise it
                // would surface on the following char as "expected '(' after sigil".
                if !buf.is_empty() {
                    return Err(format!(
                        "'{c}' sigil must be preceded by whitespace or '(' (got token char {prev:?})"
                    ));
                }
                pending_sigil = Some(c);
            }
            '(' => {
                flush_token(&mut stack, &mut buf)?;
                let head = match pending_sigil.take() {
                    Some('#') => Some("QUOTE"),
                    Some('$') => Some("EVAL"),
                    _ => None,
                };
                stack.push(Frame::Expression {
                    expr: KExpression { parts: Vec::new() },
                    head,
                });
            }
            ')' => {
                assert_no_pending(&pending_sigil, c)?;
                flush_token(&mut stack, &mut buf)?;
                if stack.len() < 2 {
                    return Err("closed paren without matching open paren".to_string());
                }
                match stack.pop().unwrap() {
                    Frame::Expression { expr: complete, head: None } => {
                        stack
                            .last_mut()
                            .unwrap()
                            .push(ExpressionPart::Expression(Box::new(complete)));
                    }
                    // Sigil frame closes as `(QUOTE body)` / `(EVAL body)`; `body` is always
                    // a single nested `Expression` part so the builtin's `KExpression` slot
                    // accepts it regardless of how many tokens were inside the sigil parens.
                    Frame::Expression { expr: complete, head: Some(head) } => {
                        let wrapped = KExpression {
                            parts: vec![
                                ExpressionPart::Keyword(head.to_string()),
                                ExpressionPart::Expression(Box::new(complete)),
                            ],
                        };
                        stack
                            .last_mut()
                            .unwrap()
                            .push(ExpressionPart::Expression(Box::new(wrapped)));
                    }
                    Frame::List(_) => {
                        return Err("closed paren but innermost frame is a list literal".to_string());
                    }
                    Frame::Dict(_) => {
                        return Err("closed paren but innermost frame is a dict literal".to_string());
                    }
                    Frame::Type(_) => {
                        return Err("closed paren but innermost frame is a type-parameter group".to_string());
                    }
                }
            }
            '[' => {
                assert_no_pending(&pending_sigil, c)?;
                check_open_adjacency('[', prev)?;
                flush_token(&mut stack, &mut buf)?;
                stack.push(Frame::List(Vec::new()));
            }
            ']' => {
                assert_no_pending(&pending_sigil, c)?;
                if !matches!(stack.last(), Some(Frame::List(_))) {
                    return Err("closed bracket without matching open bracket".to_string());
                }
                check_close_adjacency(']', chars.peek().copied())?;
                flush_token(&mut stack, &mut buf)?;
                let Frame::List(items) = stack.pop().unwrap() else { unreachable!() };
                stack.last_mut().unwrap().push(ExpressionPart::ListLiteral(items));
            }
            '{' => {
                assert_no_pending(&pending_sigil, c)?;
                check_open_adjacency('{', prev)?;
                flush_token(&mut stack, &mut buf)?;
                stack.push(Frame::Dict(DictFrame::new()));
            }
            '}' => {
                assert_no_pending(&pending_sigil, c)?;
                if !matches!(stack.last(), Some(Frame::Dict(_))) {
                    return Err("closed brace without matching open brace".to_string());
                }
                check_close_adjacency('}', chars.peek().copied())?;
                flush_token(&mut stack, &mut buf)?;
                let Frame::Dict(d) = stack.pop().unwrap() else { unreachable!() };
                stack.last_mut().unwrap().push(ExpressionPart::DictLiteral(d.finish()?));
            }
            // `:|` / `:!` (ascription operators) must be assembled here, not in
            // `classify_token`: `!` is independently a prefix operator, so a bare `!` after
            // `:` would route through `parse_compound` and never reach keyword classification.
            ':' => {
                assert_no_pending(&pending_sigil, c)?;
                flush_token(&mut stack, &mut buf)?;
                match stack.last_mut().unwrap() {
                    Frame::Dict(d) => d.accept_colon()?,
                    frame => match chars.peek().copied() {
                        Some('|') => {
                            chars.next();
                            frame.push(ExpressionPart::Keyword(":|".to_string()));
                            prev = Some('|');
                            continue;
                        }
                        Some('!') => {
                            chars.next();
                            frame.push(ExpressionPart::Keyword(":!".to_string()));
                            prev = Some('!');
                            continue;
                        }
                        _ => frame.push(ExpressionPart::Keyword(":".to_string())),
                    },
                }
            }
            // Pair separator in a dict; whitespace-equivalent (no-op) everywhere else, so
            // type-annotation triples like `a: Number, b: Str` parse without altering shape.
            ',' => {
                assert_no_pending(&pending_sigil, c)?;
                flush_token(&mut stack, &mut buf)?;
                match stack.last_mut().unwrap() {
                    Frame::Dict(d) => d.accept_comma()?,
                    Frame::List(_) | Frame::Expression { .. } | Frame::Type(_) => {}
                }
            }
            // Opens a `Frame::Type` only when the parent's last part is a bare `Type` (no
            // params); otherwise emits `Keyword("<")` and requires whitespace separation, so
            // `List<Number>` parses but `a<b` is rejected as glued.
            '<' => {
                assert_no_pending(&pending_sigil, c)?;
                flush_token(&mut stack, &mut buf)?;
                let parent = stack.last_mut().unwrap();
                let opens_type_frame = matches!(
                    parent.last_part(),
                    Some(ExpressionPart::Type(t)) if matches!(t.params, TypeParams::None)
                );
                if opens_type_frame {
                    let Some(ExpressionPart::Type(t)) = parent.pop_last_part() else {
                        unreachable!("checked above")
                    };
                    stack.push(Frame::Type(TypeFrame::new(t.name)));
                } else {
                    check_separator_adjacency('<', prev)?;
                    parent.push(ExpressionPart::Keyword("<".to_string()));
                }
            }
            // Part of the `->` arrow token; applies both inside and outside a TypeFrame so
            // `Function<Number -> Str>` keeps its arrow contiguous.
            '>' if prev == Some('-') => {
                assert_no_pending(&pending_sigil, c)?;
                buf.push('>');
            }
            '>' => {
                assert_no_pending(&pending_sigil, c)?;
                flush_token(&mut stack, &mut buf)?;
                if matches!(stack.last(), Some(Frame::Type(_))) {
                    let Frame::Type(tf) = stack.pop().unwrap() else { unreachable!() };
                    let parameterized = tf.build()?;
                    stack
                        .last_mut()
                        .unwrap()
                        .push(ExpressionPart::Type(parameterized));
                } else {
                    check_separator_adjacency('>', prev)?;
                    stack
                        .last_mut()
                        .unwrap()
                        .push(ExpressionPart::Keyword(">".to_string()));
                }
            }
            '\'' | '"' => {
                assert_no_pending(&pending_sigil, c)?;
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
            c if c.is_whitespace() => {
                assert_no_pending(&pending_sigil, c)?;
                flush_token(&mut stack, &mut buf)?;
            }
            _ => {
                assert_no_pending(&pending_sigil, c)?;
                buf.push(c);
            }
        }
        prev = Some(c);
    }
    if let Some(s) = pending_sigil {
        return Err(format!("trailing '{s}' sigil at end of input; expected '('"));
    }
    flush_token(&mut stack, &mut buf)?;

    if stack.len() > 1 {
        return Err("open paren, bracket, brace, or angle-bracket without matching close".to_string());
    }
    match stack.pop().unwrap() {
        Frame::Expression { expr, head: None } => Ok(expr),
        // Unreachable: the root frame is created with `head: None` and sigils only nest.
        Frame::Expression { head: Some(_), .. } => {
            Err("top-level frame unexpectedly carries a sigil head".to_string())
        }
        Frame::List(_) => Err("top-level frame should be an expression, got a list".to_string()),
        Frame::Dict(_) => {
            Err("top-level frame should be an expression, got a dict".to_string())
        }
        Frame::Type(_) => {
            Err("top-level frame should be an expression, got a type-parameter group".to_string())
        }
    }
}

fn assert_no_pending(pending: &Option<char>, c: char) -> Result<(), String> {
    if let Some(s) = pending {
        return Err(format!("expected '(' after '{s}', found '{c}'"));
    }
    Ok(())
}

fn check_open_adjacency(opener: char, prev: Option<char>) -> Result<(), String> {
    if matches!(prev, None | Some('(' | '[' | '{')) || matches!(prev, Some(c) if c.is_whitespace()) {
        return Ok(());
    }
    Err(format!(
        "'{opener}' must be preceded by whitespace, '(', '[', or '{{' \
         (got {prev:?}); collection literals can't be glued to a token",
    ))
}

/// Rejects glued shapes like `Number>` / `a<b` so they surface as parse errors instead of
/// silently splitting into separate tokens. TypeFrame open/close paths bypass this check.
fn check_separator_adjacency(c: char, prev: Option<char>) -> Result<(), String> {
    if matches!(prev, None | Some('(' | ')' | '[' | ']' | '{' | '}' | ','))
        || matches!(prev, Some(p) if p.is_whitespace())
    {
        return Ok(());
    }
    Err(format!(
        "'{c}' must be preceded by whitespace or a delimiter (got {prev:?}); \
         '<' and '>' outside type position cannot be glued to a token",
    ))
}

/// Symmetric to `check_open_adjacency` for closing brackets.
fn check_close_adjacency(closer: char, next: Option<char>) -> Result<(), String> {
    if matches!(next, None | Some(')' | ']' | '}')) || matches!(next, Some(c) if c.is_whitespace()) {
        return Ok(());
    }
    Err(format!(
        "'{closer}' must be followed by whitespace, ')', ']', or '}}' \
         (got {next:?}); collection literals can't be glued to a token",
    ))
}

/// Collapses single-`Expression` wrappers so `((foo bar))` and `(foo bar)` dispatch the same.
fn peel_redundant<'a>(mut expr: KExpression<'a>) -> KExpression<'a> {
    while expr.parts.len() == 1 && matches!(expr.parts[0], ExpressionPart::Expression(_)) {
        if let Some(ExpressionPart::Expression(inner)) = expr.parts.pop() {
            expr = *inner;
        }
    }
    expr.parts = expr.parts.into_iter().map(peel_part).collect();
    expr
}

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

/// Public entry point: returns one `KExpression` per top-level line.
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
