//! First parse pass: replace quoted-string contents with in-band placeholder markers so
//! later tokenization doesn't re-interpret characters inside string literals. Produces a
//! masked **byte** stream plus a lookup table; `expression_tree::build_tree` reads the
//! table back via the index baked into each placeholder.
//!
//! ## Marker encoding
//!
//! Three C0 control bytes (illegal in Koan source, so they can't collide with content)
//! carry metadata through the stream. Payloads are decimal ASCII digits.
//!
//! | Byte | Name           | Form                                          | Meaning                                          |
//! |------|----------------|-----------------------------------------------|--------------------------------------------------|
//! | 0x1F | `LITERAL_MARK` | `\x1F<idx>\x1E<orig_byte_len>`                | Placeholder: resolve via `dict[idx]`.            |
//! | 0x1E | `LEN_SEP`      | inside LITERAL                                | Separator between `<idx>` and `<orig_byte_len>`. |
//! | 0x1D | `JUMP_MARK`    | `\x1D<abs_offset>\x1D`                        | Snap cursor to `<abs_offset>`.                   |
//!
//! Each non-empty literal collapses `'foo'` → `'<LITERAL idx len>'<JUMP past>`. The
//! JUMP-after-literal is the cursor-sync invariant — the only mechanism keeping the
//! downstream cursor aligned with original byte offsets after a literal shortens the
//! stream. Empty literals (`''`) emit two verbatim quote bytes back-to-back with no
//! placeholder and no JUMP; the stream is already aligned.
//!
//! See [design/expressions-and-parsing.md](../../design/expressions-and-parsing.md).

use std::collections::HashMap;

pub const LITERAL_MARK: u8 = 0x1F;
pub const LEN_SEP: u8 = 0x1E;
pub const JUMP_MARK: u8 = 0x1D;

/// Replace each quoted region's contents with a length-bearing placeholder marker and
/// emit a JUMP after the closing quote so downstream cursor math stays aligned with
/// original byte offsets. Returns the masked byte stream plus a dictionary mapping each
/// placeholder index back to the original literal text.
pub fn mask_quotes(input: &str) -> (Vec<u8>, HashMap<usize, String>) {
    let mut out: Vec<u8> = Vec::with_capacity(input.len());
    let mut dict: HashMap<usize, String> = HashMap::new();
    let mut content = String::new();
    // `Some((opening_quote_char, first_content_byte))` while inside a literal.
    let mut quote: Option<(char, usize)> = None;
    let mut prev = '\0';
    let mut next_index: usize = 0;
    let input_bytes = input.as_bytes();

    let mut iter = input.char_indices().peekable();
    while let Some((i, c)) = iter.next() {
        let next_byte = iter.peek().map(|(j, _)| *j).unwrap_or(input.len());
        match quote {
            None => {
                out.extend_from_slice(&input_bytes[i..next_byte]);
                if (c == '\'' || c == '"') && prev != '\\' {
                    quote = Some((c, next_byte));
                    content.clear();
                }
            }
            Some((q, content_start)) => {
                if c == q && prev != '\\' {
                    let closing_quote_byte = i;
                    let past_closing_quote = next_byte;
                    let content_was_non_empty = !content.is_empty();
                    if content_was_non_empty {
                        let orig_byte_len = closing_quote_byte - content_start;
                        out.push(LITERAL_MARK);
                        out.extend_from_slice(next_index.to_string().as_bytes());
                        out.push(LEN_SEP);
                        out.extend_from_slice(orig_byte_len.to_string().as_bytes());
                        dict.insert(next_index, std::mem::take(&mut content));
                        next_index += 1;
                    }
                    out.extend_from_slice(&input_bytes[i..next_byte]);
                    if content_was_non_empty {
                        out.push(JUMP_MARK);
                        out.extend_from_slice(past_closing_quote.to_string().as_bytes());
                        out.push(JUMP_MARK);
                    }
                    quote = None;
                } else {
                    content.push(c);
                }
            }
        }
        prev = c;
    }
    if let Some((_, content_start)) = quote {
        if !content.is_empty() {
            let orig_byte_len = input.len() - content_start;
            out.push(LITERAL_MARK);
            out.extend_from_slice(next_index.to_string().as_bytes());
            out.push(LEN_SEP);
            out.extend_from_slice(orig_byte_len.to_string().as_bytes());
            dict.insert(next_index, content);
        }
    }
    (out, dict)
}

#[cfg(test)]
mod tests {
    use super::{mask_quotes, JUMP_MARK, LEN_SEP, LITERAL_MARK};
    use std::collections::HashMap;

    fn lit(idx: usize, len: usize) -> Vec<u8> {
        let mut v = vec![LITERAL_MARK];
        v.extend_from_slice(idx.to_string().as_bytes());
        v.push(LEN_SEP);
        v.extend_from_slice(len.to_string().as_bytes());
        v
    }

    fn jmp(offset: usize) -> Vec<u8> {
        let mut v = vec![JUMP_MARK];
        v.extend_from_slice(offset.to_string().as_bytes());
        v.push(JUMP_MARK);
        v
    }

    fn cat(parts: &[&[u8]]) -> Vec<u8> {
        parts.iter().flat_map(|p| p.iter().copied()).collect()
    }

