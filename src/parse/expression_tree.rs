//! Top-level parser: runs the `quote-mask → whitespace-collapse → tokenize →
//! tree-build` pipeline and returns a `KExpression`. The parse-stack and frame
//! abstractions live in sibling modules ([`super::parse_stack`], [`super::frame`]);
//! dict-pair state lives on [`super::dict_literal::DictFrame`] and type-expression
//! folding lives on [`super::type_expr_frame::TypeExprFrame`] so framing arms here stay
//! one-liners.
//!
//! Phase 4: `Reader` now tracks an original-source `cursor: u32` alongside its byte
//! position in the masked stream. JUMP markers snap the cursor; LITERAL markers leave it
//! untouched (the JUMP that follows every literal re-aligns it). Verbatim bytes advance
//! the cursor by their UTF-8 width unless the byte is synthetic — i.e. immediately
//! preceded by a JUMP — in which case the JUMP already set the cursor and the synthetic
//! byte is shadow-positioned at the same offset. Frame open/close and token-start arms
//! record cursor snapshots so spans land on `KExpression`, the wrapping `Spanned`, and
//! the structural keyword pushes. `classify_token` carries the token-start offset down to
//! atom-level so sub-atoms (ATTR, NOT, TRY triggers) get 1-codepoint trigger spans inside
//! a token-wide wrapper.
//!
//! See [design/expressions-and-parsing.md](../../design/expressions-and-parsing.md).

use std::collections::HashMap;

use crate::machine::core::source::{Span, Spanned};
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

/// Like `decode_char_at`, but transparently skips over `JUMP_MARK <digits> JUMP_MARK`
/// runs. The collapse pass plants JUMPs immediately after `]`/`}` (the close-adjacency
/// check below has to look past them to find the real next token), and similar gaps
/// appear at line / dedent boundaries. LITERAL markers are *not* skipped — a literal
/// glued to a closing bracket is still an adjacency violation.
fn peek_char_past_jumps(bytes: &[u8], pos: usize) -> Option<char> {
    let mut p = pos;
    while let Some(&b) = bytes.get(p) {
        if b != JUMP_MARK {
            return decode_char_at(bytes, p);
        }
        p += 1;
        while let Some(&d) = bytes.get(p) {
            if d == JUMP_MARK {
                break;
            }
            p += 1;
        }
        if bytes.get(p) == Some(&JUMP_MARK) {
            p += 1;
        }
    }
    None
}

