//! `ParseStack` holds the parser's nesting state. The root expression lives
//! directly on the struct so `push_part` never needs to unwrap an empty
//! stack. Variant-aware pops and the shape-shared `open_collection` /
//! `close_collection` helpers live here since they bind `ParseStack` and the
//! pending-token flush.

use crate::machine::KError;
use crate::machine::core::ProgramBrand;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::labels::LabelInterner;
use crate::parse::tokens::classify_token;
use crate::source::Spanned;

use super::dict_literal::DictFrame;
use super::frame::BracketFrame;

/// The stack carries the program storage every token string and every parts run is bumped into, so
/// no parse site reaches for storage of its own. It is a [`ProgramBrand`] rather than a plain
/// region brand because the nested-node arms the frames fold into are value-channel conduits. The root run stays a plain `Vec` until
/// [`ParseStack::finish`] freezes it — a node is built only once its parts are final.
pub(super) struct ParseStack<'a, 'l> {
    brand: ProgramBrand<'a>,
    labels: &'l LabelInterner,
    root: Vec<Spanned<ExpressionPart<'a>>>,
    rest: Vec<BracketFrame<'a>>,
}

impl<'a, 'l> ParseStack<'a, 'l> {
    pub(super) fn new(brand: ProgramBrand<'a>, labels: &'l LabelInterner) -> Self {
        Self {
            brand,
            labels,
            root: Vec::new(),
            rest: Vec::new(),
        }
    }

    /// The program storage every part this stack collects is bumped into.
    pub(super) fn brand(&self) -> ProgramBrand<'a> {
        self.brand
    }

    /// The run's label table, which every keyword this stack classifies is recorded in.
    pub(super) fn labels(&self) -> &LabelInterner {
        self.labels
    }

    pub(super) fn push_frame(&mut self, f: BracketFrame<'a>) {
        self.rest.push(f);
    }

    /// Push a span-carrying part into the current top frame (root if none
    /// open). The span is preserved when the destination's storage is
    /// `Vec<Spanned<…>>`; List/Dict/SigiledTypeExpr frames discard it.
    ///
    /// The one funnel every part passes through, so it is where a **keyword in a literal** is
    /// refused: a list, dict or record literal holds values, and the only keyword-shaped things its
    /// syntax has are the `,` / `:` / `=` delimiters the frame consumes itself. Every other frame —
    /// root, `Expression`, `Quote`, `SigiledTypeExpr`, `RecordTypeExpr` — still admits keywords,
    /// which is what `#(+)` bodies and `OP` declarations rest on. A `a.b` or `x?` inside a literal
    /// is unaffected: the suffix builders wrap their keyword inside a nested part before it reaches
    /// here.
    pub(super) fn push_part(&mut self, part: Spanned<ExpressionPart<'a>>) -> Result<(), KError> {
        match self.rest.last_mut() {
            Some(f) => {
                if let (
                    BracketFrame::List { .. } | BracketFrame::Dict { .. },
                    ExpressionPart::Keyword(symbol),
                ) = (&*f, part.value)
                {
                    return Err(KError::parse(
                        format!(
                            "`{}` is a keyword, so it cannot be an element of a list, dict, or \
                             record literal",
                            self.labels.display(symbol.symbol()),
                        ),
                        part.span,
                    ));
                }
                f.push(part);
            }
            None => self.root.push(part),
        }
        Ok(())
    }

    pub(super) fn peek_top(&self) -> Option<&BracketFrame<'a>> {
        self.rest.last()
    }

    /// Top-of-stack frame as a `Dict` for in-place state-machine ops. `None`
    /// when the top is any other variant or no frame is nested.
    pub(super) fn top_dict_mut(&mut self) -> Option<&mut DictFrame<'a>> {
        match self.rest.last_mut()? {
            BracketFrame::Dict { dict, .. } => Some(dict),
            _ => None,
        }
    }

    /// Unconditional pop of the topmost nested frame. Used by `)` which
    /// destructures all variants for distinct diagnostics.
    pub(super) fn pop_top(&mut self) -> Option<BracketFrame<'a>> {
        self.rest.pop()
    }

    pub(super) fn finish(self) -> Result<KExpression<'a>, KError> {
        if !self.rest.is_empty() {
            return Err(KError::parse(
                "open paren, bracket, or brace without matching close",
                None,
            ));
        }
        Ok(KExpression::new_from_iter(self.brand.region(), self.root))
    }
}

/// A token in flight: the byte range it occupies in the masked stream, plus the
/// original-source offset its span starts at. `masked_end` is recorded as each
/// codepoint is consumed — never read back from the reader at flush time, whose
/// position may already sit past a drained JUMP marker that follows the token.
pub(super) struct PendingToken {
    pub(super) masked_start: usize,
    pub(super) masked_end: usize,
    pub(super) span_start: u32,
}

/// Classify and push the pending token, if any. The token's text is a borrowed
/// slice of the masked stream — classification bumps every kept name into program
/// storage, so nothing outlives the borrow.
pub(super) fn flush_token<'a>(
    stack: &mut ParseStack<'a, '_>,
    pending: &mut Option<PendingToken>,
    masked: &[u8],
) -> Result<(), KError> {
    if let Some(tok) = pending.take() {
        let text = std::str::from_utf8(&masked[tok.masked_start..tok.masked_end])
            .expect("masked stream must be valid UTF-8");
        let part = classify_token(stack.brand(), stack.labels(), text, tok.span_start)?;
        stack.push_part(part)?;
    }
    Ok(())
}

/// Open shape shared by `[` and `{`: reject a glued opener, flush any
/// pending token into the parent, then push the new frame.
pub(super) fn open_collection<'a>(
    stack: &mut ParseStack<'a, '_>,
    pending: &mut Option<PendingToken>,
    masked: &[u8],
    opener: char,
    prev: Option<char>,
    frame: BracketFrame<'a>,
) -> Result<(), KError> {
    check_open_adjacency(opener, prev)?;
    flush_token(stack, pending, masked)?;
    stack.push_frame(frame);
    Ok(())
}

/// Close shape shared by `]` and `}`: verify the topmost frame matches
/// `closer`, run adjacency, flush any pending token, then pop and fold the
/// frame into the part it produces.
pub(super) fn close_collection<'a>(
    stack: &mut ParseStack<'a, '_>,
    pending: &mut Option<PendingToken>,
    masked: &[u8],
    closer: char,
    next: Option<char>,
    mismatch_msg: &str,
    end: u32,
) -> Result<(), KError> {
    let top_matches = stack.peek_top().is_some_and(|f| f.matches_closer(closer));
    if !top_matches {
        return Err(KError::parse(mismatch_msg, None));
    }
    check_close_adjacency(closer, next)?;
    flush_token(stack, pending, masked)?;
    let frame = stack
        .pop_top()
        .expect("peek_top.matches_closer checked above; flush_token preserves variant");
    let part = frame.into_part(stack.brand(), end, stack.labels())?;
    stack.push_part(part)?;
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
