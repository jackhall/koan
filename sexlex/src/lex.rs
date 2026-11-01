//! Flat token stream over the source. Each non-blank line opens with a [`Token::Line`] carrying
//! its indentation; every other token records whether whitespace preceded it, which is all the
//! reader needs to compute [`Item::glued`](crate::Item::glued). Blank lines produce nothing. A `;`
//! outside a string starts a comment, skipped to the end of its line, and a line holding only
//! whitespace and a comment is blank. Lexing one group ([`Extent::Group`]) stops after the closer
//! that balances the first opener, or at a closer that matches no open group, so text past the
//! group, which may not be code, is never lexed.
//!
//! The scan reads the source a byte at a time and carves each token's text off the front of what is
//! left ([`Rest`]), never out of the whole source. Under Miri's tree borrows, a slice taken from the
//! whole source is checked against every token slice still alive, which makes lexing cubic in the
//! source's length there; one carved from the rest overlaps none of them.

use crate::{Error, ErrorKind, Kind, Span};

#[derive(Copy, Clone, Debug)]
pub(crate) enum Token<'s> {
    /// Start of a non-blank line. `span` covers the indentation bytes; `tab` is the first tab in
    /// them, if any — the reader decides whether that matters for this line.
    Line {
        indent: usize,
        tab: Option<Span>,
        span: Span,
    },
    Atom {
        text: &'s str,
        span: Span,
        spaced: bool,
    },
    Str {
        quote: char,
        body: &'s str,
        span: Span,
        spaced: bool,
    },
    Comma {
        span: Span,
        spaced: bool,
    },
    Open {
        kind: Kind,
        span: Span,
        spaced: bool,
    },
    Close {
        kind: Kind,
        span: Span,
    },
}

fn span(start: usize, end: usize) -> Span {
    Span {
        start: start as u32,
        end: end as u32,
    }
}

/// The characters that end an atom: whitespace, punctuation, and the `;` that starts a comment.
fn is_break(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '(' | ')' | '[' | ']' | '{' | '}' | ',' | '\'' | '"' | ';'
        )
}

/// What is left of the source past the last token text carved from it.
struct Rest<'s> {
    text: &'s str,
    /// Where `text` starts in the source.
    at: usize,
}

impl<'s> Rest<'s> {
    /// The source from `start` to `end`, split off the front of what is left. `start` is at or past
    /// the last carve's end.
    fn carve(&mut self, start: usize, end: usize) -> &'s str {
        let (_, from) = self.text.split_at(start - self.at);
        let (carved, tail) = from.split_at(end - start);
        (self.text, self.at) = (tail, end);
        carved
    }

    /// The char at `pos`, a char boundary short of the source's end and at or past the last carve's
    /// end: the byte there when it is ASCII.
    fn char_at(&self, bytes: &[u8], pos: usize) -> char {
        match bytes[pos] {
            byte if byte.is_ascii() => byte as char,
            _ => (self.text[pos - self.at..].chars().next())
                .expect("pos < len and pos is a char boundary"),
        }
    }
}

/// How much of a source [`lex`] reads.
#[derive(Copy, Clone, PartialEq, Eq)]
pub(crate) enum Extent {
    /// All of it.
    Whole,
    /// Up to and including the closer that balances the source's first opener, or the first closer
    /// that does not match the innermost open group.
    Group,
}

pub(crate) fn lex(source: &str, extent: Extent) -> Result<Vec<Token<'_>>, Error> {
    let bytes = source.as_bytes();
    let len = bytes.len();
    let mut rest = Rest {
        text: source,
        at: 0,
    };
    let mut out = Vec::new();
    let mut pos = 0;
    let mut spaced = true;
    let mut at_line_start = true;
    // The open groups' kinds, kept only when lexing one group.
    let mut open = Vec::new();

    while pos < len {
        if at_line_start {
            let indent_start = pos;
            let mut tab = None;
            while pos < len && (bytes[pos] == b' ' || bytes[pos] == b'\t') {
                if bytes[pos] == b'\t' && tab.is_none() {
                    tab = Some(span(pos, pos + 1));
                }
                pos += 1;
            }
            // A line of nothing but whitespace and a comment is blank. Scanning to the line's end
            // only for a blank line keeps a group read from the middle of a long line linear.
            let mut content = pos;
            while content < len && bytes[content] != b'\n' {
                let c = rest.char_at(bytes, content);
                if !c.is_whitespace() {
                    break;
                }
                content += c.len_utf8();
            }
            if content == len || matches!(bytes[content], b'\n' | b';') {
                while content < len && bytes[content] != b'\n' {
                    content += 1;
                }
                pos = (content + 1).min(len);
                continue;
            }
            out.push(Token::Line {
                indent: pos - indent_start,
                tab,
                span: span(indent_start, pos),
            });
            at_line_start = false;
            spaced = true;
        }

        let c = rest.char_at(bytes, pos);
        match c {
            '\n' => {
                at_line_start = true;
                pos += 1;
            }
            c if c.is_whitespace() => {
                spaced = true;
                pos += c.len_utf8();
            }
            ';' => {
                // A comment runs to the end of its line; the `'\n'` arm ends the line.
                while pos < len && bytes[pos] != b'\n' {
                    pos += 1;
                }
                spaced = true;
            }
            '(' | '[' | '{' => {
                let kind = match c {
                    '(' => Kind::Paren,
                    '[' => Kind::Bracket,
                    _ => Kind::Brace,
                };
                out.push(Token::Open {
                    kind,
                    span: span(pos, pos + 1),
                    spaced,
                });
                if extent == Extent::Group {
                    open.push(kind);
                }
                pos += 1;
                spaced = false;
            }
            ')' | ']' | '}' => {
                let kind = match c {
                    ')' => Kind::Paren,
                    ']' => Kind::Bracket,
                    _ => Kind::Brace,
                };
                out.push(Token::Close {
                    kind,
                    span: span(pos, pos + 1),
                });
                pos += 1;
                spaced = false;
                if extent == Extent::Group && (open.pop() != Some(kind) || open.is_empty()) {
                    // The group's closer, or a mismatch the reader reports: nothing past it is
                    // lexed.
                    return Ok(out);
                }
            }
            ',' => {
                out.push(Token::Comma {
                    span: span(pos, pos + 1),
                    spaced,
                });
                pos += 1;
                spaced = false;
            }
            '\'' | '"' => {
                let start = pos;
                let quote = c as u8;
                let mut p = pos + 1;
                loop {
                    if p >= len {
                        return Err(Error {
                            kind: ErrorKind::UnclosedString { quote: c },
                            span: span(start, len),
                        });
                    }
                    match bytes[p] {
                        b'\\' => p += 2,
                        b if b == quote => break,
                        _ => p += 1,
                    }
                }
                out.push(Token::Str {
                    quote: c,
                    body: rest.carve(start + 1, p),
                    span: span(start, p + 1),
                    spaced,
                });
                pos = p + 1;
                spaced = false;
            }
            _ => {
                let start = pos;
                while pos < len {
                    let c = rest.char_at(bytes, pos);
                    if is_break(c) {
                        break;
                    }
                    pos += c.len_utf8();
                }
                out.push(Token::Atom {
                    text: rest.carve(start, pos),
                    span: span(start, pos),
                    spaced,
                });
                spaced = false;
            }
        }
    }
    Ok(out)
}
