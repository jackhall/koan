//! The parser's error: a message, the span it was observed at, and the file the span indexes.
//!
//! The parser reaches nothing above [`source`], so its failure is its own leaf type rather than
//! the runtime's error. The runtime wraps one whole — `From<ParseError>` on its error type — and
//! renders it through this file's `Display`, so a parse error reads the same from the CLI, from
//! a test asserting on the message, and from the value the runtime hands a program that catches it.

use std::fmt;

use crate::source::{self, FileId, Span};

/// A parse failure. `file` is the source the span indexes; both are `None` when the failure has
/// no location to point at, and the rendering drops the `at path:line:col` clause with them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub span: Option<Span>,
    pub file: Option<FileId>,
}

impl ParseError {
    /// Resolves `file` from the thread-local current file so call sites only thread the observed
    /// `Span`.
    pub fn new(message: impl Into<String>, span: Option<Span>) -> Self {
        Self {
            message: message.into(),
            span,
            file: source::current(),
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let loc = match (self.span, self.file) {
            (Some(sp), Some(fid)) => source::with(fid, |sf| {
                let (line, col_utf16) = sf.resolve(sp.start);
                Some((sf.path.clone(), line, col_utf16))
            }),
            _ => None,
        };
        match loc {
            Some((path, line, col)) => {
                write!(f, "parse error at {path}:{line}:{col}: {}", self.message)
            }
            None => write!(f, "parse error: {}", self.message),
        }
    }
}

impl std::error::Error for ParseError {}
