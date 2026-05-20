//! Second parse pass: collapse indentation-based blocks into parenthesized form. Reads the
//! masked **byte** output of `quotes` (which is valid UTF-8 — see `quotes` for the marker
//! encoding) and produces a paren-structured **byte** stream consumed by
//! `expression_tree::build_tree`.
//!
//! Phase 3: the pass now tracks an `orig_cursor` (the byte offset into the *original* source)
//! while walking each line, and emits its own `JUMP_MARK <offset> JUMP_MARK` anchors around
//! every synthetic character it inserts (`(` at line open, `)` at dedent / EOF, and the
//! joining space on continuation lines), plus around the sigil byte on sigil-led lines. The
//! masked-stream cursor stays aligned with original byte offsets because mask_quotes already
//! emits a JUMP after each literal placeholder. `build_tree` still consumes-and-ignores all
//! JUMP payloads in Phase 3; Phase 4 will read them to populate `KExpression::span`.
//!
//! See [design/expressions-and-parsing.md](../../design/expressions-and-parsing.md).

use crate::machine::KError;
use crate::machine::core::source::Span;
use crate::parse::quotes::{JUMP_MARK, LEN_SEP, LITERAL_MARK};

/// Each non-blank line becomes a `(...)` group; deeper indents nest, dedents close. Tabs and
/// odd-numbered space indentation are rejected.
///
/// **Collection-literal continuations.** When `[`/`{` opens but its match is on a later line,
/// intermediate lines are appended to the open span instead of being wrapped — `build_tree`
/// pairs the brackets. A single delta counter conflates `[`/`{`; `build_tree`'s frame stack
/// catches cross-pairing like `[1 2}`. Strings are already masked, so brackets inside them
/// don't reach this function.
///
/// **Trailing-comma continuations.** A line ending in `,` suspends indentation through the
/// next non-blank line, so `UNION Maybe = (some: Number,\n  none: Null)` parses as one
/// expression. Parens (`(`) are intentionally *not* tracked the same way — they already wrap
/// sub-expressions inside indent-structured blocks, so suspending on them would change the
/// meaning of existing programs. Blank lines preserve the continuation flag.
///
/// **Cursor anchors (Phase 3).** Every synthetic character inserted by this pass is preceded
/// by a `JUMP <orig_offset> JUMP` marker so `build_tree`'s downstream cursor can recover the
/// original byte offset after the collapse pass strips newlines and inserts synthetic
/// whitespace. Sigil-led lines (`#3` → `#(3)`) also get a JUMP before the sigil byte so the
/// real `#` / `$` keeps its original offset and the synthetic `(` snaps to the offset of the
/// first byte of the rest of the line.
pub fn collapse_whitespace(input: &[u8]) -> Result<Vec<u8>, KError> {
    let s = std::str::from_utf8(input)
        .map_err(|_| KError::parse("collapse_whitespace expected UTF-8 input", None))?;
    collapse_str(s)
}

