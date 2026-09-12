//! Whitespace-sensitive s-expressions with glue: the layout half of a parser, with no vocabulary.
//!
//! [`read`] turns source text into a tree of [`Item`]s and knows exactly five things about it:
//!
//! 1. **Whitespace separates.** A token is a maximal run of anything that is not whitespace, a
//!    bracket, a quote or a comma. Nothing else splits a token, so `:Number`, `a.b`, `->` and `#`
//!    are all single atoms and the language on top decides what they mean.
//! 2. **Three bracket families group.** `(...)`, `[...]` and `{...}` become
//!    [`Kind::Paren`] / [`Kind::Bracket`] / [`Kind::Brace`] groups. A comma is its own
//!    [`Node::Comma`] item.
//! 3. **Quotes delimit strings.** `'...'` and `"..."` become [`Node::Str`] with the body kept
//!    verbatim; a backslash escapes the following character. A string may span lines.
//! 4. **Adjacency is recorded, not interpreted.** Every item carries [`Item::glued`]: whether the
//!    next sibling followed it with no whitespace between. A sigil is simply an atom glued to the
//!    group after it, so this crate never needs to know which atoms are sigils.
//! 5. **Indentation groups lines.** Each non-blank line is a [`Kind::Layout`] group; a deeper line
//!    nests inside the line above it and a dedent closes. Indentation is spaces only, in even
//!    widths. Three regimes suspend the line-becomes-group rule:
//!    - inside an open `[` or `{`, lines join flat and indentation is ignored entirely;
//!    - a line ending in `,` joins the next non-blank line flat, whatever its indentation;
//!    - inside an open `(`, a deeper line nests as its own layout group (one per line), a
//!      same-or-shallower line that is not the closer is an error, and the closing `)` may sit on
//!      its own line at any indentation not less than the layout line that opened it, or at the
//!      end of the last body line. Where the closer sits is layout, not structure. Text after the
//!      closer continues the layout line the paren was opened on. "Deeper" and "shallower" are
//!      measured against that layout line: a `(` on a comma-joined continuation anchors to the
//!      line the continuation joined, not to its own indentation.
//!
//! Everything else — which atoms are keywords, which glued prefixes are sigils, what a comma means
//! inside a brace — belongs to the layer above. The tree borrows the source it was read from, and
//! every span is a byte range into that source.
//!
//! The design — why the crate refuses a token class, a sigil table and an expression shape, how
//! the lexer and the reader split the work, and what each error's span points at — is
//! [README.md](../README.md).

#![forbid(unsafe_code)]

mod lex;
mod reader;
mod render;

#[cfg(test)]
mod tests;

use std::fmt;

pub use render::render;

/// Byte-offset half-open range into the source text a tree was read from.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    /// The source text this span covers.
    pub fn slice(self, source: &str) -> &str {
        &source[self.start as usize..self.end as usize]
    }
}

/// The four group shapes. `Layout` groups are synthesized from line structure; the other three
/// come from bracket pairs in the text.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Layout,
    Paren,
    Bracket,
    Brace,
}

impl Kind {
    /// The opening and closing delimiter of a bracketed kind; `None` for `Layout`.
    pub fn delimiters(self) -> Option<(char, char)> {
        match self {
            Kind::Layout => None,
            Kind::Paren => Some(('(', ')')),
            Kind::Bracket => Some(('[', ']')),
            Kind::Brace => Some(('{', '}')),
        }
    }

    fn opener(self) -> char {
        self.delimiters().map_or(' ', |(open, _)| open)
    }

    fn closer(self) -> char {
        self.delimiters().map_or(' ', |(_, close)| close)
    }
}

/// One node of the tree with its source span and its adjacency to the next sibling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item<'s> {
    pub node: Node<'s>,
    /// Bracketed groups span opener through closer; a layout group spans its first item's start
    /// through its last item's end; a string spans both quotes.
    pub span: Span,
    /// No whitespace separated this item from the sibling after it. Always `false` on the last
    /// item of a group, on a layout group, and on the item before a layout group (a line break
    /// intervenes).
    pub glued: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node<'s> {
    Atom(&'s str),
    /// A quoted string. `body` is the text between the quotes, escapes untouched.
    Str {
        quote: char,
        body: &'s str,
    },
    Comma,
    Group {
        kind: Kind,
        items: Vec<Item<'s>>,
    },
}

/// A structural error, located by the span that best explains it (see each [`ErrorKind`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// Span: the first tab in the line's indentation.
    TabIndent,
    /// Span: the line's indentation.
    OddIndent { width: usize },
    /// Span: the opening quote through end of input.
    UnclosedString { quote: char },
    /// Span: the opener.
    UnclosedGroup { kind: Kind },
    /// A closer with no open group to close. Span: the closer.
    UnexpectedCloser { kind: Kind },
    /// A closer of the wrong family for the innermost open group. Span: the closer.
    MismatchedCloser {
        opened: Kind,
        opener: Span,
        found: Kind,
    },
    /// A line at or above the indentation of the layout line that opened a `(` arrived before
    /// the `)`. Span: the opener.
    DanglingParen,
    /// A `)` on its own line, less indented than the layout line that opened it. Span: the
    /// closer.
    CloserDedented { opener: Span },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ErrorKind::TabIndent => write!(f, "tab indentation is not allowed"),
            ErrorKind::OddIndent { width } => {
                write!(f, "odd-numbered space indentation ({width} spaces)")
            }
            ErrorKind::UnclosedString { quote } => {
                write!(f, "unclosed string literal: missing closing {quote}")
            }
            ErrorKind::UnclosedGroup { kind } => write!(
                f,
                "unclosed '{}': this group was never closed with a matching '{}'",
                kind.opener(),
                kind.closer(),
            ),
            ErrorKind::UnexpectedCloser { kind } => write!(
                f,
                "closing '{}' without a matching '{}'",
                kind.closer(),
                kind.opener(),
            ),
            ErrorKind::MismatchedCloser { opened, found, .. } => write!(
                f,
                "closing '{}' does not match the open '{}'",
                found.closer(),
                opened.opener(),
            ),
            ErrorKind::DanglingParen => write!(
                f,
                "unmatched '(': an open paren must close before an expression break (a line at \
                 the same or lesser indentation). Indent the continuation deeper, or close the \
                 paren before breaking the line."
            ),
            ErrorKind::CloserDedented { .. } => write!(
                f,
                "closing ')' is less indented than the '(' it closes; a paren must close at the \
                 same or greater indentation as its opener."
            ),
        }
    }
}

impl std::error::Error for Error {}

/// Read `source` into its top-level layout groups, one per top-level line.
pub fn read(source: &str) -> Result<Vec<Item<'_>>, Error> {
    reader::read(source)
}
