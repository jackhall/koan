//! One entry on `build_tree`'s parse stack — one variant per nesting shape
//! (paren-expression, list literal, dict literal, `:(...)` type-expression group).
//!
//! `span_start` is the opener's original-source byte offset, used to stamp
//! `Span { start: span_start, end: cursor }` at close time. The `$(...)` Expression
//! frame and the `#(...)` Quote frame additionally carry `sigil_cursor` so the outer
//! part's span covers the sigil byte plus the body.

use crate::machine::KError;
use crate::machine::core::ProgramBrand;
use crate::machine::model::ast::ExpressionPart;
use crate::source::{self, Span, Spanned};

use super::dict_literal::{BraceContents, DictFrame};

/// An open frame collects its parts in a plain `Vec` and freezes them into a node only at
/// [`BracketFrame::into_part`], where the run is complete — a node's parts and structural cache are
/// bumped together and never touched again.
pub(super) enum BracketFrame<'a> {
    /// `head: Some(_)` flags a `$(...)` sigil; on close such a frame yields the
    /// `(EVAL <body>)` AST shape rather than a bare Expression part, and `sigil_cursor`
    /// (set iff `head` is) anchors the outer span at the sigil.
    Expression {
        parts: Vec<Spanned<ExpressionPart<'a>>>,
        head: Option<&'static str>,
        span_start: u32,
        sigil_cursor: Option<u32>,
    },
    /// Opened by a `#(` sigil. On close the body folds into an
    /// [`ExpressionPart::QuotedExpression`] — a parse-static capture, not a call — so no
    /// keyword is prepended. `span_start` is the `(` cursor (the body's own span) and
    /// `sigil_cursor` the `#`, which the outer part's span starts at.
    Quote {
        parts: Vec<Spanned<ExpressionPart<'a>>>,
        span_start: u32,
        sigil_cursor: u32,
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
        parts: Vec<Spanned<ExpressionPart<'a>>>,
        span_start: u32,
    },
    /// Opened by a glued `:{` sigil. Collects a typed field list verbatim and folds into a
    /// first-class [`ExpressionPart::RecordType`] the elaborator turns into a `KType::Record`
    /// directly. `span_start` is the cursor of the leading `:`.
    RecordTypeExpr {
        parts: Vec<Spanned<ExpressionPart<'a>>>,
        span_start: u32,
    },
}

impl<'a> BracketFrame<'a> {
    /// Spans are preserved on the node-building variants (Expression, Quote, SigiledTypeExpr,
    /// RecordTypeExpr), whose run is a `Vec<Spanned<…>>`; List and Dict store bare parts so the
    /// span is dropped here.
    pub(super) fn push(&mut self, part: Spanned<ExpressionPart<'a>>) {
        match self {
            BracketFrame::Expression { parts, .. } => parts.push(part),
            BracketFrame::Quote { parts, .. } => parts.push(part),
            BracketFrame::List { items, .. } => items.push(part.value),
            BracketFrame::Dict { dict, .. } => dict.push(part.value),
            BracketFrame::SigiledTypeExpr { parts, .. } => parts.push(part),
            BracketFrame::RecordTypeExpr { parts, .. } => parts.push(part),
        }
    }

