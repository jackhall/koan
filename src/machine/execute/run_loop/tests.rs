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

use crate::builtins::test_support::program_brand;
use crate::machine::model::{ExpressionPart, KExpression, KLiteral};
use crate::machine::model::{WorkingExpression, WorkingPart};
use crate::parse::parse;
use crate::source::Spanned;

/// Parse `src` and cross each top-level statement into the scheduler — the shape
/// [`dispatch_in_scope`](crate::machine::execute::KoanRuntime::dispatch_in_scope) and
/// [`enter_block`](crate::machine::execute::KoanRuntime::enter_block) take.
pub(super) fn working_all(src: &str) -> Vec<WorkingExpression<'static>> {
    let brand = program_brand();
    parse(brand, src)
        .expect("parse should succeed")
        .into_iter()
        .map(|expr| WorkingExpression::from_ast(brand.region(), expr))
        .collect()
}

/// [`working_all`] for a source expected to hold exactly one statement.
pub(super) fn working_one(src: &str) -> WorkingExpression<'static> {
    let mut all = working_all(src);
    assert_eq!(all.len(), 1, "test helper expects a single expression");
    all.remove(0)
}

/// Cross a hand-built AST node into the scheduler at the shared program brand.
pub(super) fn working(expr: KExpression<'static>) -> WorkingExpression<'static> {
    WorkingExpression::from_ast(program_brand().region(), expr)
}

/// A bare `Keyword` node, for the tests that only need a distinguishable slot.
pub(super) fn keyword_expr(name: &str) -> WorkingExpression<'static> {
    let brand = program_brand().region();
    WorkingExpression::new(
        brand,
        vec![Spanned::bare(WorkingPart::Ast(ExpressionPart::Keyword(
            brand.alloc_text(name),
        )))],
    )
}

/// `LET <name> = <value>` as parsed AST, so the node carries the binder plan a statement
/// submission installs from.
pub(super) fn let_ast(name: &str, value: f64) -> KExpression<'static> {
    let brand = program_brand().region();
    KExpression::new(
        brand,
        vec![
            Spanned::bare(ExpressionPart::Keyword(brand.alloc_text("LET"))),
            Spanned::bare(ExpressionPart::Identifier(brand.alloc_text(name))),
            Spanned::bare(ExpressionPart::Keyword(brand.alloc_text("="))),
            Spanned::bare(ExpressionPart::Literal(KLiteral::Number(value))),
        ],
    )
}

pub(super) fn let_expr(name: &str, value: f64) -> WorkingExpression<'static> {
    working(let_ast(name, value))
}
