//! Tests for the scheduler, split by surface:
//!
//! - [`execute`] — basic dispatch ordering and inter-expression lookup.
//! - [`reclaim`] — `free` / node-reclamation invariants.
//! - [`combine`] — combine, defer_to, and tail-call slot reuse.
//! - [`dispatch`] — overload routing rules end-to-end through the scheduler.

mod combine;
mod dispatch;
mod execute;
mod reclaim;

use crate::machine::model::ast::{ExpressionPart, KExpression, KLiteral};

pub(super) fn let_expr<'a>(name: &str, value: f64) -> KExpression<'a> {
    KExpression {
        parts: vec![
            ExpressionPart::Keyword("LET".into()),
            ExpressionPart::Identifier(name.into()),
            ExpressionPart::Keyword("=".into()),
            ExpressionPart::Literal(KLiteral::Number(value)),
        ],
    }
}
