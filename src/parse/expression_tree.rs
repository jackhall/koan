//! Top-level parser: runs the `quote-mask → whitespace-collapse → tokenize →
//! tree-build` pipeline and returns a `KExpression`. The parse-stack and frame
//! abstractions live in sibling modules ([`super::parse_stack`], [`super::frame`]);
//! dict-pair state lives on [`super::dict_literal::DictFrame`] and type-expression
//! folding lives on [`super::type_expr_frame::TypeExprFrame`] so framing arms here stay
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
use super::type_expr_frame::TypeExprFrame;

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
    // `true` after consuming a `:(` glued type-expression sigil. The next `(` arm opens a
    // `TypeExpr` frame instead of an `Expression` frame.
    let mut pending_type_paren = false;

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
                if pending_type_paren {
                    pending_type_paren = false;
                    stack.push_frame(Frame::TypeExpr(TypeExprFrame::new()));
                } else {
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
            // Dispatch order: dict-pair separator first (state-machine has its own rules),
            // then `:|` / `:!` ascription operators (assembled here because `!` is itself a
            // prefix operator and would never reach keyword classification), then the
            // glued-right type sigils `:(` and `:T`. A lone `:` outside a dict (with
            // whitespace after or at EOF) is a parse error — type-position annotation must
            // glue the sigil to its operand.
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
                        Some('(') => {
                            pending_type_paren = true;
                            // Don't consume the '(' — let the next loop iteration's '(' arm
                            // see it and open the TypeExpr frame because pending_type_paren
                            // is set.
                        }
                        Some(ch) if ch.is_ascii_uppercase() => {
                            // Glued type sigil: the next token starts with an uppercase
                            // letter and will classify as a `Type` per `classify_atom`. We
                            // consume the `:` here and let the regular tokenizer produce
                            // the `ExpressionPart::Type`.
                        }
                        Some(ch) if ch.is_whitespace() => {
                            return Err(
                                "':' must be glued to its operand at a type position; \
                                 write `name :Type` (no space after `:`) or `:(List ...)`"
                                    .to_string(),
                            );
                        }
                        None => {
                            return Err(
                                "trailing ':' at end of input; expected a type name or `(`"
                                    .to_string(),
                            );
                        }
                        Some(ch) => {
                            return Err(format!(
                                "':' must be followed by a type name (uppercase-leading) or `(`; \
                                 got `{ch}`"
                            ));
                        }
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
            // `<` and `>` no longer carry type-position meaning (Design B retired the
            // `<>` TypeFrame in favour of the `:(...)` sigil). They flush the buffer and
            // emit standalone `Keyword` parts, leaving room for future numeric-comparison
            // operators to dispatch on. The `->` arrow stays contiguous because the
            // `prev == Some('-')` carve-out keeps `>` glued to the leading `-`.
            '>' if prev == Some('-') => {
                buf.push('>');
            }
            '<' | '>' => {
                flush_token(&mut stack, &mut buf)?;
                stack.push_part(ExpressionPart::Keyword(c.to_string()));
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
