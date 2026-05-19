//! Top-level parser: runs the `quote-mask → whitespace-collapse → tokenize →
//! tree-build` pipeline and returns a `KExpression`. The parse-stack and frame
//! abstractions live in sibling modules ([`super::parse_stack`], [`super::frame`]);
//! dict-pair state lives on [`super::dict_literal::DictFrame`] and type-expression
//! folding lives on [`super::type_expr_frame::TypeExprFrame`] so framing arms here stay
//! one-liners.
//!
//! Phase 2: `build_tree` now consumes the `Vec<u8>` masked stream emitted by `quotes`
//! and `whitespace`. The `chars().peekable()` iterator has been replaced with a
//! hand-rolled UTF-8 byte cursor (`Reader`) that walks codepoints and handles in-band
//! LITERAL / JUMP markers. The marker payloads are consumed and discarded — no cursor
//! tracking yet — so spans on `KExpression` and `ExpressionPart` remain `None` through
//! this phase. Phase 4 wires the byte offsets through.
//!
//! See [design/expressions-and-parsing.md](../../design/expressions-and-parsing.md).

use std::collections::HashMap;

use crate::machine::core::source::Spanned;
use crate::machine::model::ast::{ExpressionPart, KExpression, KLiteral};
use crate::parse::quotes::{mask_quotes, JUMP_MARK, LEN_SEP, LITERAL_MARK};
use crate::parse::whitespace::collapse_whitespace;

use super::dict_literal::DictFrame;
use super::frame::{close_paren_to_part, Frame};
use super::parse_stack::{close_collection, flush_token, open_collection, ParseStack};
use super::type_expr_frame::TypeExprFrame;

/// Width of the UTF-8 codepoint whose leading byte is `b`. Defaults to 1 on malformed
/// continuation bytes so a corrupt input still terminates rather than spinning.
fn utf8_width(b: u8) -> usize {
    match b {
        _ if b < 0x80 => 1,
        _ if b & 0xE0 == 0xC0 => 2,
        _ if b & 0xF0 == 0xE0 => 3,
        _ if b & 0xF8 == 0xF0 => 4,
        _ => 1,
    }
}

/// Decode the codepoint at `bytes[pos]` without advancing. Returns `None` past EOF or
/// on malformed UTF-8 (the masked stream is always valid UTF-8 in practice).
fn decode_char_at(bytes: &[u8], pos: usize) -> Option<char> {
    let b = *bytes.get(pos)?;
    let width = utf8_width(b);
    let slice = bytes.get(pos..pos + width)?;
    std::str::from_utf8(slice).ok()?.chars().next()
}

/// Hand-rolled byte cursor over the masked stream. Tracks only `pos` in Phase 2;
/// Phase 4 will add a `cursor: u32` field for original-source byte offsets driven
/// by the in-band JUMP / LITERAL markers.
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn peek_byte(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn peek_char(&self) -> Option<char> {
        decode_char_at(self.bytes, self.pos)
    }

    fn advance_byte(&mut self) {
        self.pos += 1;
    }

    /// Decode the codepoint at `pos` and advance `pos` by its UTF-8 width.
    fn advance_codepoint(&mut self) -> char {
        let b = self.bytes[self.pos];
        let width = utf8_width(b);
        let c = decode_char_at(self.bytes, self.pos)
            .expect("masked stream must be valid UTF-8");
        self.pos += width;
        c
    }

    /// Consume a `\x1D<digits>\x1D` JUMP marker. The leading sentinel must already be
    /// at the cursor. Returns the parsed offset; in Phase 2 the caller discards it.
    fn read_jump(&mut self) -> Result<u32, String> {
        debug_assert_eq!(self.peek_byte(), Some(JUMP_MARK));
        self.pos += 1;
        let value = self.read_decimal(JUMP_MARK, "JUMP marker")?;
        if self.peek_byte() != Some(JUMP_MARK) {
            return Err("JUMP marker missing closing sentinel".to_string());
        }
        self.pos += 1;
        Ok(value)
    }

    /// Consume a `\x1F<idx>\x1E<orig_byte_len>` LITERAL marker. The leading sentinel
    /// must already be at the cursor. Returns `(idx, orig_byte_len)`; in Phase 2 the
    /// caller uses `idx` to look up the literal text and discards the length.
    fn read_literal_marker(&mut self) -> Result<(usize, u32), String> {
        debug_assert_eq!(self.peek_byte(), Some(LITERAL_MARK));
        self.pos += 1;
        let idx = self.read_decimal(LEN_SEP, "LITERAL marker idx")?;
        if self.peek_byte() != Some(LEN_SEP) {
            return Err("LITERAL marker missing length separator".to_string());
        }
        self.pos += 1;
        let len = self.read_decimal_until_non_digit("LITERAL marker length")?;
        Ok((idx as usize, len))
    }

    /// Read ASCII decimal digits up to (but not consuming) `stop`. Errors when the
    /// run is empty or when the digits don't fit in `u32`.
    fn read_decimal(&mut self, stop: u8, label: &str) -> Result<u32, String> {
        let start = self.pos;
        while let Some(b) = self.peek_byte() {
            if b == stop {
                break;
            }
            if !b.is_ascii_digit() {
                return Err(format!("{label}: non-digit byte {b:#x} in payload"));
            }
            self.pos += 1;
        }
        let digits = &self.bytes[start..self.pos];
        if digits.is_empty() {
            return Err(format!("{label}: empty decimal payload"));
        }
        std::str::from_utf8(digits)
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .ok_or_else(|| format!("{label}: invalid decimal payload"))
    }

    /// Read ASCII decimal digits until the next non-digit byte (or EOF). Errors when
    /// the run is empty or when the digits don't fit in `u32`.
    fn read_decimal_until_non_digit(&mut self, label: &str) -> Result<u32, String> {
        let start = self.pos;
        while let Some(b) = self.peek_byte() {
            if !b.is_ascii_digit() {
                break;
            }
            self.pos += 1;
        }
        let digits = &self.bytes[start..self.pos];
        if digits.is_empty() {
            return Err(format!("{label}: empty decimal payload"));
        }
        std::str::from_utf8(digits)
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .ok_or_else(|| format!("{label}: invalid decimal payload"))
    }
}

