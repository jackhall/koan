//! Recursive descent over the token stream, building the item tree. Three readers cover the
//! three regimes the crate doc describes: [`Reader::layout_line`] for an indentation-governed
//! line and its children, the paren arm of [`Reader::group`] for an indentation-sensitive `(`,
//! and the flat arm for `[` / `{` (and any `(` nested inside them), which ignores line breaks.
//! [`Reader::inline_items`] reads one line's run of items and performs the trailing-comma join.
//!
//! A closer is owned by the innermost bracketed group; a layout group never owns one, so a `)`
//! arriving inside a layout line ends that line (and every layout group above it up to the paren)
//! and is left for the paren to consume.

use crate::lex::{Token, lex};
use crate::{Error, ErrorKind, Item, Kind, Node, Span};

pub(crate) fn read(source: &str) -> Result<Vec<Item<'_>>, Error> {
    let mut reader = Reader {
        tokens: lex(source)?,
        pos: 0,
        layout_indent: 0,
        flat: 0,
    };
    let mut top = Vec::new();
    while let Some(token) = reader.peek() {
        match token {
            Token::Line { .. } => {
                if let Some((_, kind, span)) = reader.closer_first_line() {
                    return Err(Error {
                        kind: ErrorKind::UnexpectedCloser { kind },
                        span,
                    });
                }
                top.push(reader.layout_line()?);
            }
            Token::Close { kind, span, .. } => {
                return Err(Error {
                    kind: ErrorKind::UnexpectedCloser { kind },
                    span,
                });
            }
            _ => unreachable!("a layout line consumes its whole inline run"),
        }
    }
    Ok(top)
}

struct Reader<'s> {
    tokens: Vec<Token<'s>>,
    pos: usize,
    /// Indentation of the layout line whose items are being read; a `(` anchors to it. A joined
    /// line or a closer line is not a layout line and leaves it untouched.
    layout_indent: usize,
    /// Depth of enclosing `[` / `{` groups. Above zero, line breaks are ignored.
    flat: usize,
}

