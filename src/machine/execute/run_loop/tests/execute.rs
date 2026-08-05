//! Basic dispatch ordering and inter-expression lookup.

use crate::builtins::test_support::TestRun;
use crate::machine::core::{program_storage, run_root_storage};
use crate::machine::model::KObject;
use crate::machine::model::{ExpressionPart, KExpression};
use crate::source::Spanned;

use super::{let_expr, working};

#[test]
fn dispatches_independent_expressions_in_order() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let root = test_run.scope;
    let runtime = &mut test_run.runtime;
    let ids = runtime.enter_block(
        root.id,
        vec![let_expr(&program, "x", 1.0), let_expr(&program, "y", 2.0)],
        root,
    );
    let id1 = ids[0];
    let id2 = ids[1];

    runtime.execute().unwrap();

    assert!(runtime
        .read_result_with(
            id1,
            |v| matches!(v.object(), KObject::Number(n) if *n == 1.0)
        )
        .expect("value"));
    assert!(runtime
        .read_result_with(
            id2,
            |v| matches!(v.object(), KObject::Number(n) if *n == 2.0)
        )
        .expect("value"));
    let data = root.bindings().data();
    assert!(data.contains_key("x"));
    assert!(data.contains_key("y"));
}

#[test]
fn later_expression_sees_earlier_binding_via_lookup() {
    // The second top-level expression spawns a sub-Dispatch for `(x)`; the earlier
    // LET runs first because its NodeId is smaller. Guards in-order processing.
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let root = test_run.scope;
    let runtime = &mut test_run.runtime;

    let brand = program.brand().region();
    let lookup_a = KExpression::new(
        brand,
        vec![
            Spanned::bare(ExpressionPart::Keyword("LET")),
            Spanned::bare(ExpressionPart::Identifier("b")),
            Spanned::bare(ExpressionPart::Keyword("=")),
            Spanned::bare(ExpressionPart::expression(
                program.brand(),
                vec![Spanned::bare(ExpressionPart::Identifier("a"))],
            )),
        ],
    );
    runtime.enter_block(
        root.id,
        vec![let_expr(&program, "a", 10.0), working(&program, lookup_a)],
        root,
    );

    runtime.execute().unwrap();
    assert!(matches!(root.lookup("b"), Some(KObject::Number(n)) if *n == 10.0));
}
