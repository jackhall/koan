//! Tests for the scheduler, split by surface:
//!
//! - [`execute`] — basic dispatch ordering and inter-expression lookup.
//! - [`reclaim`] — `free` / node-reclamation invariants.
//! - [`combine`] — combine, defer_to, and tail-call slot reuse.
//! - [`dispatch`] — overload routing rules end-to-end through the scheduler.
//! - [`dispatch_shapes`] — `DispatchShape` classification + no-keyword
//!   fast-lane routing (the four no-keyword shapes bypass
//!   `resolve_dispatch_with_chain`; keyword-bearing shapes enter it).
//! - [`unified_walk`] — cache-driven strict-only dispatch surface tests not
//!   covered elsewhere (self-reference, bare-name forward ref to a
//!   nominal-binder placeholder).
//! - [`lexical_provenance`] — `LexicalFrame` chain attachment + assembly.
//! - [`index_gated`] — index-gated resolution end-to-end (forward / backward refs,
//!   value-style vs nominal-binder visibility, overload pre-filter, type-side gate,
//!   mutual recursion across nominal binders).
//! - [`nested_binder_submission`] — recursive submission of binder-shaped
//!   sub-Dispatches at outermost-submission time, so nested binders' placeholders
//!   install before any sibling can dispatch (closes the `LET f = (FN NAME [x] x)`
//!   race independent of FIFO ordering).

mod combine;
mod dispatch;
mod dispatch_shapes;
mod execute;
mod index_gated;
mod lexical_provenance;
mod nested_binder_submission;
mod reclaim;
mod unified_walk;

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
