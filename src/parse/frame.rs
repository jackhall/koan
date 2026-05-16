//! `Frame` is one entry on the parse stack maintained by `build_tree`. Each variant
//! corresponds to one nesting shape — paren-expression, list literal, dict literal,
//! `:(...)` type-expression group. `into_part` folds a closed frame into the
//! `ExpressionPart` it produces; `matches_closer` is the variant↔closer lookup the
//! close-bracket arms use to decide whether the topmost frame legally ends here.

use crate::machine::model::ast::{ExpressionPart, KExpression};

use super::dict_literal::DictFrame;
use super::type_expr_frame::TypeExprFrame;

/// One frame of `build_tree`'s parse stack. `Expression::head` is `Some` only when the frame
/// was opened by a `#(...)` / `$(...)` sigil; on close such a frame yields the
/// `(QUOTE <body>)` / `(EVAL <body>)` AST shape rather than a bare `Expression` part.
pub(super) enum Frame<'a> {
    Expression {
        expr: KExpression<'a>,
        head: Option<&'static str>,
    },
    List(Vec<ExpressionPart<'a>>),
    Dict(DictFrame<'a>),
    /// Opened by a glued `:(` sigil; contents parse as type-expression mode and the close
    /// folds into `ExpressionPart::Type(TypeExpr { ... })`.
    TypeExpr(TypeExprFrame<'a>),
}

impl<'a> Frame<'a> {
    pub(super) fn push(&mut self, part: ExpressionPart<'a>) {
        match self {
            Frame::Expression { expr, .. } => expr.parts.push(part),
            Frame::List(items) => items.push(part),
            Frame::Dict(d) => d.push(part),
            Frame::TypeExpr(tf) => tf.parts.push(part),
        }
    }

    /// Returns `None` for `Dict` (its state machine doesn't expose a flat last-part).
    pub(super) fn last_part(&self) -> Option<&ExpressionPart<'a>> {
        match self {
            Frame::Expression { expr, .. } => expr.parts.last(),
            Frame::List(items) => items.last(),
            Frame::TypeExpr(tf) => tf.parts.last(),
            Frame::Dict(_) => None,
        }
    }

    /// Symmetric to `last_part`: `Dict` returns `None`.
    pub(super) fn pop_last_part(&mut self) -> Option<ExpressionPart<'a>> {
        match self {
            Frame::Expression { expr, .. } => expr.parts.pop(),
            Frame::List(items) => items.pop(),
            Frame::TypeExpr(tf) => tf.parts.pop(),
            Frame::Dict(_) => None,
        }
    }

    /// Fold this frame's contents into the `ExpressionPart` it produces when closed by
    /// its matching closer. Expression frames with a sigil head wrap as `(QUOTE body)` /
    /// `(EVAL body)`; the dict and type-expr variants run their deferred validation
    /// (`DictFrame::finish`, `TypeExprFrame::build`), which is the only failure path here.
    /// Closer-vs-variant mismatch is the caller's responsibility — this function trusts
    /// that the frame was matched against the right closer upstream.
    pub(super) fn into_part(self) -> Result<ExpressionPart<'a>, String> {
        match self {
            Frame::Expression { expr, head: None } => Ok(ExpressionPart::Expression(Box::new(expr))),
            Frame::Expression { expr, head: Some(head) } => {
                let wrapped = KExpression {
                    parts: vec![
                        ExpressionPart::Keyword(head.to_string()),
                        ExpressionPart::Expression(Box::new(expr)),
                    ],
                };
                Ok(ExpressionPart::Expression(Box::new(wrapped)))
            }
            Frame::List(items) => Ok(ExpressionPart::ListLiteral(items)),
            Frame::Dict(d) => Ok(ExpressionPart::DictLiteral(d.finish()?)),
            Frame::TypeExpr(tf) => Ok(ExpressionPart::Type(tf.build()?)),
        }
    }

    /// True iff `closer` is the legal end-token for this frame variant. Both Expression
    /// and TypeExpr frames close on `)`; the variant determines which builder runs.
    pub(super) fn matches_closer(&self, closer: char) -> bool {
        matches!(
            (self, closer),
            (Frame::Expression { .. }, ')')
                | (Frame::List(_), ']')
                | (Frame::Dict(_), '}')
                | (Frame::TypeExpr(_), ')')
        )
    }
}

/// `)`-close case-analysis. Reject the two non-paren frame variants with a diagnostic
/// that names the actual frame that bled through to the `)`; for Expression and TypeExpr
/// frames, delegate to `Frame::into_part`.
pub(super) fn close_paren_to_part<'a>(frame: Frame<'a>) -> Result<ExpressionPart<'a>, String> {
    match frame {
        Frame::Expression { .. } => frame.into_part(),
        Frame::TypeExpr(_) => frame.into_part(),
        Frame::List(_) => Err("closed paren but innermost frame is a list literal".to_string()),
        Frame::Dict(_) => Err("closed paren but innermost frame is a dict literal".to_string()),
    }
}
