//! `ParseStack` holds the parser's nesting state. The root expression lives directly on
//! the struct (rather than as the bottom of a `Vec<Frame>`), so `push_part` /
//! `top_last_part` never need to unwrap an empty stack. Variant-aware pop helpers and the
//! `open_collection` / `close_collection` shape-shared close-bracket helpers live here
//! since they bind `ParseStack` and the token-buffer flush.

use crate::parse::tokens::classify_token;
use crate::runtime::machine::model::ast::{ExpressionPart, KExpression};

use super::dict_literal::DictFrame;
use super::frame::Frame;

pub(super) struct ParseStack<'a> {
    root: KExpression<'a>,
    rest: Vec<Frame<'a>>,
}

impl<'a> ParseStack<'a> {
    pub(super) fn new() -> Self {
        Self {
            root: KExpression { parts: Vec::new() },
            rest: Vec::new(),
        }
    }

    pub(super) fn push_frame(&mut self, f: Frame<'a>) {
        self.rest.push(f);
    }

    /// Push a part into the current top frame (root if no nested frame is open).
    pub(super) fn push_part(&mut self, part: ExpressionPart<'a>) {
        match self.rest.last_mut() {
            Some(f) => f.push(part),
            None => self.root.parts.push(part),
        }
    }

    pub(super) fn peek_top(&self) -> Option<&Frame<'a>> {
        self.rest.last()
    }

    /// Top-of-stack frame as a `Dict` for in-place state-machine ops. Returns `None`
    /// when the top is any other variant (or no frame is nested).
    pub(super) fn top_dict_mut(&mut self) -> Option<&mut DictFrame<'a>> {
        match self.rest.last_mut()? {
            Frame::Dict(d) => Some(d),
            _ => None,
        }
    }

    /// Unconditional pop of the topmost nested frame. Used by `)` which needs to
    /// destructure all four variants for distinct diagnostics.
    pub(super) fn pop_top(&mut self) -> Option<Frame<'a>> {
        self.rest.pop()
    }

    pub(super) fn finish(self) -> Result<KExpression<'a>, String> {
        if !self.rest.is_empty() {
            return Err(
                "open paren, bracket, or brace without matching close".to_string(),
            );
        }
        Ok(self.root)
    }
}

pub(super) fn flush_token<'a>(stack: &mut ParseStack<'a>, buf: &mut String) -> Result<(), String> {
    if !buf.is_empty() {
        let tok = std::mem::take(buf);
        let part = classify_token(tok)?;
        stack.push_part(part);
    }
    Ok(())
}

/// Standard open-collection shape shared by `[` and `{`: reject a glued opener, flush any
/// pending token into the parent frame, then push the new (empty) frame onto the stack.
pub(super) fn open_collection<'a>(
    stack: &mut ParseStack<'a>,
    buf: &mut String,
    opener: char,
    prev: Option<char>,
    frame: Frame<'a>,
) -> Result<(), String> {
    check_open_adjacency(opener, prev)?;
    flush_token(stack, buf)?;
    stack.push_frame(frame);
    Ok(())
}

/// Standard close-collection shape shared by `]` and `}`: verify the topmost frame
/// matches `closer`, run close-side adjacency, flush any pending token into the closing
/// frame, then pop and fold the frame into the part it produces.
pub(super) fn close_collection<'a>(
    stack: &mut ParseStack<'a>,
    buf: &mut String,
    closer: char,
    next: Option<char>,
    mismatch_msg: &str,
) -> Result<(), String> {
    let top_matches = stack
        .peek_top()
        .is_some_and(|f| f.matches_closer(closer));
    if !top_matches {
        return Err(mismatch_msg.to_string());
    }
    check_close_adjacency(closer, next)?;
    flush_token(stack, buf)?;
    let frame = stack
        .pop_top()
        .expect("peek_top.matches_closer checked above; flush_token preserves variant");
    stack.push_part(frame.into_part()?);
    Ok(())
}

fn check_open_adjacency(opener: char, prev: Option<char>) -> Result<(), String> {
    if matches!(prev, None | Some('(' | '[' | '{')) || matches!(prev, Some(c) if c.is_whitespace()) {
        return Ok(());
    }
    Err(format!(
        "'{opener}' must be preceded by whitespace, '(', '[', or '{{' \
         (got {prev:?}); collection literals can't be glued to a token",
    ))
}

/// Symmetric to `check_open_adjacency` for closing brackets.
fn check_close_adjacency(closer: char, next: Option<char>) -> Result<(), String> {
    if matches!(next, None | Some(')' | ']' | '}')) || matches!(next, Some(c) if c.is_whitespace()) {
        return Ok(());
    }
    Err(format!(
        "'{closer}' must be followed by whitespace, ')', ']', or '}}' \
         (got {next:?}); collection literals can't be glued to a token",
    ))
}
