//! Flat token stream over the source. Each non-blank line opens with a [`Token::Line`] carrying
//! its indentation; every other token records whether whitespace preceded it, which is all the
//! reader needs to compute [`Item::glued`](crate::Item::glued). Blank lines produce nothing.

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

pub(crate) fn lex(source: &str) -> Result<Vec<Token<'_>>, Error> {
    let bytes = source.as_bytes();
    let len = bytes.len();
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
            let line_end = source[pos..].find('\n').map_or(len, |n| pos + n);
            if source[pos..line_end].trim().is_empty() {
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

        let c = source[pos..]
            .chars()
            .next()
            .expect("pos < len and pos is a char boundary");
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
                    body: &source[start + 1..p],
                    span: span(start, p + 1),
                    spaced,
                });
                pos = p + 1;
                spaced = false;
            }
            _ => {
                let start = pos;
                while let Some(c) = source[pos..].chars().next()
                    && !is_break(c)
                {
                    pos += c.len_utf8();
                }
                out.push(Token::Atom {
                    text: &source[start..pos],
                    span: span(start, pos),
                    spaced,
                });
                spaced = false;
            }
        }
    }
    Ok(out)
}
