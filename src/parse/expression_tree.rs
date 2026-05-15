//! Top-level parser: runs the `quote-mask → whitespace-collapse → tokenize →
//! tree-build` pipeline and returns a `KExpression`. The parse-stack and frame
//! abstractions live in sibling modules ([`super::parse_stack`], [`super::frame`]);
//! dict-pair state lives on [`super::dict_literal::DictFrame`] and type-parameter
//! folding lives on [`super::type_frame::TypeFrame`] so framing arms here stay
//! one-liners.
//!
//! See [design/expressions-and-parsing.md](../../design/expressions-and-parsing.md).

use std::collections::HashMap;

use crate::parse::quotes::{mask_quotes, QUOTE_PLACEHOLDER};
use crate::parse::whitespace::collapse_whitespace;
use crate::runtime::machine::model::ast::{ExpressionPart, KExpression, KLiteral};

use super::dict_literal::DictFrame;
use super::frame::{close_paren_to_part, Frame};
use super::parse_stack::{close_collection, flush_token, open_collection, ParseStack};
use super::type_frame::TypeFrame;

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
    let mut stack = ParseStack::new();
    let mut buf = String::new();
    let mut chars = masked.chars().peekable();
    // Used by adjacency checks for `[`, `{`, `<`, `>`.
    let mut prev: Option<char> = None;
    // `Some(c)` after consuming a `#`/`$` while waiting for the mandatory `(`. Sigils are
    // paren-only (`#foo`, `# (foo)`, `#42` are parse errors); enforced by the top-of-loop
    // guard below, so individual arms don't repeat the check.
    let mut pending_sigil: Option<char> = None;

    while let Some(c) = chars.next() {
        if let Some(s) = pending_sigil {
            if c != '(' {
                return Err(format!("expected '(' after '{s}', found '{c}'"));
            }
        }
        match c {
            '#' | '$' => {
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
                stack.push_frame(Frame::Expression {
                    expr: KExpression { parts: Vec::new() },
                    head,
                });
            }
            ')' => {
                flush_token(&mut stack, &mut buf)?;
                let frame = stack
                    .pop_top()
                    .ok_or_else(|| "closed paren without matching open paren".to_string())?;
                stack.push_part(close_paren_to_part(frame)?);
            }
            '[' => open_collection(&mut stack, &mut buf, '[', prev, Frame::List(Vec::new()))?,
            ']' => close_collection(
                &mut stack,
                &mut buf,
                ']',
                chars.peek().copied(),
                "closed bracket without matching open bracket",
            )?,
            '{' => open_collection(&mut stack, &mut buf, '{', prev, Frame::Dict(DictFrame::new()))?,
            '}' => close_collection(
                &mut stack,
                &mut buf,
                '}',
                chars.peek().copied(),
                "closed brace without matching open brace",
            )?,
            // `:|` / `:!` (ascription operators) must be assembled here, not in
            // `classify_token`: `!` is independently a prefix operator, so a bare `!` after
            // `:` would route through `parse_compound` and never reach keyword classification.
            ':' => {
                flush_token(&mut stack, &mut buf)?;
                if let Some(d) = stack.top_dict_mut() {
                    d.accept_colon()?;
                } else {
                    match chars.peek().copied() {
                        Some('|') => {
                            chars.next();
                            stack.push_part(ExpressionPart::Keyword(":|".to_string()));
                            prev = Some('|');
                            continue;
                        }
                        Some('!') => {
                            chars.next();
                            stack.push_part(ExpressionPart::Keyword(":!".to_string()));
                            prev = Some('!');
                            continue;
                        }
                        _ => stack.push_part(ExpressionPart::Keyword(":".to_string())),
                    }
                }
            }
            // Pair separator in a dict; whitespace-equivalent (no-op) everywhere else, so
            // type-annotation triples like `a: Number, b: Str` parse without altering shape.
            ',' => {
                flush_token(&mut stack, &mut buf)?;
                if let Some(d) = stack.top_dict_mut() {
                    d.accept_comma()?;
                }
            }
            // Opens a `Frame::Type` only when the parent's last part is a bare `Type` (no
            // params); otherwise emits `Keyword("<")` and requires whitespace separation, so
            // `List<Number>` parses but `a<b` is rejected as glued.
            '<' => {
                flush_token(&mut stack, &mut buf)?;
                if let Some(t) = stack.pop_if_bare_type_part() {
                    stack.push_frame(Frame::Type(TypeFrame::new(t.name)));
                } else {
                    check_separator_adjacency('<', prev)?;
                    stack.push_part(ExpressionPart::Keyword("<".to_string()));
                }
            }
            // Part of the `->` arrow token; applies both inside and outside a TypeFrame so
            // `Function<Number -> Str>` keeps its arrow contiguous.
            '>' if prev == Some('-') => {
                buf.push('>');
            }
            '>' => {
                flush_token(&mut stack, &mut buf)?;
                if stack.top_is_type() {
                    let tf = stack
                        .pop_if_type()
                        .expect("top_is_type checked above; flush_token preserves variant");
                    let parameterized = tf.build()?;
                    stack.push_part(ExpressionPart::Type(parameterized));
                } else {
                    check_separator_adjacency('>', prev)?;
                    stack.push_part(ExpressionPart::Keyword(">".to_string()));
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
                stack.push_part(ExpressionPart::Literal(KLiteral::String(literal)));
            }
            c if c.is_whitespace() => {
                flush_token(&mut stack, &mut buf)?;
            }
            _ => {
                buf.push(c);
            }
        }
        prev = Some(c);
    }
    if let Some(s) = pending_sigil {
        return Err(format!("trailing '{s}' sigil at end of input; expected '('"));
    }
    flush_token(&mut stack, &mut buf)?;
    stack.finish()
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