fn collapse_str(input: &str) -> Result<Vec<u8>, KError> {
    let mut out: Vec<u8> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut delim_depth: i32 = 0;
    let mut continuing: bool = false;
    let mut last_content_orig_end: u32 = 0;

    let bytes = input.as_bytes();
    let mut line_start: usize = 0;
    let mut lineno: usize = 0;
    let mut orig_at_line_start: u32 = 0;

    while line_start <= bytes.len() {
        let line_end = bytes[line_start..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|n| line_start + n)
            .unwrap_or(bytes.len());

        let raw = &input[line_start..line_end];
        let stripped = raw.trim_start();
        let indent = raw.len() - stripped.len();
        let content = stripped.trim_end();

        let nl_advance: u32 = if line_end < bytes.len() { 1 } else { 0 };

        if content.is_empty() {
            // Pure-whitespace lines carry no markers (markers can only appear inside literal
            // sequences, which begin with a non-whitespace opening quote), so cursor advance
            // is just byte-count + the trailing newline if present.
            let line_total = (line_end - line_start) as u32 + nl_advance;
            orig_at_line_start += line_total;
            line_start = if line_end < bytes.len() {
                line_end + 1
            } else {
                bytes.len() + 1
            };
            lineno += 1;
            continue;
        }

        let orig_at_content_start = orig_at_line_start + indent as u32;

        let content_bytes = &bytes[line_start + indent..line_start + indent + content.len()];
        let orig_at_content_end = walk_content_cursor(content_bytes, orig_at_content_start)?;

        let trailing_ws_len = (line_end - line_start - indent - content.len()) as u32;
        let orig_at_next_line_start = orig_at_content_end + trailing_ws_len + nl_advance;

        if delim_depth > 0 || continuing {
            // Continuation: synthetic joining space (preceded by JUMP) + verbatim content.
            // The trailing comma / open bracket carried us here; we don't open a new frame.
            emit_jump(&mut out, orig_at_content_start);
            out.push(b' ');
            out.extend_from_slice(content.as_bytes());
            delim_depth += line_delim_delta(content);
            continuing = content.ends_with(',');
            last_content_orig_end = orig_at_content_end;
            orig_at_line_start = orig_at_next_line_start;
            line_start = if line_end < bytes.len() {
                line_end + 1
            } else {
                bytes.len() + 1
            };
            lineno += 1;
            continue;
        }

        if let Some(tab_pos) = raw.as_bytes()[..indent].iter().position(|&b| b == b'\t') {
            let off = orig_at_line_start + tab_pos as u32;
            return Err(KError::parse(
                format!("tab indentation not allowed on line {}", lineno + 1),
                Some(Span { start: off, end: off + 1 }),
            ));
        }
        if !indent.is_multiple_of(2) {
            let off = orig_at_line_start;
            return Err(KError::parse(
                format!("odd-numbered space indentation on line {}", lineno + 1),
                Some(Span { start: off, end: off + indent as u32 }),
            ));
        }

        while let Some(&top) = stack.last() {
            if top >= indent {
                stack.pop();
                emit_jump(&mut out, last_content_orig_end);
                out.push(b')');
            } else {
                break;
            }
        }

        // Sibling separator: a plain space between groups. The adjacent JUMP that follows
        // (for the next line's `(` opener) snaps the cursor, so this space needs no anchor.
        if !out.is_empty() {
            out.push(b' ');
        }

        // Sigil-led lines place the wrapping paren *after* the sigil (`#3` → `#(3)`, not
        // `(#3)`); `expression_tree` rejects a sigil adjacent to a non-paren inside a group.
        // The sigil byte is *real* content, so a JUMP precedes it (step 1 of the design's
        // sigil-line recipe) and a second JUMP precedes the synthetic `(`.
        let first_byte = content.as_bytes()[0];
        if first_byte == b'#' || first_byte == b'$' {
            emit_jump(&mut out, orig_at_content_start);
            out.push(first_byte);
            emit_jump(&mut out, orig_at_content_start + 1);
            out.push(b'(');
            out.extend_from_slice(&content.as_bytes()[1..]);
        } else {
            emit_jump(&mut out, orig_at_content_start);
            out.push(b'(');
            out.extend_from_slice(content.as_bytes());
        }

        stack.push(indent);
        delim_depth += line_delim_delta(content);
        continuing = content.ends_with(',');
        last_content_orig_end = orig_at_content_end;

        orig_at_line_start = orig_at_next_line_start;
        line_start = if line_end < bytes.len() {
            line_end + 1
        } else {
            bytes.len() + 1
        };
        lineno += 1;
    }

    while stack.pop().is_some() {
        emit_jump(&mut out, last_content_orig_end);
        out.push(b')');
    }

    Ok(out)
}

fn emit_jump(out: &mut Vec<u8>, offset: u32) {
    out.push(JUMP_MARK);
    out.extend_from_slice(offset.to_string().as_bytes());
    out.push(JUMP_MARK);
}

/// Walk a slice of content bytes, returning the original-source byte offset just past the
/// last byte. LITERAL markers leave the cursor unchanged (the following JUMP from
/// `mask_quotes` re-aligns it); JUMP markers snap the cursor to their payload; everything
/// else is a verbatim byte that advances the cursor by 1.
fn walk_content_cursor(content: &[u8], start_orig: u32) -> Result<u32, KError> {
    let mut orig = start_orig;
    let mut i = 0;
    while i < content.len() {
        let b = content[i];
        if b == JUMP_MARK {
            let mut j = i + 1;
            let digits_start = j;
            while j < content.len() && content[j].is_ascii_digit() {
                j += 1;
            }
            if j == digits_start {
                return Err(KError::parse("JUMP marker: empty payload", None));
            }
            if j >= content.len() || content[j] != JUMP_MARK {
                return Err(KError::parse("JUMP marker missing closing sentinel", None));
            }
            orig = std::str::from_utf8(&content[digits_start..j])
                .ok()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| KError::parse("JUMP marker: invalid payload", None))?;
            i = j + 1;
        } else if b == LITERAL_MARK {
            let mut j = i + 1;
            while j < content.len() && content[j].is_ascii_digit() {
                j += 1;
            }
            if j >= content.len() || content[j] != LEN_SEP {
                return Err(KError::parse("LITERAL marker missing LEN_SEP", None));
            }
            j += 1;
            while j < content.len() && content[j].is_ascii_digit() {
                j += 1;
            }
            i = j;
        } else {
            orig += 1;
            i += 1;
        }
    }
    Ok(orig)
}

/// Net `[`+`{` − `]`+`}` on a single (post-mask, post-trim) line. The two bracket families
/// are conflated intentionally — this only decides whether we're inside an open span;
/// `build_tree`'s frame stack enforces `[`/`]` and `{`/`}` pairing.
fn line_delim_delta(s: &str) -> i32 {
    let opens = s.chars().filter(|&c| c == '[' || c == '{').count() as i32;
    let closes = s.chars().filter(|&c| c == ']' || c == '}').count() as i32;
    opens - closes
}

#[cfg(test)]
mod tests;
