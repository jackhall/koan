//! Scheduler tests, split by surface:
//!
//! - [`execute`], [`reclaim`], [`dep_finish`], [`dispatch`],
//!   [`lexical_provenance`], [`index_gated`], [`unified_walk`].
//! - [`dispatch_shapes`] — no-keyword shapes bypass
//!   `resolve_dispatch`; keyword-bearing shapes enter it.
//! - [`nested_binder_submission`] — recursive submission of binder-shaped
//!   sub-Dispatches at outermost-submission time installs nested binders'
//!   placeholders before any sibling can dispatch, closing the
//!   `LET f = (FN NAME [x] x)` race independent of FIFO ordering.

mod dep_finish;
mod dispatch;
mod dispatch_shapes;
mod execute;
mod index_gated;
mod lexical_provenance;
mod nested_binder_submission;
mod reclaim;
mod unified_walk;

use crate::machine::model::ast::{ExpressionPart, KExpression, KLiteral};
use crate::source::Spanned;

pub(super) fn let_expr<'run>(name: &str, value: f64) -> KExpression<'run> {
    KExpression::new(vec![
        Spanned::bare(ExpressionPart::Keyword("LET".into())),
        Spanned::bare(ExpressionPart::Identifier(name.into())),
        Spanned::bare(ExpressionPart::Keyword("=".into())),
        Spanned::bare(ExpressionPart::Literal(KLiteral::Number(value))),
    ])
}
