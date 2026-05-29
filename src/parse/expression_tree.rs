//! Top-level parser: runs the `quote-mask → whitespace-collapse → tokenize →
//! tree-build` pipeline and returns a `KExpression`. Parse-stack and frame
//! abstractions live in sibling modules ([`super::parse_stack`], [`super::frame`]);
//! dict-pair state lives on [`super::dict_literal::DictFrame`]. The `:(...)`
//! type-expression frame ([`Frame::TypeExpr`](super::frame::Frame)) collects its
//! inner expression verbatim — shape recognition is the dispatcher's job.
//!
//! `Reader` tracks an original-source `cursor: u32` alongside its byte position
//! in the masked stream. JUMP markers snap the cursor; LITERAL markers leave it
//! untouched (the trailing JUMP re-aligns it). Verbatim bytes advance the cursor
//! by their UTF-8 width unless the byte is synthetic (immediately after a JUMP).
//!
//! See [design/expressions-and-parsing.md](../../design/expressions-and-parsing.md).

use std::collections::HashMap;

use std::rc::Rc;

use crate::machine::core::source::{self, CurrentFileGuard, FileId, SourceFile, Span, Spanned};
use crate::machine::model::ast::{ExpressionPart, KExpression, KLiteral};
use crate::machine::KError;
use crate::parse::quotes::{mask_quotes, JUMP_MARK, LEN_SEP, LITERAL_MARK};
use crate::parse::whitespace::collapse_whitespace;

use super::dict_literal::DictFrame;
use super::frame::{close_paren_to_part, Frame};
use super::parse_stack::{close_collection, flush_token, open_collection, ParseStack};

/// Width of the UTF-8 codepoint whose leading byte is `b`. Defaults to 1 on a
/// malformed continuation byte so corrupt input terminates rather than spinning.
fn utf8_width(b: u8) -> usize {
    match b {
        _ if b < 0x80 => 1,
        _ if b & 0xE0 == 0xC0 => 2,
        _ if b & 0xF0 == 0xE0 => 3,
        _ if b & 0xF8 == 0xF0 => 4,
        _ => 1,
    }
}

fn decode_char_at(bytes: &[u8], pos: usize) -> Option<char> {
    let b = *bytes.get(pos)?;
    let width = utf8_width(b);
    let slice = bytes.get(pos..pos + width)?;
    std::str::from_utf8(slice).ok()?.chars().next()
}

/// Like `decode_char_at`, but skips over `JUMP_MARK <digits> JUMP_MARK` runs so
/// the close-adjacency check can see past collapse-planted JUMPs to the real
/// next token. LITERAL markers are *not* skipped — a literal glued to a closing
/// bracket is still an adjacency violation.
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

