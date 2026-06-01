//! Second parse pass: collapse indentation-based blocks into parenthesized form. Reads the
//! masked **byte** output of `quotes` and produces a paren-structured **byte** stream
//! consumed by `expression_tree::build_tree`.
//!
//! Every synthetic character this pass inserts (`(` at line open, `)` at dedent/EOF, the
//! joining space on continuation lines, and the sigil byte on sigil-led lines) is preceded
//! by a `JUMP_MARK <offset> JUMP_MARK` anchor so downstream span recovery can map back to
//! the original source byte offset after newlines are stripped.
//!
//! See [design/expressions-and-parsing.md](../../design/expressions-and-parsing.md).

use crate::machine::core::source::Span;
use crate::machine::KError;
use crate::parse::quotes::{JUMP_MARK, LEN_SEP, LITERAL_MARK};

/// Each non-blank line becomes a `(...)` group; deeper indents nest, dedents close. Tabs and
/// odd-numbered space indentation are rejected.
///
/// Three continuation regimes suspend the line-becomes-group rule:
///
/// - **`[`/`{` open span.** Intermediate lines append verbatim until the match closes; a
///   single delta counter conflates the families and `build_tree` catches cross-pairing.
/// - **Trailing comma.** A line ending in `,` joins the next non-blank line flat. Blank
///   lines preserve the flag.
/// - **Open `(`.** Indentation-sensitive: a deeper line nests as its own wrapped group
///   (nest-per-line); a same-or-shallower non-closing line is the dangling-`(` error; the
///   matching `)` may sit at any indent >= its opener. Closing joins lazily so `build_tree`
///   pairs the literal `)` with the innermost open group. Parens can't ride `delim_depth`
///   because that counter ignores indent; an open-paren anchor stack keyed by `(indent,
///   span)` carries the info instead.
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
    // One `(indent, span)` per still-open `(`, innermost last; span feeds the dangling-`(`
    // diagnostic.
    let mut paren_anchors: Vec<(usize, Span)> = Vec::new();

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
            // Pure-whitespace lines carry no markers (those only appear inside literal
            // sequences, which begin with a non-whitespace opening quote), so the cursor
            // advances by raw byte count.
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

        let paren_delta = line_paren_delta(content);
        let content_span = Span {
            start: orig_at_content_start,
            end: orig_at_content_end,
        };

        macro_rules! join_line {
            () => {{
                emit_jump(&mut out, orig_at_content_start);
                out.push(b' ');
                out.extend_from_slice(content.as_bytes());
                delim_depth += line_delim_delta(content);
                adjust_parens(&mut paren_anchors, paren_delta, indent, content_span);
                continuing = content.ends_with(',');
                last_content_orig_end = orig_at_content_end;
                orig_at_line_start = orig_at_next_line_start;
                line_start = if line_end < bytes.len() {
                    line_end + 1
                } else {
                    bytes.len() + 1
                };
                lineno += 1;
            }};
        }

        // Flat-continuation regimes: append verbatim regardless of indentation. Parens on
        // the line still adjust the anchor stack so a later indent-governed line sees the
        // right depth.
        if delim_depth > 0 || continuing {
            join_line!();
            continue;
        }

        // Inside an open paren, indentation decides continuation vs. break.
        if let Some(&(anchor_indent, anchor_span)) = paren_anchors.last() {
            if paren_delta < 0 {
                // Lazy join: let `build_tree` pair the literal `)` with the innermost open
                // group instead of forcing a synthetic-frame pop here.
                if indent < anchor_indent {
                    return Err(KError::parse(
                        "closing ')' is less indented than the '(' it closes; a paren must \
                         close at the same or greater indentation as its opener.",
                        Some(content_span),
                    ));
                }
                join_line!();
                continue;
            }
            if indent <= anchor_indent {
                // Surface the dangling-`(` here so the user sees the opener span instead of
                // a downstream "dispatch failed for :".
                return Err(KError::parse(
                    "unmatched '(': an open paren must close before an expression break (a \
                     line at the same or lesser indentation). Indent the continuation deeper, \
                     or close the paren before breaking the line.",
                    Some(anchor_span),
                ));
            }
        }

        if let Some(tab_pos) = raw.as_bytes()[..indent].iter().position(|&b| b == b'\t') {
            let off = orig_at_line_start + tab_pos as u32;
            return Err(KError::parse(
                format!("tab indentation not allowed on line {}", lineno + 1),
                Some(Span {
                    start: off,
                    end: off + 1,
                }),
            ));
        }
        if !indent.is_multiple_of(2) {
            let off = orig_at_line_start;
            return Err(KError::parse(
                format!("odd-numbered space indentation on line {}", lineno + 1),
                Some(Span {
                    start: off,
                    end: off + indent as u32,
                }),
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

        // Sibling separator. The next line's own JUMP snaps the cursor, so this space
        // needs no anchor of its own.
        if !out.is_empty() {
            out.push(b' ');
        }

        // Sigil-led lines wrap *inside* the sigil (`#3` → `#(3)`, not `(#3)`) so the result
        // satisfies `expression_tree`'s sigil-adjacency rule.
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
        adjust_parens(&mut paren_anchors, paren_delta, indent, content_span);
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

/// Return the original-source byte offset just past the last byte of `content`. LITERAL
/// markers leave the cursor unchanged (the following JUMP from `mask_quotes` re-aligns it);
/// JUMP markers snap the cursor to their payload; verbatim bytes advance by 1.
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

/// Conflated `[`+`{` − `]`+`}` delta; this pass only needs the open/closed bit and
/// `build_tree`'s frame stack enforces per-family pairing.
fn line_delim_delta(s: &str) -> i32 {
    let opens = s.chars().filter(|&c| c == '[' || c == '{').count() as i32;
    let closes = s.chars().filter(|&c| c == ']' || c == '}').count() as i32;
    opens - closes
}

fn line_paren_delta(s: &str) -> i32 {
    let opens = s.chars().filter(|&c| c == '(').count() as i32;
    let closes = s.chars().filter(|&c| c == ')').count() as i32;
    opens - closes
}

/// Pop is clamped to len; a genuinely unbalanced stream is rejected by `build_tree`.
fn adjust_parens(anchors: &mut Vec<(usize, Span)>, delta: i32, indent: usize, span: Span) {
    if delta > 0 {
        for _ in 0..delta {
            anchors.push((indent, span));
        }
    } else {
        for _ in 0..(-delta).min(anchors.len() as i32) {
            anchors.pop();
        }
    }
}

#[cfg(test)]
mod tests;
