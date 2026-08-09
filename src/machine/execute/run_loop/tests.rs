//! Scheduler tests, split by surface:
//!
//! - [`execute`], [`reclaim`], [`dep_finish`], [`dispatch`],
//!   [`lexical_provenance`], [`index_gated`], [`unified_walk`].
//! - [`dispatch_shapes`] — no-keyword shapes bypass
//!   `resolve_dispatch`; keyword-bearing shapes enter it.
//! - [`combined_binder_submission`] — a combined `LET <name> = FN …` statement installs both
//!   channels at submission, before any sibling can dispatch, closing the race independent of
//!   FIFO ordering.
//! - [`nested_binder_positions`] — the position rule: a binder outside statement position and a
//!   lazily-captured body is a TRY-catchable `NestedBinder` error.
//! - [`ambient_bracket`] — the slot-step bracket restores ambient values on
//!   unwind, not just on normal return.

mod ambient_bracket;
mod combined_binder_submission;
mod dep_finish;
mod dispatch;
mod dispatch_shapes;
mod execute;
mod index_gated;
mod lexical_provenance;
mod nested_binder_positions;
mod reclaim;
mod statement_binder_install;
mod unified_walk;

use crate::machine::core::ProgramStorage;
use crate::machine::model::{ExpressionPart, KExpression, KLiteral};
use crate::machine::model::{WorkingExpression, WorkingPart};
use crate::parse::parse;
use crate::source::Spanned;

/// Parse `src` and cross each top-level statement into the scheduler — the shape
/// [`dispatch_in_scope`](crate::machine::execute::KoanRuntime::dispatch_in_scope) and
/// [`enter_block`](crate::machine::execute::KoanRuntime::enter_block) take.
pub(super) fn working_all<'a>(
    program: &'a ProgramStorage,
    src: &str,
) -> Vec<WorkingExpression<'a>> {
    let brand = program.brand();
    parse(brand, src)
        .expect("parse should succeed")
        .into_iter()
        .map(|expr| WorkingExpression::from_ast(brand.region(), expr))
        .collect()
}

/// [`working_all`] for a source expected to hold exactly one statement.
pub(super) fn working_one<'a>(program: &'a ProgramStorage, src: &str) -> WorkingExpression<'a> {
    let mut all = working_all(program, src);
    assert_eq!(all.len(), 1, "test helper expects a single expression");
    all.remove(0)
}

/// Cross a hand-built AST node into the scheduler at the shared program brand.
pub(super) fn working<'a>(
    program: &'a ProgramStorage,
    expr: KExpression<'a>,
) -> WorkingExpression<'a> {
    WorkingExpression::from_ast(program.brand().region(), expr)
}

/// A bare `Keyword` node, for the tests that only need a distinguishable slot.
pub(super) fn keyword_expr<'a>(program: &'a ProgramStorage, name: &str) -> WorkingExpression<'a> {
    let brand = program.brand().region();
    WorkingExpression::new(
        brand,
        vec![Spanned::bare(WorkingPart::Ast(ExpressionPart::Keyword(
            brand.alloc_text(name),
        )))],
    )
}

/// `LET <name> = <value>` as parsed AST, so the node carries the binder plan a statement
/// submission installs from.
pub(super) fn let_ast<'a>(program: &'a ProgramStorage, name: &str, value: f64) -> KExpression<'a> {
    let brand = program.brand().region();
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

pub(super) fn let_expr<'a>(
    program: &'a ProgramStorage,
    name: &str,
    value: f64,
) -> WorkingExpression<'a> {
    working(program, let_ast(program, name, value))
}
