//! Scheduler tests, split by surface:
//!
//! - [`execute`], [`reclaim`], [`dep_finish`], [`dispatch`],
//!   [`lexical_provenance`], [`index_gated`], [`unified_walk`].
//! - [`dispatch_shapes`] — no-keyword shapes bypass
//!   `resolve_dispatch`; keyword-bearing shapes enter it.
//! - [`nested_binder_submission`] — the statement's cached binder aggregate
//!   installs every nested binder's placeholder at submission, before any
//!   sibling can dispatch, closing the `LET f = (FN NAME [x] x)` race
//!   independent of FIFO ordering.
//! - [`nested_binder_positions`] — the position rule: a binder in an eagerly
//!   dispatched value position is a TRY-catchable `NestedBinder` error.
//! - [`ambient_bracket`] — the slot-step bracket restores ambient values on
//!   unwind, not just on normal return.

mod ambient_bracket;
mod dep_finish;
mod dispatch;
mod dispatch_shapes;
mod execute;
mod index_gated;
mod lexical_provenance;
mod nested_binder_positions;
mod nested_binder_submission;
mod reclaim;
mod statement_binder_install;
mod unified_walk;

use crate::machine::model::{ExpressionPart, KExpression, KLiteral};
use crate::source::Spanned;

pub(super) fn let_expr<'run>(name: &str, value: f64) -> KExpression<'run> {
    KExpression::new(vec![
        Spanned::bare(ExpressionPart::Keyword("LET".into())),
        Spanned::bare(ExpressionPart::Identifier(name.into())),
        Spanned::bare(ExpressionPart::Keyword("=".into())),
        Spanned::bare(ExpressionPart::Literal(KLiteral::Number(value))),
    ])
}
