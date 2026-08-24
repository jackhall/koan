//! Scheduler tests, one module per surface, over the shared expression-building helpers below.
//! Each submodule's own header states the rule it pins.

mod ambient_bracket;
mod combined_binder_submission;
mod dep_finish;
mod dispatch;
mod dispatch_shapes;
mod edge_wiring;
mod execute;
mod index_gated;
mod lexical_provenance;
mod nested_binder_positions;
mod reclaim;
mod splice_walk;
mod statement_binder_install;

use crate::builtins::test_support::kw_part;
use crate::machine::core::ProgramStorage;
use crate::machine::model::{ExpressionPart, KExpression, KLiteral};
use crate::machine::model::{WorkingExpression, WorkingPart};
use crate::parse::parse;
use crate::source::Spanned;

/// Parse `src` into the shape
/// [`dispatch_in_scope`](crate::machine::execute::KoanRuntime::dispatch_in_scope) and
/// [`enter_block`](crate::machine::execute::KoanRuntime::enter_block) take.
pub(super) fn working_all<'a>(
    program: &'a ProgramStorage,
    labels: &crate::machine::model::LabelInterner,
    src: &str,
) -> Vec<WorkingExpression<'a>> {
    let brand = program.brand();
    parse(brand, labels, src)
        .expect("parse should succeed")
        .into_iter()
        .map(|expr| WorkingExpression::from_ast(brand.region(), expr))
        .collect()
}

/// Wire a watch edge onto every slot in `ids`, destined at `scope`'s own region. Slots reclaim at
/// finalize, so a reader must hold an edge, wired before `execute` as production does.
pub(super) fn watch_all(
    runtime: &mut crate::machine::execute::KoanRuntime<'_>,
    ids: &[crate::scheduler::NodeId],
    scope: &crate::machine::Scope<'_>,
) -> Vec<crate::scheduler::EdgeId> {
    ids.iter()
        .map(|&id| runtime.install_edge_for_test(id, scope))
        .collect()
}

/// [`working_all`] for a source expected to hold exactly one statement.
pub(super) fn working_one<'a>(
    program: &'a ProgramStorage,
    labels: &crate::machine::model::LabelInterner,
    src: &str,
) -> WorkingExpression<'a> {
    let mut all = working_all(program, labels, src);
    assert_eq!(all.len(), 1, "test helper expects a single expression");
    all.remove(0)
}

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
        &[Spanned::bare(WorkingPart::Ast(kw_part(
            brand.allocator().text(name),
        )))],
    )
}

/// `LET <name> = <value>` at AST level, so the node carries the binder plan a statement
/// submission installs from.
pub(super) fn let_ast<'a>(program: &'a ProgramStorage, name: &str, value: f64) -> KExpression<'a> {
    let brand = program.brand().region();
    KExpression::new(
        brand,
        &[
            Spanned::bare(kw_part("LET")),
            Spanned::bare(ExpressionPart::Identifier(brand.allocator().text(name))),
            Spanned::bare(kw_part("=")),
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
