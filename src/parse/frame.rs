//! One entry on `build_tree`'s parse stack — one variant per nesting shape
//! (paren-expression, list literal, dict literal, `:(...)` type-expression group).
//!
//! `span_start` is the opener's original-source byte offset, used to stamp
//! `Span { start: span_start, end: cursor }` at close time. The sigiled-Expression
//! variant additionally carries `sigil_cursor` so the outer `#(...)` / `$(...)`
//! wrapper covers the sigil byte plus the body.

use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::KError;
use crate::source::{self, Span, Spanned};

use super::dict_literal::{BraceContents, DictFrame};

pub(super) enum BracketFrame<'a> {
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
    SigiledTypeExpr {
        expr: KExpression<'a>,
        span_start: u32,
    },
    /// Opened by a glued `:{` sigil. Collects a typed field list verbatim and folds into a
    /// first-class [`ExpressionPart::RecordType`] the elaborator turns into a `KType::Record`
    /// directly. `span_start` is the cursor of the leading `:`.
    RecordTypeExpr {
        expr: KExpression<'a>,
        span_start: u32,
    },
}

impl<'a> BracketFrame<'a> {
    /// Spans are preserved on Expression and SigiledTypeExpr (whose payload is a
    /// `Vec<Spanned<…>>`); List and Dict store bare parts so the span is dropped here.
    pub(super) fn push(&mut self, part: Spanned<ExpressionPart<'a>>) {
        match self {
            BracketFrame::Expression { expr, .. } => expr.parts.push(part),
            BracketFrame::List { items, .. } => items.push(part.value),
            BracketFrame::Dict { dict, .. } => dict.push(part.value),
            BracketFrame::SigiledTypeExpr { expr, .. } => expr.parts.push(part),
            BracketFrame::RecordTypeExpr { expr, .. } => expr.parts.push(part),
        }
    }

    /// `end` is the cursor just past the closer (exclusive end of the span). The only
    /// failure path is `DictFrame::finish` for the Dict variant; closer-vs-variant
    /// pairing is assumed valid (see `matches_closer`).
    pub(super) fn into_part(self, end: u32) -> Result<Spanned<ExpressionPart<'a>>, KError> {
        let file = source::current();
        match self {
            BracketFrame::Expression {
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
                // Parts were pushed incrementally; refresh the structural cache now
                // that the vector is final.
                expr.fill_cache();
                Ok(Spanned::at(
                    ExpressionPart::Expression(Box::new(expr)),
                    span,
                ))
            }
            BracketFrame::Expression {
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
                expr.fill_cache();
                let sc =
                    sigil_cursor.expect("sigil-headed Expression frame must carry sigil_cursor");
                let outer_span = Span { start: sc, end };
                let sigil_span = Span {
                    start: sc,
                    end: sc + 1,
                };
                let wrapped = KExpression::build(
                    vec![
                        Spanned::at(ExpressionPart::Keyword(head.to_string()), sigil_span),
                        Spanned::at(ExpressionPart::Expression(Box::new(expr)), body_span),
                    ],
                    Some(outer_span),
                    file,
                );
                Ok(Spanned::at(
                    ExpressionPart::Expression(Box::new(wrapped)),
                    outer_span,
                ))
            }
            BracketFrame::List { items, span_start } => {
                let span = Span {
                    start: span_start,
                    end,
                };
                Ok(Spanned::at(ExpressionPart::ListLiteral(items), span))
            }
            BracketFrame::Dict { dict, span_start } => {
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
            BracketFrame::SigiledTypeExpr {
                mut expr,
                span_start,
            } => {
                let span = Span {
                    start: span_start,
                    end,
                };
                expr.span = Some(span);
                expr.file = file;
                expr.fill_cache();
                Ok(Spanned::at(
                    ExpressionPart::SigiledTypeExpr(Box::new(expr)),
                    span,
                ))
            }
            // `:{x :Number}` → `RecordType(<field list>)` — a first-class part the
            // elaborator folds straight to `KType::Record`. The inner `KExpression` is the
            // bare `(x :Number, …)` field list; `span_start` is the leading `:`.
            BracketFrame::RecordTypeExpr {
                mut expr,
                span_start,
            } => {
                let span = Span {
                    start: span_start,
                    end,
                };
                expr.span = Some(span);
                expr.file = file;
                expr.fill_cache();
                Ok(Spanned::at(
                    ExpressionPart::RecordType(Box::new(expr)),
                    span,
                ))
            }
        }
    }

    /// Expression and SigiledTypeExpr both close on `)`; the variant determines which
    /// builder runs in `into_part`.
    pub(super) fn matches_closer(&self, closer: char) -> bool {
        matches!(
            (self, closer),
            (BracketFrame::Expression { .. }, ')')
                | (BracketFrame::List { .. }, ']')
                | (BracketFrame::Dict { .. }, '}')
                | (BracketFrame::SigiledTypeExpr { .. }, ')')
                | (BracketFrame::RecordTypeExpr { .. }, '}')
        )
    }
}

/// A `)` reaching a List/Dict frame means the `[`/`{` was never closed; report it as
/// an unclosed bracket pointing at the opener rather than a paren mismatch.
pub(super) fn close_paren_to_part<'a>(
    frame: BracketFrame<'a>,
    end: u32,
) -> Result<Spanned<ExpressionPart<'a>>, KError> {
    match frame {
        BracketFrame::Expression { .. } => frame.into_part(end),
        BracketFrame::SigiledTypeExpr { .. } => frame.into_part(end),
        BracketFrame::RecordTypeExpr { span_start, .. } => Err(KError::parse(
            "unclosed ':{': this record type was never closed with a matching '}'",
            Some(Span {
                start: span_start,
                end: span_start + 1,
            }),
        )),
        BracketFrame::List { span_start, .. } => Err(KError::parse(
            "unclosed '[': this list literal was never closed with a matching ']'",
            Some(Span {
                start: span_start,
                end: span_start + 1,
            }),
        )),
        BracketFrame::Dict { span_start, .. } => Err(KError::parse(
            "unclosed '{': this dict literal was never closed with a matching '}'",
            Some(Span {
                start: span_start,
                end: span_start + 1,
            }),
        )),
    }
}
