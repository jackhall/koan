//! `ParseStack` holds the parser's nesting state. The root expression lives
//! directly on the struct so `push_part` never needs to unwrap an empty
//! stack. Variant-aware pops and the shape-shared `open_collection` /
//! `close_collection` helpers live here since they bind `ParseStack` and the
//! token-buffer flush.

use crate::source::Spanned;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::KError;
use crate::parse::tokens::classify_token;

use super::dict_literal::DictFrame;
use super::frame::Frame;

pub(super) struct ParseStack<'a> {
    root: KExpression<'a>,
    rest: Vec<Frame<'a>>,
}

impl<'a> ParseStack<'a> {
    pub(super) fn new() -> Self {
        Self {
            root: KExpression::new(Vec::new()),
            rest: Vec::new(),
        }
    }

    pub(super) fn push_frame(&mut self, f: Frame<'a>) {
        self.rest.push(f);
    }

    /// Push a span-carrying part into the current top frame (root if none
    /// open). The span is preserved when the destination's storage is
    /// `Vec<Spanned<…>>`; List/Dict/TypeExpr frames discard it.
    pub(super) fn push_part(&mut self, part: Spanned<ExpressionPart<'a>>) {
        match self.rest.last_mut() {
            Some(f) => f.push(part),
            None => self.root.parts.push(part),
        }
    }

    pub(super) fn peek_top(&self) -> Option<&Frame<'a>> {
        self.rest.last()
    }

    /// Top-of-stack frame as a `Dict` for in-place state-machine ops. `None`
    /// when the top is any other variant or no frame is nested.
    pub(super) fn top_dict_mut(&mut self) -> Option<&mut DictFrame<'a>> {
        match self.rest.last_mut()? {
            Frame::Dict { dict, .. } => Some(dict),
            _ => None,
        }
    }

    /// Unconditional pop of the topmost nested frame. Used by `)` which
    /// destructures all variants for distinct diagnostics.
    pub(super) fn pop_top(&mut self) -> Option<Frame<'a>> {
        self.rest.pop()
    }

    pub(super) fn finish(self) -> Result<KExpression<'a>, KError> {
        if !self.rest.is_empty() {
            return Err(KError::parse(
                "open paren, bracket, or brace without matching close",
                None,
            ));
        }
        Ok(self.root)
    }
}

pub(super) fn flush_token<'a>(
    stack: &mut ParseStack<'a>,
    buf: &mut String,
    token_start: &mut Option<u32>,
) -> Result<(), KError> {
    if !buf.is_empty() {
        let tok = std::mem::take(buf);
        let start = token_start
            .take()
            .expect("token_start must be set whenever buf is non-empty");
        let part = classify_token(&tok, start)?;
        stack.push_part(part);
    } else {
        *token_start = None;
    }
    Ok(())
}

/// Open shape shared by `[` and `{`: reject a glued opener, flush any
/// pending token into the parent, then push the new frame.
pub(super) fn open_collection<'a>(
    stack: &mut ParseStack<'a>,
    buf: &mut String,
    opener: char,
    prev: Option<char>,
    frame: Frame<'a>,
    token_start: &mut Option<u32>,
) -> Result<(), KError> {
    check_open_adjacency(opener, prev)?;
    flush_token(stack, buf, token_start)?;
    stack.push_frame(frame);
    Ok(())
}

/// Close shape shared by `]` and `}`: verify the topmost frame matches
/// `closer`, run adjacency, flush any pending token, then pop and fold the
/// frame into the part it produces.
pub(super) fn close_collection<'a>(
    stack: &mut ParseStack<'a>,
    buf: &mut String,
    closer: char,
    next: Option<char>,
    mismatch_msg: &str,
    token_start: &mut Option<u32>,
    end: u32,
) -> Result<(), KError> {
    let top_matches = stack.peek_top().is_some_and(|f| f.matches_closer(closer));
    if !top_matches {
        return Err(KError::parse(mismatch_msg, None));
    }
    check_close_adjacency(closer, next)?;
    flush_token(stack, buf, token_start)?;
    let frame = stack
        .pop_top()
        .expect("peek_top.matches_closer checked above; flush_token preserves variant");
    stack.push_part(frame.into_part(end)?);
    Ok(())
}

fn check_open_adjacency(opener: char, prev: Option<char>) -> Result<(), KError> {
    if matches!(prev, None | Some('(' | '[' | '{')) || matches!(prev, Some(c) if c.is_whitespace())
    {
        return Ok(());
    }
    Err(KError::parse(
        format!(
            "'{opener}' must be preceded by whitespace, '(', '[', or '{{' \
             (got {prev:?}); collection literals can't be glued to a token",
        ),
        None,
    ))
}

fn check_close_adjacency(closer: char, next: Option<char>) -> Result<(), KError> {
    if matches!(next, None | Some(')' | ']' | '}')) || matches!(next, Some(c) if c.is_whitespace())
    {
        return Ok(());
    }
    Err(KError::parse(
        format!(
            "'{closer}' must be followed by whitespace, ')', ']', or '}}' \
             (got {next:?}); collection literals can't be glued to a token",
        ),
        None,
    ))
}