pub fn build_tree<'a>(
    masked: &[u8],
    quotes: &HashMap<usize, String>,
) -> Result<KExpression<'a>, String> {
    let mut stack = ParseStack::new();
    let mut buf = String::new();
    let mut reader = Reader::new(masked);
    let mut prev: Option<char> = None;
    let mut pending_sigil: Option<char> = None;
    let mut pending_type_paren = false;

    loop {
        // Drain JUMP markers at the top of the loop so the dispatch never has to
        // think about them. In Phase 2 the offset value is discarded; Phase 4 will
        // snap a `cursor` field to it.
        while reader.peek_byte() == Some(JUMP_MARK) {
            reader.read_jump()?;
        }
        let Some(b) = reader.peek_byte() else { break };

        let c = reader
            .peek_char()
            .ok_or_else(|| "malformed UTF-8 in masked stream".to_string())?;

        if let Some(s) = pending_sigil {
            if c != '(' {
                return Err(format!("expected '(' after '{s}', found '{c}'"));
            }
        }

        match c {
            '#' | '$' => {
                if !buf.is_empty() {
                    return Err(format!(
                        "'{c}' sigil must be preceded by whitespace or '(' (got token char {prev:?})"
                    ));
                }
                pending_sigil = Some(c);
                reader.advance_byte();
            }
            '(' => {
                flush_token(&mut stack, &mut buf)?;
                reader.advance_byte();
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
                        expr: KExpression::new(Vec::new()),
                        head,
                    });
                }
            }
            ')' => {
                flush_token(&mut stack, &mut buf)?;
                reader.advance_byte();
                let frame = stack
                    .pop_top()
                    .ok_or_else(|| "closed paren without matching open paren".to_string())?;
                stack.push_part(close_paren_to_part(frame)?);
            }
            '[' => {
                open_collection(&mut stack, &mut buf, '[', prev, Frame::List(Vec::new()))?;
                reader.advance_byte();
            }
            ']' => {
                let next = decode_char_at(reader.bytes, reader.pos + 1);
                close_collection(
                    &mut stack,
                    &mut buf,
                    ']',
                    next,
                    "closed bracket without matching open bracket",
                )?;
                reader.advance_byte();
            }
            '{' => {
                open_collection(&mut stack, &mut buf, '{', prev, Frame::Dict(DictFrame::new()))?;
                reader.advance_byte();
            }
            '}' => {
                let next = decode_char_at(reader.bytes, reader.pos + 1);
                close_collection(
                    &mut stack,
                    &mut buf,
                    '}',
                    next,
                    "closed brace without matching open brace",
                )?;
                reader.advance_byte();
            }
            // Dispatch order: dict-pair separator first (state-machine has its own rules),
            // then `:|` / `:!` ascription operators (assembled here because `!` is itself a
            // prefix operator and would never reach keyword classification), then the
            // glued-right type sigils `:(` and `:T`. A lone `:` outside a dict (with
            // whitespace after or at EOF) is a parse error — type-position annotation must
            // glue the sigil to its operand.
            ':' => {
                flush_token(&mut stack, &mut buf)?;
                reader.advance_byte();
                if let Some(d) = stack.top_dict_mut() {
                    d.accept_colon()?;
                } else {
                    match reader.peek_byte() {
                        Some(b'|') => {
                            reader.advance_byte();
                            stack.push_part(ExpressionPart::Keyword(":|".to_string()));
                            prev = Some('|');
                            continue;
                        }
                        Some(b'!') => {
                            reader.advance_byte();
                            stack.push_part(ExpressionPart::Keyword(":!".to_string()));
                            prev = Some('!');
                            continue;
                        }
                        Some(b'(') => {
                            pending_type_paren = true;
                            // Don't consume the '(' — let the next loop iteration's '(' arm
                            // see it and open the TypeExpr frame because pending_type_paren
                            // is set.
                        }
                        Some(byte) if byte.is_ascii_uppercase() => {
                            // Glued type sigil: the next token starts with an uppercase
                            // letter and will classify as a `Type` per `classify_atom`.
                            // The `:` is already consumed; the regular tokenizer produces
                            // the `ExpressionPart::Type` from the following identifier.
                        }
                        Some(_) => {
                            let next_char = reader.peek_char().unwrap_or('?');
                            if next_char.is_whitespace() {
                                return Err(
                                    "':' must be glued to its operand at a type position; \
                                     write `name :Type` (no space after `:`) or `:(List ...)`"
                                        .to_string(),
                                );
                            }
                            return Err(format!(
                                "':' must be followed by a type name (uppercase-leading) or `(`; \
                                 got `{next_char}`"
                            ));
                        }
                        None => {
                            return Err(
                                "trailing ':' at end of input; expected a type name or `(`"
                                    .to_string(),
                            );
                        }
                    }
                }
            }
            // Pair separator in a dict; whitespace-equivalent (no-op) everywhere else, so
            // type-annotation triples like `a: Number, b: Str` parse without altering shape.
            ',' => {
                flush_token(&mut stack, &mut buf)?;
                reader.advance_byte();
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
                reader.advance_byte();
            }
            '<' | '>' => {
                flush_token(&mut stack, &mut buf)?;
                stack.push_part(ExpressionPart::Keyword(c.to_string()));
                reader.advance_byte();
            }
            // Quote literals: `mask_quotes` has rewritten the content into either an
            // empty pair (no marker) or a `LITERAL_MARK <idx> LEN_SEP <len>` placeholder
            // followed by the closing quote and a `JUMP_MARK <past_close> JUMP_MARK`
            // anchor. Phase 2 consumes the marker payloads and discards them; only
            // `idx` is used to look up the literal text in `quotes`.
            '\'' | '"' => {
                flush_token(&mut stack, &mut buf)?;
                let open_byte = b;
                reader.advance_byte();
                match reader.peek_byte() {
                    Some(byte) if byte == open_byte => {
                        // Empty literal `''` / `""`.
                        reader.advance_byte();
                        stack.push_part(ExpressionPart::Literal(KLiteral::String(String::new())));
                    }
                    Some(LITERAL_MARK) => {
                        let (idx, _orig_byte_len) = reader.read_literal_marker()?;
                        match reader.peek_byte() {
                            Some(byte) if byte == open_byte => reader.advance_byte(),
                            _ => return Err(format!("unclosed quote: {}", open_byte as char)),
                        }
                        if reader.peek_byte() == Some(JUMP_MARK) {
                            reader.read_jump()?;
                        }
                        let literal = quotes.get(&idx).cloned().ok_or_else(|| {
                            format!("unknown literal placeholder index: {idx}")
                        })?;
                        stack.push_part(ExpressionPart::Literal(KLiteral::String(literal)));
                    }
                    _ => return Err(format!("unclosed quote: {}", open_byte as char)),
                }
            }
            c if c.is_whitespace() => {
                flush_token(&mut stack, &mut buf)?;
                reader.advance_codepoint();
            }
            _ => {
                let consumed = reader.advance_codepoint();
                buf.push(consumed);
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
    while expr.parts.len() == 1 && matches!(expr.parts[0].value, ExpressionPart::Expression(_)) {
        if let Some(Spanned { value: ExpressionPart::Expression(inner), .. }) = expr.parts.pop() {
            expr = *inner;
        }
    }
    expr.parts = expr.parts.into_iter().map(peel_spanned).collect();
    expr
}

fn peel_spanned<'a>(part: Spanned<ExpressionPart<'a>>) -> Spanned<ExpressionPart<'a>> {
    Spanned { value: peel_part(part.value), span: part.span }
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
        .map(|part| match part.value {
            ExpressionPart::Expression(e) => Ok(peel_redundant(*e)),
            other => Err(format!("unexpected top-level part: {:?}", other)),
        })
        .collect()
}

#[cfg(test)]
mod tests;
