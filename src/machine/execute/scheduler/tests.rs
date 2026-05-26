//! Tests for the scheduler, split by surface:
//!
//! - [`execute`] — basic dispatch ordering and inter-expression lookup.
//! - [`reclaim`] — `free` / node-reclamation invariants.
//! - [`combine`] — combine, defer_to, and tail-call slot reuse.
//! - [`dispatch`] — overload routing rules end-to-end through the scheduler.
//! - [`lexical_provenance`] — `LexicalFrame` chain attachment + assembly.

mod combine;
mod dispatch;
mod execute;
mod lexical_provenance;
mod reclaim;

use crate::machine::core::source::Spanned;
use crate::machine::model::ast::{ExpressionPart, KExpression, KLiteral};

pub(super) fn let_expr<'a>(name: &str, value: f64) -> KExpression<'a> {
    KExpression::new(vec![
        Spanned::bare(ExpressionPart::Keyword("LET".into())),
        Spanned::bare(ExpressionPart::Identifier(name.into())),
        Spanned::bare(ExpressionPart::Keyword("=".into())),
        Spanned::bare(ExpressionPart::Literal(KLiteral::Number(value))),
    ])
}