/// Hand-rolled byte cursor over the masked stream. Maintains both stream `pos` and
/// original-source `cursor: u32`. The `just_jumped` flag flips on every JUMP and is
/// consumed by the next byte-advance: that next byte is "synthetic" (collapse-inserted
/// or post-mask alignment), so it shouldn't advance the cursor — the JUMP already set
/// it to the next real-content offset. Subsequent verbatim bytes advance the cursor
/// normally.
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
    cursor: u32,
    just_jumped: bool,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0, cursor: 0, just_jumped: false }
    }

    fn peek_byte(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn peek_char(&self) -> Option<char> {
        decode_char_at(self.bytes, self.pos)
    }

    /// Consume one byte. The cursor advances by 1 for verbatim bytes; for the byte
    /// immediately after a JUMP the advance is suppressed (the byte is synthetic and
    /// the JUMP already snapped cursor to the next real offset).
    fn advance_byte(&mut self) {
        self.pos += 1;
        if self.just_jumped {
            self.just_jumped = false;
        } else {
            self.cursor += 1;
        }
    }

    /// Decode the codepoint at `pos` and advance `pos` by its UTF-8 width. The cursor
    /// advances by the same width unless this is the byte immediately after a JUMP.
    fn advance_codepoint(&mut self) -> char {
        let b = self.bytes[self.pos];
        let width = utf8_width(b);
        let c = decode_char_at(self.bytes, self.pos)
            .expect("masked stream must be valid UTF-8");
        self.pos += width;
        if self.just_jumped {
            self.just_jumped = false;
        } else {
            self.cursor += width as u32;
        }
        c
    }

    /// Consume a `\x1D<digits>\x1D` JUMP marker. The leading sentinel must already be
    /// at the cursor. Snaps `cursor` to the parsed offset and flags `just_jumped` so the
    /// next byte-advance doesn't double-count.
    fn read_jump(&mut self) -> Result<u32, String> {
        debug_assert_eq!(self.peek_byte(), Some(JUMP_MARK));
        self.pos += 1;
        let value = self.read_decimal(JUMP_MARK, "JUMP marker")?;
        if self.peek_byte() != Some(JUMP_MARK) {
            return Err("JUMP marker missing closing sentinel".to_string());
        }
        self.pos += 1;
        self.cursor = value;
        self.just_jumped = true;
        Ok(value)
    }

    /// Consume a `\x1F<idx>\x1E<orig_byte_len>` LITERAL marker. The leading sentinel
    /// must already be at the cursor. Returns `(idx, orig_byte_len)`; the cursor is
    /// *not* advanced — the JUMP that mask_quotes always emits after a literal
    /// re-aligns it past the closing quote.
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
    let mut token_start: Option<u32> = None;
    let mut reader = Reader::new(masked);
    let mut prev: Option<char> = None;
    // (sigil char, cursor of the sigil byte).
    let mut pending_sigil: Option<(char, u32)> = None;
    // Cursor of the leading `:` that opened a `:(` type-expression group.
    let mut pending_type_paren_cursor: Option<u32> = None;

    loop {
        // Drain JUMP markers at the top of the loop so the dispatch never has to
        // think about them. Each JUMP snaps `reader.cursor` to the payload offset
        // and flips `just_jumped` so the next byte-advance doesn't double-count.
        while reader.peek_byte() == Some(JUMP_MARK) {
            reader.read_jump()?;
        }
        let Some(b) = reader.peek_byte() else { break };

        let c = reader
            .peek_char()
            .ok_or_else(|| "malformed UTF-8 in masked stream".to_string())?;

        if let Some((s, _)) = pending_sigil {
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
                let sigil_cursor = reader.cursor;
                reader.advance_byte();
                pending_sigil = Some((c, sigil_cursor));
            }
            '(' => {
                flush_token(&mut stack, &mut buf, &mut token_start)?;
                let span_start = reader.cursor;
                reader.advance_byte();
                if let Some(type_start) = pending_type_paren_cursor.take() {
                    stack.push_frame(Frame::TypeExpr {
                        tef: TypeExprFrame::new(),
                        span_start: type_start,
                    });
                } else {
                    let (head, sigil_cursor) = match pending_sigil.take() {
                        Some(('#', sc)) => (Some("QUOTE"), Some(sc)),
                        Some(('$', sc)) => (Some("EVAL"), Some(sc)),
                        _ => (None, None),
                    };
                    stack.push_frame(Frame::Expression {
                        expr: KExpression::new(Vec::new()),
                        head,
                        span_start,
                        sigil_cursor,
                    });
                }
            }
            ')' => {
                flush_token(&mut stack, &mut buf, &mut token_start)?;
                reader.advance_byte();
                let end = reader.cursor;
                let frame = stack
                    .pop_top()
                    .ok_or_else(|| "closed paren without matching open paren".to_string())?;
                stack.push_part(close_paren_to_part(frame, end)?);
            }
            '[' => {
                let span_start = reader.cursor;
                open_collection(
                    &mut stack,
                    &mut buf,
                    '[',
                    prev,
                    Frame::List { items: Vec::new(), span_start },
                    &mut token_start,
                )?;
                reader.advance_byte();
            }
            ']' => {
                reader.advance_byte();
                let end = reader.cursor;
                let next = peek_char_past_jumps(reader.bytes, reader.pos);
                close_collection(
                    &mut stack,
                    &mut buf,
                    ']',
                    next,
                    "closed bracket without matching open bracket",
                    &mut token_start,
                    end,
                )?;
            }
            '{' => {
                let span_start = reader.cursor;
                open_collection(
                    &mut stack,
                    &mut buf,
                    '{',
                    prev,
                    Frame::Dict { dict: DictFrame::new(), span_start },
                    &mut token_start,
                )?;
                reader.advance_byte();
            }
            '}' => {
                reader.advance_byte();
                let end = reader.cursor;
                let next = peek_char_past_jumps(reader.bytes, reader.pos);
                close_collection(
                    &mut stack,
                    &mut buf,
                    '}',
                    next,
                    "closed brace without matching open brace",
                    &mut token_start,
                    end,
                )?;
            }
            // Dispatch order: dict-pair separator first (state-machine has its own rules),
            // then `:|` / `:!` ascription operators (assembled here because `!` is itself a
            // prefix operator and would never reach keyword classification), then the
            // glued-right type sigils `:(` and `:T`. A lone `:` outside a dict (with
            // whitespace after or at EOF) is a parse error — type-position annotation must
            // glue the sigil to its operand.
            ':' => {
                flush_token(&mut stack, &mut buf, &mut token_start)?;
                let colon_cursor = reader.cursor;
                reader.advance_byte();
                if let Some(d) = stack.top_dict_mut() {
                    d.accept_colon()?;
                } else {
                    match reader.peek_byte() {
                        Some(b'|') => {
                            reader.advance_byte();
                            let span = Span { start: colon_cursor, end: reader.cursor };
                            stack.push_part(Spanned::at(
                                ExpressionPart::Keyword(":|".to_string()),
                                span,
                            ));
                            prev = Some('|');
                            continue;
                        }
                        Some(b'!') => {
                            reader.advance_byte();
                            let span = Span { start: colon_cursor, end: reader.cursor };
                            stack.push_part(Spanned::at(
                                ExpressionPart::Keyword(":!".to_string()),
                                span,
                            ));
                            prev = Some('!');
                            continue;
                        }
                        Some(b'(') => {
                            pending_type_paren_cursor = Some(colon_cursor);
                            // Don't consume the '(' — let the next loop iteration's '(' arm
                            // see it and open the TypeExpr frame because the cursor is set.
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
                flush_token(&mut stack, &mut buf, &mut token_start)?;
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
                flush_token(&mut stack, &mut buf, &mut token_start)?;
                let start = reader.cursor;
                reader.advance_byte();
                let span = Span { start, end: reader.cursor };
                stack.push_part(Spanned::at(
                    ExpressionPart::Keyword(c.to_string()),
                    span,
                ));
            }
            // Quote literals: `mask_quotes` has rewritten the content into either an
            // empty pair (no marker) or a `LITERAL_MARK <idx> LEN_SEP <len>` placeholder
            // followed by the closing quote and a `JUMP_MARK <past_close> JUMP_MARK`
            // anchor. The literal-open cursor is captured before any advance; the
            // closing JUMP snaps cursor to one past the original closing quote, which is
            // the correct exclusive end of the span.
            '\'' | '"' => {
                flush_token(&mut stack, &mut buf, &mut token_start)?;
                let open_byte = b;
                let literal_open_cursor = reader.cursor;
                reader.advance_byte();
                match reader.peek_byte() {
                    Some(byte) if byte == open_byte => {
                        // Empty literal `''` / `""`. No marker, no JUMP — both quotes
                        // advance cursor verbatim, so reader.cursor is already past the
                        // closing quote.
                        reader.advance_byte();
                        let span = Span { start: literal_open_cursor, end: reader.cursor };
                        stack.push_part(Spanned::at(
                            ExpressionPart::Literal(KLiteral::String(String::new())),
                            span,
                        ));
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
                        let span = Span { start: literal_open_cursor, end: reader.cursor };
                        stack.push_part(Spanned::at(
                            ExpressionPart::Literal(KLiteral::String(literal)),
                            span,
                        ));
                    }
                    _ => return Err(format!("unclosed quote: {}", open_byte as char)),
                }
            }
            c if c.is_whitespace() => {
                flush_token(&mut stack, &mut buf, &mut token_start)?;
                reader.advance_codepoint();
            }
            _ => {
                if buf.is_empty() {
                    token_start = Some(reader.cursor);
                }
                let consumed = reader.advance_codepoint();
                buf.push(consumed);
            }
        }
        prev = Some(c);
    }
    if let Some((s, _)) = pending_sigil {
        return Err(format!("trailing '{s}' sigil at end of input; expected '('"));
    }
    flush_token(&mut stack, &mut buf, &mut token_start)?;
    stack.finish()
}

/// Collapses single-`Expression` wrappers so `((foo bar))` and `(foo bar)` dispatch the
/// same. Captures the outermost `(span, file)` before the collapse loop and stamps them
/// back onto the final survivor — peeling otherwise leaks the innermost wrapper's span,
/// but users see the outermost region in diagnostics.
fn peel_redundant<'a>(mut expr: KExpression<'a>) -> KExpression<'a> {
    let outer_span = expr.span;
    let outer_file = expr.file;
    while expr.parts.len() == 1 && matches!(expr.parts[0].value, ExpressionPart::Expression(_)) {
        if let Some(Spanned { value: ExpressionPart::Expression(inner), .. }) = expr.parts.pop() {
            expr = *inner;
        }
    }
    expr.parts = expr.parts.into_iter().map(peel_spanned).collect();
    if outer_span.is_some() {
        expr.span = outer_span;
    }
    if outer_file.is_some() {
        expr.file = outer_file;
    }
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
