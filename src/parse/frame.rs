//! One entry on `build_tree`'s parse stack — one variant per nesting shape
//! (paren-expression, list literal, dict literal, `:(...)` type-expression group).
//!
//! `span_start` is the opener's original-source byte offset, used to stamp
//! `Span { start: span_start, end: cursor }` at close time. The sigiled-Expression
//! variant additionally carries `sigil_cursor` so the outer `#(...)` / `$(...)`
//! wrapper covers the sigil byte plus the body.

use crate::machine::core::source::{self, Span, Spanned};
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::KError;

use super::dict_literal::{BraceContents, DictFrame};

pub(super) enum Frame<'a> {
    /// `head: Some(_)` flags a `#(...)` / `$(...)` sigil; on close such a frame yields
    /// the `(QUOTE <body>)` / `(EVAL <body>)` AST shape rather than a bare Expression
    /// part, and `sigil_cursor` (set iff `head` is) anchors the outer span at the sigil.
    Expression {
        expr: KExpression<'a>,
        head: Option<&'static str>,
        span_start: u32,
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
    /// Opened by a glued `:(` sigil. The inner expression is stored verbatim and folded
    /// into [`ExpressionPart::SigiledTypeExpr`] — shape recognition is the dispatcher's
    /// job. `span_start` is the cursor of the leading `:`.
    TypeExpr {
        expr: KExpression<'a>,
        span_start: u32,
    },
    /// Opened by a glued `:{` sigil. Collects a typed field list verbatim and folds into
    /// `SigiledTypeExpr([Keyword("RECORD"), Expression(<field list>)])` so the internal
    /// `RECORD` type-constructor overload builds the `KType::Record`. `span_start` is the
    /// cursor of the leading `:`.
    RecordTypeExpr {
        expr: KExpression<'a>,
        span_start: u32,
    },
}

impl<'a> Frame<'a> {
    /// Spans are preserved on Expression and TypeExpr (whose payload is a
    /// `Vec<Spanned<…>>`); List and Dict store bare parts so the span is dropped here.
    pub(super) fn push(&mut self, part: Spanned<ExpressionPart<'a>>) {
        match self {
            Frame::Expression { expr, .. } => expr.parts.push(part),
            Frame::List { items, .. } => items.push(part.value),
            Frame::Dict { dict, .. } => dict.push(part.value),
            Frame::TypeExpr { expr, .. } => expr.parts.push(part),
            Frame::RecordTypeExpr { expr, .. } => expr.parts.push(part),
        }
    }

    /// `end` is the cursor just past the closer (exclusive end of the span). The only
    /// failure path is `DictFrame::finish` for the Dict variant; closer-vs-variant
    /// pairing is assumed valid (see `matches_closer`).
    pub(super) fn into_part(self, end: u32) -> Result<Spanned<ExpressionPart<'a>>, KError> {
        let file = source::current();
        match self {
            Frame::Expression {
                mut expr,
                head: None,
                span_start,
                ..
            } => {
                let span = Span {
                    start: span_start,
                    end,
                };
                expr.span = Some(span);
                expr.file = file;
                Ok(Spanned::at(
                    ExpressionPart::Expression(Box::new(expr)),
                    span,
                ))
            }
            Frame::Expression {
                mut expr,
                head: Some(head),
                span_start,
                sigil_cursor,
            } => {
                let body_span = Span {
                    start: span_start,
                    end,
                };
                expr.span = Some(body_span);
                expr.file = file;
                let sc =
                    sigil_cursor.expect("sigil-headed Expression frame must carry sigil_cursor");
                let outer_span = Span { start: sc, end };
                let sigil_span = Span {
                    start: sc,
                    end: sc + 1,
                };
                let wrapped = KExpression {
                    parts: vec![
                        Spanned::at(ExpressionPart::Keyword(head.to_string()), sigil_span),
                        Spanned::at(ExpressionPart::Expression(Box::new(expr)), body_span),
                    ],
                    span: Some(outer_span),
                    file,
                };
                Ok(Spanned::at(
                    ExpressionPart::Expression(Box::new(wrapped)),
                    outer_span,
                ))
            }
            Frame::List { items, span_start } => {
                let span = Span {
                    start: span_start,
                    end,
                };
                Ok(Spanned::at(ExpressionPart::ListLiteral(items), span))
            }
            Frame::Dict { dict, span_start } => {
                let span = Span {
                    start: span_start,
                    end,
                };
                let part = match dict.finish()? {
                    BraceContents::Dict(pairs) => ExpressionPart::DictLiteral(pairs),
                    BraceContents::Record(fields) => ExpressionPart::RecordLiteral(fields),
                };
                Ok(Spanned::at(part, span))
            }
            Frame::TypeExpr {
                mut expr,
                span_start,
            } => {
                let span = Span {
                    start: span_start,
                    end,
                };
                expr.span = Some(span);
                expr.file = file;
                Ok(Spanned::at(
                    ExpressionPart::SigiledTypeExpr(Box::new(expr)),
                    span,
                ))
            }
            // `:{x :Number}` → `SigiledTypeExpr([Keyword("RECORD"), Expression(<fields>)])`,
            // routed through the internal RECORD type-constructor overload.
            Frame::RecordTypeExpr {
                mut expr,
                span_start,
            } => {
                let span = Span {
                    start: span_start,
                    end,
                };
                expr.span = Some(span);
                expr.file = file;
                let sigil_span = Span {
                    start: span_start,
                    end: span_start + 1,
                };
                let wrapped = KExpression {
                    parts: vec![
                        Spanned::at(ExpressionPart::Keyword("RECORD".to_string()), sigil_span),
                        Spanned::at(ExpressionPart::Expression(Box::new(expr)), span),
                    ],
                    span: Some(span),
                    file,
                };
                Ok(Spanned::at(
                    ExpressionPart::SigiledTypeExpr(Box::new(wrapped)),
                    span,
                ))
            }
        }
    }

    /// Expression and TypeExpr both close on `)`; the variant determines which
    /// builder runs in `into_part`.
    pub(super) fn matches_closer(&self, closer: char) -> bool {
        matches!(
            (self, closer),
            (Frame::Expression { .. }, ')')
                | (Frame::List { .. }, ']')
                | (Frame::Dict { .. }, '}')
                | (Frame::TypeExpr { .. }, ')')
                | (Frame::RecordTypeExpr { .. }, '}')
        )
    }
}

/// A `)` reaching a List/Dict frame means the `[`/`{` was never closed; report it as
/// an unclosed bracket pointing at the opener rather than a paren mismatch.
pub(super) fn close_paren_to_part<'a>(
    frame: Frame<'a>,
    end: u32,
) -> Result<Spanned<ExpressionPart<'a>>, KError> {
    match frame {
        Frame::Expression { .. } => frame.into_part(end),
        Frame::TypeExpr { .. } => frame.into_part(end),
        Frame::RecordTypeExpr { span_start, .. } => Err(KError::parse(
            "unclosed ':{': this record type was never closed with a matching '}'",
            Some(Span {
                start: span_start,
                end: span_start + 1,
            }),
        )),
        Frame::List { span_start, .. } => Err(KError::parse(
            "unclosed '[': this list literal was never closed with a matching ']'",
            Some(Span {
                start: span_start,
                end: span_start + 1,
            }),
        )),
        Frame::Dict { span_start, .. } => Err(KError::parse(
            "unclosed '{': this dict literal was never closed with a matching '}'",
            Some(Span {
                start: span_start,
                end: span_start + 1,
            }),
        )),
    }
}
