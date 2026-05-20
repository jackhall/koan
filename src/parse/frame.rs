//! `Frame` is one entry on the parse stack maintained by `build_tree`. Each variant
//! corresponds to one nesting shape — paren-expression, list literal, dict literal,
//! `:(...)` type-expression group. `into_part` folds a closed frame into the
//! `Spanned<ExpressionPart>` it produces; `matches_closer` is the variant↔closer lookup
//! the close-bracket arms use to decide whether the topmost frame legally ends here.
//!
//! Each variant carries `span_start: u32` — the original-source byte offset of the
//! opener — so frame close can stamp `Span { start: span_start, end: cursor }` on the
//! resulting node. The sigiled-Expression variant additionally carries `sigil_cursor`
//! so the outer `#(...)` / `$(...)` wrapper covers the sigil byte plus the body.

use crate::machine::core::source::{self, Span, Spanned};
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::KError;

use super::dict_literal::DictFrame;
use super::type_expr_frame::TypeExprFrame;

/// One frame of `build_tree`'s parse stack. `Expression::head` is `Some` only when the frame
/// was opened by a `#(...)` / `$(...)` sigil; on close such a frame yields the
/// `(QUOTE <body>)` / `(EVAL <body>)` AST shape rather than a bare `Expression` part.
pub(super) enum Frame<'a> {
    Expression {
        expr: KExpression<'a>,
        head: Option<&'static str>,
        span_start: u32,
        /// Set only when `head` is `Some(_)`: the cursor of the leading `#` / `$`.
        sigil_cursor: Option<u32>,
    },
    List {
        items: Vec<ExpressionPart<'a>>,
        span_start: u32,
    },
    Dict {
        dict: DictFrame<'a>,
        span_start: u32,
    },
    /// Opened by a glued `:(` sigil; contents parse as type-expression mode and the close
    /// folds into `ExpressionPart::Type(TypeExpr { ... })`. `span_start` is the cursor of
    /// the leading `:`, so the resulting Spanned wrapper covers the whole `:(...)`.
    TypeExpr {
        tef: TypeExprFrame<'a>,
        span_start: u32,
    },
}

impl<'a> Frame<'a> {
    /// Append a part to this frame. Span info is preserved when the frame's payload is a
    /// `Vec<Spanned<…>>` (Expression frame's KExpression); for List/Dict/TypeExpr the
    /// inner storage holds bare `ExpressionPart`s so the span is dropped at push time.
    pub(super) fn push(&mut self, part: Spanned<ExpressionPart<'a>>) {
        match self {
            Frame::Expression { expr, .. } => expr.parts.push(part),
            Frame::List { items, .. } => items.push(part.value),
            Frame::Dict { dict, .. } => dict.push(part.value),
            Frame::TypeExpr { tef, .. } => tef.parts.push(part.value),
        }
    }

    /// Fold this frame's contents into the `Spanned<ExpressionPart>` it produces when
    /// closed by its matching closer. `end` is the cursor just past the closer (=
    /// exclusive end of the span). Expression frames with a sigil head wrap as
    /// `(QUOTE body)` / `(EVAL body)` with the outer span covering sigil + body; the
    /// dict and type-expr variants run their deferred validation (`DictFrame::finish`,
    /// `TypeExprFrame::build`), which is the only failure path here. Closer-vs-variant
    /// mismatch is the caller's responsibility — this function trusts that the frame
    /// was matched against the right closer upstream.
    pub(super) fn into_part(self, end: u32) -> Result<Spanned<ExpressionPart<'a>>, KError> {
        let file = source::current();
        match self {
            Frame::Expression { mut expr, head: None, span_start, .. } => {
                let span = Span { start: span_start, end };
                expr.span = Some(span);
                expr.file = file;
                Ok(Spanned::at(ExpressionPart::Expression(Box::new(expr)), span))
            }
            Frame::Expression { mut expr, head: Some(head), span_start, sigil_cursor } => {
                let body_span = Span { start: span_start, end };
                expr.span = Some(body_span);
                expr.file = file;
                let sc = sigil_cursor
                    .expect("sigil-headed Expression frame must carry sigil_cursor");
                let outer_span = Span { start: sc, end };
                let sigil_span = Span { start: sc, end: sc + 1 };
                let wrapped = KExpression {
                    parts: vec![
                        Spanned::at(ExpressionPart::Keyword(head.to_string()), sigil_span),
                        Spanned::at(ExpressionPart::Expression(Box::new(expr)), body_span),
                    ],
                    span: Some(outer_span),
                    file,
                };
                Ok(Spanned::at(ExpressionPart::Expression(Box::new(wrapped)), outer_span))
            }
            Frame::List { items, span_start } => {
                let span = Span { start: span_start, end };
                Ok(Spanned::at(ExpressionPart::ListLiteral(items), span))
            }
            Frame::Dict { dict, span_start } => {
                let span = Span { start: span_start, end };
                Ok(Spanned::at(ExpressionPart::DictLiteral(dict.finish()?), span))
            }
            Frame::TypeExpr { tef, span_start } => {
                let span = Span { start: span_start, end };
                Ok(Spanned::at(ExpressionPart::Type(tef.build()?), span))
            }
        }
    }

    /// True iff `closer` is the legal end-token for this frame variant. Both Expression
    /// and TypeExpr frames close on `)`; the variant determines which builder runs.
    pub(super) fn matches_closer(&self, closer: char) -> bool {
        matches!(
            (self, closer),
            (Frame::Expression { .. }, ')')
                | (Frame::List { .. }, ']')
                | (Frame::Dict { .. }, '}')
                | (Frame::TypeExpr { .. }, ')')
        )
    }
}

/// `)`-close case-analysis. Reaching a `)` (literal or the synthetic close the whitespace
/// pass emits at end-of-expression) while a `[`/`{` frame is still open means that bracket
/// was never closed — report it as an unclosed `[`/`{` pointing at the opener, not as an
/// internal "closed paren" mismatch. Expression and TypeExpr frames delegate to
/// `Frame::into_part`.
pub(super) fn close_paren_to_part<'a>(
    frame: Frame<'a>,
    end: u32,
) -> Result<Spanned<ExpressionPart<'a>>, KError> {
    match frame {
        Frame::Expression { .. } => frame.into_part(end),
        Frame::TypeExpr { .. } => frame.into_part(end),
        Frame::List { span_start, .. } => Err(KError::parse(
            "unclosed '[': this list literal was never closed with a matching ']'",
            Some(Span { start: span_start, end: span_start + 1 }),
        )),
        Frame::Dict { span_start, .. } => Err(KError::parse(
            "unclosed '{': this dict literal was never closed with a matching '}'",
            Some(Span { start: span_start, end: span_start + 1 }),
        )),
    }
}