    /// `end` is the cursor just past the closer (exclusive end of the span). The collected run is
    /// complete here, so this is where each frame freezes it into a node through
    /// [`ProgramBrand::build_expression`] and bumps it into program storage. The only failure path is
    /// `DictFrame::finish` for the Dict variant; closer-vs-variant pairing is assumed valid (see
    /// `matches_closer`).
    pub(super) fn into_part(
        self,
        brand: ProgramBrand<'a>,
        end: u32,
    ) -> Result<Spanned<ExpressionPart<'a>>, KError> {
        let file = source::current();
        match self {
            BracketFrame::Expression {
                parts,
                head: None,
                span_start,
                ..
            } => {
                let span = Span {
                    start: span_start,
                    end,
                };
                let expr = brand.build_expression(parts, Some(span), file);
                Ok(Spanned::at(
                    ExpressionPart::Expression(brand.alloc_node(expr)),
                    span,
                ))
            }
            BracketFrame::Expression {
                parts,
                head: Some(head),
                span_start,
                sigil_cursor,
            } => {
                let body_span = Span {
                    start: span_start,
                    end,
                };
                let expr = brand.build_expression(parts, Some(body_span), file);
                let sc =
                    sigil_cursor.expect("sigil-headed Expression frame must carry sigil_cursor");
                let outer_span = Span { start: sc, end };
                let sigil_span = Span {
                    start: sc,
                    end: sc + 1,
                };
                let wrapped = brand.build_expression(
                    vec![
                        Spanned::at(ExpressionPart::Keyword(head), sigil_span),
                        Spanned::at(
                            ExpressionPart::Expression(brand.alloc_node(expr)),
                            body_span,
                        ),
                    ],
                    Some(outer_span),
                    file,
                );
                Ok(Spanned::at(
                    ExpressionPart::Expression(brand.alloc_node(wrapped)),
                    outer_span,
                ))
            }
            // `#(...)`: the body keeps the paren span, the captured part covers the sigil too.
            BracketFrame::Quote {
                parts,
                span_start,
                sigil_cursor,
            } => {
                let body_span = Span {
                    start: span_start,
                    end,
                };
                let expr = brand.build_expression(parts, Some(body_span), file);
                let outer_span = Span {
                    start: sigil_cursor,
                    end,
                };
                Ok(Spanned::at(
                    ExpressionPart::QuotedExpression(brand.alloc_node(expr)),
                    outer_span,
                ))
            }
            BracketFrame::List { items, span_start } => {
                let span = Span {
                    start: span_start,
                    end,
                };
                Ok(Spanned::at(
                    ExpressionPart::ListLiteral(brand.region().allocator().slice(&items)),
                    span,
                ))
            }
            BracketFrame::Dict { dict, span_start } => {
                let span = Span {
                    start: span_start,
                    end,
                };
                let part = match dict.finish()? {
                    BraceContents::Dict(pairs) => {
                        ExpressionPart::DictLiteral(brand.region().allocator().slice(&pairs))
                    }
                    BraceContents::Record(fields) => {
                        ExpressionPart::RecordLiteral(brand.region().allocator().slice(&fields))
                    }
                };
                Ok(Spanned::at(part, span))
            }
            BracketFrame::SigiledTypeExpr { parts, span_start } => {
                let span = Span {
                    start: span_start,
                    end,
                };
                let expr = brand.build_expression(parts, Some(span), file);
                Ok(Spanned::at(
                    ExpressionPart::SigiledTypeExpr(brand.alloc_node(expr)),
                    span,
                ))
            }
            // `:{x :Number}` → `RecordType(<field list>)` — a first-class part the
            // elaborator folds straight to `KType::Record`. The inner `KExpression` is the
            // bare `(x :Number, …)` field list; `span_start` is the leading `:`.
            BracketFrame::RecordTypeExpr { parts, span_start } => {
                let span = Span {
                    start: span_start,
                    end,
                };
                let expr = brand.build_expression(parts, Some(span), file);
                Ok(Spanned::at(
                    ExpressionPart::RecordType(brand.alloc_node(expr)),
                    span,
                ))
            }
        }
    }

    /// Expression, Quote and SigiledTypeExpr all close on `)`; the variant determines which
    /// builder runs in `into_part`.
    pub(super) fn matches_closer(&self, closer: char) -> bool {
        matches!(
            (self, closer),
            (BracketFrame::Expression { .. }, ')')
                | (BracketFrame::Quote { .. }, ')')
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
    brand: ProgramBrand<'a>,
    frame: BracketFrame<'a>,
    end: u32,
) -> Result<Spanned<ExpressionPart<'a>>, KError> {
    match frame {
        BracketFrame::Expression { .. } => frame.into_part(brand, end),
        BracketFrame::Quote { .. } => frame.into_part(brand, end),
        BracketFrame::SigiledTypeExpr { .. } => frame.into_part(brand, end),
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
