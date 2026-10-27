//! Flat token stream over the source. Each non-blank line opens with a [`Token::Line`] carrying
//! its indentation; every other token records whether whitespace preceded it, which is all the
//! reader needs to compute [`Item::glued`](crate::Item::glued). Blank lines produce nothing.
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

/// The characters that end an atom.
fn is_break(c: char) -> bool {
    c.is_whitespace() || matches!(c, '(' | ')' | '[' | ']' | '{' | '}' | ',' | '\'' | '"')
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

    /// The source from `start` to `end`, left in place. `start` is at or past the last carve's end.
    fn peek(&self, start: usize, end: usize) -> &'s str {
        &self.text[start - self.at..end - self.at]
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

pub(crate) fn lex(source: &str) -> Result<Vec<Token<'_>>, Error> {
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
            let mut line_end = pos;
            while line_end < len && bytes[line_end] != b'\n' {
                line_end += 1;
            }
            if rest.peek(pos, line_end).trim().is_empty() {
                pos = (line_end + 1).min(len);
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