impl<'s> Reader<'s> {
    fn peek(&self) -> Option<Token<'s>> {
        self.tokens.get(self.pos).copied()
    }

    fn peek2(&self) -> Option<Token<'s>> {
        self.tokens.get(self.pos + 1).copied()
    }

    fn bump(&mut self) -> Token<'s> {
        let token = self.tokens[self.pos];
        self.pos += 1;
        token
    }

    /// Consume a `Line` in indentation-governed position: tabs and odd widths are errors.
    fn take_layout_line(&mut self) -> Result<usize, Error> {
        let Token::Line { indent, tab, span } = self.bump() else {
            unreachable!("caller peeked a Line token")
        };
        if let Some(tab) = tab {
            return Err(Error {
                kind: ErrorKind::TabIndent,
                span: tab,
            });
        }
        if indent % 2 != 0 {
            return Err(Error {
                kind: ErrorKind::OddIndent { width: indent },
                span,
            });
        }
        Ok(indent)
    }

    /// Consume a `Line` that joins the line before it; its indentation is free-form.
    fn take_joined_line(&mut self) {
        let Token::Line { .. } = self.bump() else {
            unreachable!("caller peeked a Line token")
        };
    }

    /// The next line's indentation when that line can open a layout group — it exists and does
    /// not begin with a closer.
    fn peek_block_line(&self) -> Option<usize> {
        match (self.peek(), self.peek2()) {
            (Some(Token::Line { .. }), Some(Token::Close { .. })) => None,
            (Some(Token::Line { indent, .. }), _) => Some(indent),
            _ => None,
        }
    }

    /// The next line when it begins with a closer: its indentation and the closer's kind and span.
    fn closer_first_line(&self) -> Option<(usize, Kind, Span)> {
        match (self.peek(), self.peek2()) {
            (Some(Token::Line { indent, .. }), Some(Token::Close { kind, span, .. })) => {
                Some((indent, kind, span))
            }
            _ => None,
        }
    }

    /// Whether the item just finished is glued to the sibling after it.
    fn glued_next(&self) -> bool {
        matches!(
            self.peek(),
            Some(
                Token::Atom { spaced: false, .. }
                    | Token::Str { spaced: false, .. }
                    | Token::Comma { spaced: false, .. }
                    | Token::Open { spaced: false, .. }
            )
        )
    }

    fn leaf(&mut self) -> Item<'s> {
        let (node, span) = match self.bump() {
            Token::Atom { text, span, .. } => (Node::Atom(text), span),
            Token::Str {
                quote, body, span, ..
            } => (Node::Str { quote, body }, span),
            Token::Comma { span, .. } => (Node::Comma, span),
            _ => unreachable!("caller peeked a leaf token"),
        };
        Item {
            node,
            span,
            glued: self.glued_next(),
        }
    }

    /// One line's run of items, stopping before a line break, a closer, or end of input. A run
    /// ending in a comma continues onto the next line, whatever its indentation.
    fn inline_items(&mut self) -> Result<Vec<Item<'s>>, Error> {
        let mut items = Vec::new();
        loop {
            match self.peek() {
                Some(Token::Atom { .. } | Token::Str { .. } | Token::Comma { .. }) => {
                    items.push(self.leaf());
                }
                Some(Token::Open { .. }) => items.push(self.group()?),
                Some(Token::Line { .. })
                    if matches!(
                        items.last(),
                        Some(Item {
                            node: Node::Comma,
                            ..
                        })
                    ) =>
                {
                    self.take_joined_line();
                }
                _ => return Ok(items),
            }
        }
    }

    /// An indentation-governed line: its inline run, then every deeper line as a child.
    fn layout_line(&mut self) -> Result<Item<'s>, Error> {
        let indent = self.take_layout_line()?;
        self.layout_indent = indent;
        let mut items = self.inline_items()?;
        while self.peek_block_line().is_some_and(|child| child > indent) {
            items.push(self.layout_line()?);
        }
        let first = items.first().expect("a layout line has at least one token");
        let last = items.last().expect("a layout line has at least one token");
        let span = Span {
            start: first.span.start,
            end: last.span.end,
        };
        Ok(Item {
            node: Node::Group {
                kind: Kind::Layout,
                items,
            },
            span,
            glued: false,
        })
    }

    fn group(&mut self) -> Result<Item<'s>, Error> {
        let Token::Open {
            kind, span: open, ..
        } = self.bump()
        else {
            unreachable!("caller peeked an Open token")
        };
        let opener_indent = self.layout_indent;
        let mut items = Vec::new();

        let close = if kind == Kind::Paren && self.flat == 0 {
            loop {
                items.append(&mut self.inline_items()?);
                match self.peek() {
                    Some(Token::Close {
                        kind: found, span, ..
                    }) => {
                        check_closer(kind, open, found, span)?;
                        self.bump();
                        break span;
                    }
                    Some(Token::Line { indent, .. }) => {
                        if let Some((_, _, closer)) = self.closer_first_line() {
                            if indent < opener_indent {
                                return Err(Error {
                                    kind: ErrorKind::CloserDedented { opener: open },
                                    span: closer,
                                });
                            }
                            self.take_layout_line()?;
                        } else if indent <= opener_indent {
                            return Err(Error {
                                kind: ErrorKind::DanglingParen,
                                span: open,
                            });
                        } else {
                            while self
                                .peek_block_line()
                                .is_some_and(|child| child > opener_indent)
                            {
                                items.push(self.layout_line()?);
                            }
                            // Whatever follows the closer continues the opening layout line.
                            self.layout_indent = opener_indent;
                        }
                    }
                    None => {
                        return Err(Error {
                            kind: ErrorKind::UnclosedGroup { kind },
                            span: open,
                        });
                    }
                    Some(_) => unreachable!("inline_items consumes every leaf and opener"),
                }
            }
        } else {
            self.flat += 1;
            let close = loop {
                match self.peek() {
                    Some(Token::Line { .. }) => self.take_joined_line(),
                    Some(Token::Atom { .. } | Token::Str { .. } | Token::Comma { .. }) => {
                        items.push(self.leaf());
                    }
                    Some(Token::Open { .. }) => items.push(self.group()?),
                    Some(Token::Close {
                        kind: found, span, ..
                    }) => {
                        check_closer(kind, open, found, span)?;
                        self.bump();
                        break span;
                    }
                    None => {
                        return Err(Error {
                            kind: ErrorKind::UnclosedGroup { kind },
                            span: open,
                        });
                    }
                }
            };
            self.flat -= 1;
            close
        };

        Ok(Item {
            node: Node::Group { kind, items },
            span: Span {
                start: open.start,
                end: close.end,
            },
            glued: self.glued_next(),
        })
    }
}

fn check_closer(opened: Kind, opener: Span, found: Kind, closer: Span) -> Result<(), Error> {
    if opened == found {
        Ok(())
    } else {
        Err(Error {
            kind: ErrorKind::MismatchedCloser {
                opened,
                opener,
                found,
            },
            span: closer,
        })
    }
}