/// Byte cursor over the masked stream tracking both stream `pos` and
/// original-source `cursor: u32`. `just_jumped` flips on every JUMP and is
/// consumed by the next byte-advance — that byte is synthetic so the JUMP
/// already snapped cursor to the next real offset.
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

    fn advance_byte(&mut self) {
        self.pos += 1;
        if self.just_jumped {
            self.just_jumped = false;
        } else {
            self.cursor += 1;
        }
    }

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

    /// Consume a `\x1D<digits>\x1D` JUMP marker; snap `cursor` to the parsed
    /// offset and set `just_jumped` so the next byte-advance doesn't double-count.
    fn read_jump(&mut self) -> Result<u32, KError> {
        debug_assert_eq!(self.peek_byte(), Some(JUMP_MARK));
        self.pos += 1;
        let value = self.read_decimal(JUMP_MARK, "JUMP marker")?;
        if self.peek_byte() != Some(JUMP_MARK) {
            return Err(KError::parse("JUMP marker missing closing sentinel", None));
        }
        self.pos += 1;
        self.cursor = value;
        self.just_jumped = true;
        Ok(value)
    }

    /// Consume a `\x1F<idx>\x1E<orig_byte_len>` LITERAL marker. The cursor is
    /// *not* advanced — the JUMP that `mask_quotes` emits after every literal
    /// re-aligns it past the closing quote.
    fn read_literal_marker(&mut self) -> Result<(usize, u32), KError> {
        debug_assert_eq!(self.peek_byte(), Some(LITERAL_MARK));
        self.pos += 1;
        let idx = self.read_decimal(LEN_SEP, "LITERAL marker idx")?;
        if self.peek_byte() != Some(LEN_SEP) {
            return Err(KError::parse("LITERAL marker missing length separator", None));
        }
        self.pos += 1;
        let len = self.read_decimal_until_non_digit("LITERAL marker length")?;
        Ok((idx as usize, len))
    }

    /// Read ASCII decimal digits up to (but not consuming) `stop`.
    fn read_decimal(&mut self, stop: u8, label: &str) -> Result<u32, KError> {
        let start = self.pos;
        while let Some(b) = self.peek_byte() {
            if b == stop {
                break;
            }
            if !b.is_ascii_digit() {
                return Err(KError::parse(
                    format!("{label}: non-digit byte {b:#x} in payload"),
                    None,
                ));
            }
            self.pos += 1;
        }
        let digits = &self.bytes[start..self.pos];
        if digits.is_empty() {
            return Err(KError::parse(format!("{label}: empty decimal payload"), None));
        }
        std::str::from_utf8(digits)
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .ok_or_else(|| KError::parse(format!("{label}: invalid decimal payload"), None))
    }

    /// Read ASCII decimal digits until the next non-digit byte (or EOF).
    fn read_decimal_until_non_digit(&mut self, label: &str) -> Result<u32, KError> {
        let start = self.pos;
        while let Some(b) = self.peek_byte() {
            if !b.is_ascii_digit() {
                break;
            }
            self.pos += 1;
        }
        let digits = &self.bytes[start..self.pos];
        if digits.is_empty() {
            return Err(KError::parse(format!("{label}: empty decimal payload"), None));
        }
        std::str::from_utf8(digits)
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .ok_or_else(|| KError::parse(format!("{label}: invalid decimal payload"), None))
    }
}