    fn d(pairs: &[(usize, &str)]) -> HashMap<usize, String> {
        pairs.iter().map(|(i, s)| (*i, s.to_string())).collect()
    }

    fn check(input: &str, expected_out: Vec<u8>, expected_dict: HashMap<usize, String>) {
        let (actual_out, actual_dict) = mask_quotes(input);
        assert_eq!(
            actual_out, expected_out,
            "output mismatch on {input:?}\n  got: {actual_out:?}\n want: {expected_out:?}"
        );
        assert_eq!(actual_dict, expected_dict, "dict mismatch on {input:?}");
    }

    #[test]
    fn plain_text_passes_through_verbatim() {
        check("lorem ipsum", b"lorem ipsum".to_vec(), d(&[]));
    }

    #[test]
    fn empty_literal_emits_no_marker() {
        // `''` → two verbatim quote bytes, no LITERAL placeholder, no JUMP.
        check("''", b"''".to_vec(), d(&[]));
        check("\"\"", b"\"\"".to_vec(), d(&[]));
    }

    #[test]
    fn single_quote_literal() {
        // bytes:        0 1 2 3 4 5 6        len=7, past-closing = 7
        // input:        ' h e l l o '
        check(
            "'hello'",
            cat(&[b"'", &lit(0, 5), b"'", &jmp(7)]),
            d(&[(0, "hello")]),
        );
    }

    #[test]
    fn double_quote_literal() {
        check(
            "\"hello\"",
            cat(&[b"\"", &lit(0, 5), b"\"", &jmp(7)]),
            d(&[(0, "hello")]),
        );
    }

    #[test]
    fn back_to_back_single_quote_literals() {
        // bytes:  0 1 2 3 4 5 6 7 8 9 10 11
        // input:  ' h e l l o ' ' b y e  '
        // first literal: content_start=1, close=6, len=5, past=7
        // second literal: content_start=8, close=11, len=3, past=12
        check(
            "'hello''bye'",
            cat(&[b"'", &lit(0, 5), b"'", &jmp(7), b"'", &lit(1, 3), b"'", &jmp(12)]),
            d(&[(0, "hello"), (1, "bye")]),
        );
    }

    #[test]
    fn literal_with_surrounding_text() {
        // input:   s  a  y  ' h  e  l  l  o  '  ...
        // bytes:   0  1  2  3 4  5  6  7  8  9  10 11 ...
        // content_start=5, close=10, len=5, past=11
        check(
            "say 'hello' to me",
            cat(&[b"say '", &lit(0, 5), b"'", &jmp(11), b" to me"]),
            d(&[(0, "hello")]),
        );
    }

    #[test]
    fn backslash_escaped_quote_inside_literal() {
        // raw input: `say 'hel\'lo' to me` — backslash is a literal byte.
        // index:    0 's', 1 'a', 2 'y', 3 ' ', 4 '\'', 5 'h', 6 'e', 7 'l',
        //           8 '\\', 9 '\'', 10 'l', 11 'o', 12 '\'', 13 ' ', ...
        // content_start = 5, close = 12, len = 7, past = 13.
        check(
            r"say 'hel\'lo' to me",
            cat(&[b"say '", &lit(0, 7), b"'", &jmp(13), b" to me"]),
            d(&[(0, r"hel\'lo")]),
        );
    }

    #[test]
    fn cross_quote_nesting_passes_through() {
        // raw input: say 'hey "hello" you' again
        // bytes:     s a y _ ' h e y _ " h e l l o " _ y o u '  _ a g a i n
        // index:     0 1 2 3 4 5 6 7 8 9 ...                 20 21 ...
        // Opening `'` at 4, content_start=5, closing `'` at 20. Content = `hey "hello" you`
        // (15 bytes), past-closing = 21.
        check(
            r#"say 'hey "hello" you' again"#,
            cat(&[b"say '", &lit(0, 15), b"'", &jmp(21), b" again"]),
            d(&[(0, r#"hey "hello" you"#)]),
        );
    }

    #[test]
    fn multi_byte_literal_length_counts_bytes_not_chars() {
        // `'héllo'` — é is 2 bytes (0xC3 0xA9) so the orig_byte_len reports 6 bytes
        // for the 5-codepoint content. Past-closing = 8 (1 + 6 + 1).
        check(
            "'héllo'",
            cat(&[b"'", &lit(0, 6), b"'", &jmp(8)]),
            d(&[(0, "héllo")]),
        );
    }

    #[test]
    fn emoji_literal_four_byte_codepoint() {
        // `'💡'` — 💡 is 4 bytes in UTF-8. content_start=1, close=5, len=4, past=6.
        check(
            "'💡'",
            cat(&[b"'", &lit(0, 4), b"'", &jmp(6)]),
            d(&[(0, "💡")]),
        );
    }

    #[test]
    fn unclosed_literal_emits_placeholder_without_jump() {
        // Trailing-content fallback emits the LITERAL with no JUMP; the downstream
        // parser detects the missing JUMP and reports "unclosed quote".
        let (out, dict) = mask_quotes("'hello");
        assert_eq!(out, cat(&[b"'", &lit(0, 5)]));
        assert_eq!(dict, d(&[(0, "hello")]));
    }
}