pub fn build_tree<'a>(
    masked: &[u8],
    quotes: &HashMap<usize, String>,
) -> Result<KExpression<'a>, KError> {
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
        // Drain JUMPs up-front so dispatch arms never see them.
        while reader.peek_byte() == Some(JUMP_MARK) {
            reader.read_jump()?;
        }
        let Some(b) = reader.peek_byte() else { break };

        let c = reader
            .peek_char()
            .ok_or_else(|| KError::parse("malformed UTF-8 in masked stream", None))?;

        if let Some((s, _)) = pending_sigil {
            if c != '(' {
                return Err(KError::parse(
                    format!("expected '(' after '{s}', found '{c}'"),
                    None,
                ));
            }
        }

        match c {
            '#' | '$' => {
                if !buf.is_empty() {
                    return Err(KError::parse(
                        format!(
                            "'{c}' sigil must be preceded by whitespace or '(' (got token char {prev:?})"
                        ),
                        None,
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
                        expr: KExpression::new(Vec::new()),
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
                let frame = stack.pop_top().ok_or_else(|| {
                    KError::parse("closed paren without matching open paren", None)
                })?;
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
            // Dict-pair separator first; then `:|` / `:!` (assembled here because
            // `!` is itself a prefix operator and would never reach keyword
            // classification); then glued-right type sigils `:(` / `:T`. A lone
            // `:` outside a dict is a parse error — annotation must glue to its
            // operand.
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
                            // Leave the '(' for the next iteration; the '(' arm
                            // sees `pending_type_paren_cursor` and opens a
                            // TypeExpr frame.
                            pending_type_paren_cursor = Some(colon_cursor);
                        }
                        Some(byte) if byte.is_ascii_uppercase() => {
                            // Glued `:T`: the regular tokenizer turns the
                            // following uppercase-leading token into a Type.
                        }
                        Some(_) => {
                            let next_char = reader.peek_char().unwrap_or('?');
                            if next_char.is_whitespace() {
                                return Err(KError::parse(
                                    "':' must be glued to its operand at a type position; \
                                     write `name :Type` (no space after `:`) or `:(List ...)`",
                                    None,
                                ));
                            }
                            return Err(KError::parse(
                                format!(
                                    "':' must be followed by a type name (uppercase-leading) or `(`; \
                                     got `{next_char}`"
                                ),
                                None,
                            ));
                        }
                        None => {
                            return Err(KError::parse(
                                "trailing ':' at end of input; expected a type name or `(`",
                                None,
                            ));
                        }
                    }
                }
            }
            // Dict pair separator; whitespace-equivalent elsewhere so
            // annotation triples like `a: Number, b: Str` parse cleanly.
            ',' => {
                flush_token(&mut stack, &mut buf, &mut token_start)?;
                reader.advance_byte();
                if let Some(d) = stack.top_dict_mut() {
                    d.accept_comma()?;
                }
            }
            // `<` / `>` emit standalone `Keyword` parts. The `prev == Some('-')`
            // carve-out keeps `->` contiguous so the arrow survives as one token.
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
            // `mask_quotes` rewrote the body as either an empty pair or
            // `LITERAL_MARK <idx> LEN_SEP <len>` + closing quote + trailing JUMP.
            // The trailing JUMP snaps cursor to one past the original closing
            // quote — the correct exclusive span end.
            '\'' | '"' => {
                flush_token(&mut stack, &mut buf, &mut token_start)?;
                let open_byte = b;
                let literal_open_cursor = reader.cursor;
                reader.advance_byte();
                match reader.peek_byte() {
                    Some(byte) if byte == open_byte => {
                        // Empty literal: no marker, no JUMP — both quotes
                        // advanced the cursor verbatim.
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
                            _ => {
                                return Err(KError::parse(
                                    format!("unclosed quote: {}", open_byte as char),
                                    None,
                                ));
                            }
                        }
                        if reader.peek_byte() == Some(JUMP_MARK) {
                            reader.read_jump()?;
                        }
                        let literal = quotes.get(&idx).cloned().ok_or_else(|| {
                            KError::parse(format!("unknown literal placeholder index: {idx}"), None)
                        })?;
                        let span = Span { start: literal_open_cursor, end: reader.cursor };
                        stack.push_part(Spanned::at(
                            ExpressionPart::Literal(KLiteral::String(literal)),
                            span,
                        ));
                    }
                    _ => {
                        return Err(KError::parse(
                            format!("unclosed quote: {}", open_byte as char),
                            None,
                        ));
                    }
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
        return Err(KError::parse(
            format!("trailing '{s}' sigil at end of input; expected '('"),
            None,
        ));
    }
    flush_token(&mut stack, &mut buf, &mut token_start)?;
    stack.finish()
}

/// Collapses single-`Expression` wrappers so `((foo bar))` and `(foo bar)`
/// dispatch the same. The outermost `(span, file)` is re-stamped onto the
/// survivor so diagnostics point at the user-visible region, not the innermost
/// wrapper.
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

/// Returns one `KExpression` per top-level line, registering the input under
/// the synthetic path `<input>`. Use [`parse_with_path`] to supply a real path.
pub fn parse<'a>(input: &str) -> Result<Vec<KExpression<'a>>, KError> {
    parse_with_path(input, "<input>")
}

/// [`parse`] variant that registers the source under a caller-supplied `path`
/// so error frames render real filenames.
pub fn parse_with_path<'a>(
    input: &str,
    path: impl Into<Rc<str>>,
) -> Result<Vec<KExpression<'a>>, KError> {
    let id = source::register(SourceFile::new(path, input.to_string()));
    parse_with_source(id)
}

/// Parse against a pre-registered `SourceFile`. Installs `id` as the active
/// `CURRENT_FILE` via [`CurrentFileGuard`] so `KError::parse` sees the right
/// file.
pub fn parse_with_source<'a>(id: FileId) -> Result<Vec<KExpression<'a>>, KError> {
    let _guard = CurrentFileGuard::push(id);
    let (masked, quotes) = source::with(id, |f| mask_quotes(&f.text));
    let collapsed = collapse_whitespace(&masked)?;
    let root = build_tree(&collapsed, &quotes)?;
    root.parts
        .into_iter()
        .map(|part| match part.value {
            ExpressionPart::Expression(e) => Ok(peel_redundant(*e)),
            other => Err(KError::parse(
                format!("unexpected top-level part: {:?}", other),
                None,
            )),
        })
        .collect()
}

#[cfg(test)]
mod tests;
