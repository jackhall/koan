//! Basic dispatch ordering and inter-expression lookup.

use crate::builtins::test_support::TestRun;
use crate::machine::core::{program_storage, run_root_storage};
use crate::machine::model::KObject;
use crate::machine::model::{ExpressionPart, KExpression};
use crate::source::Spanned;

use super::{let_expr, working};
use crate::builtins::test_support::{kw_part, value_name};

#[test]
fn dispatches_independent_expressions_in_order() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let root = test_run.scope;
    let registry = test_run.registry_handle();
    let labels = &registry.registries().labels;
    let runtime = &mut test_run.runtime;
    let ids = runtime.enter_block(
        root.id,
        vec![
            let_expr(&program, labels, "x", 1.0),
            let_expr(&program, labels, "y", 2.0),
        ],
        root,
    );
    let edge1 = runtime.install_edge_for_test(ids[0], root);
    let edge2 = runtime.install_edge_for_test(ids[1], root);

    runtime.execute().unwrap();

    assert!(
        runtime
            .read_edge_result_with(
                edge1,
                |v| matches!(v.object(), KObject::Number(n) if *n == 1.0)
            )
            .expect("value")
    );
    assert!(
        runtime
            .read_edge_result_with(
                edge2,
                |v| matches!(v.object(), KObject::Number(n) if *n == 2.0)
            )
            .expect("value")
    );
    let data = root.bindings().data();
    assert!(
        data.contains_key(&crate::builtins::test_support::value_name(
            "x",
            test_run.registries()
        ))
    );
    assert!(
        data.contains_key(&crate::builtins::test_support::value_name(
            "y",
            test_run.registries()
        ))
    );
}

#[test]
fn later_expression_sees_earlier_binding_via_lookup() {
    // The second top-level expression spawns a sub-Dispatch for `(x)`; the earlier
    // LET runs first because its NodeId is smaller. Guards in-order processing.
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let root = test_run.scope;
    let registry = test_run.registry_handle();
    let labels = &registry.registries().labels;
    let runtime = &mut test_run.runtime;

    let brand = program.brand().region();
    let lookup_a = KExpression::new(
        brand,
        &[
            Spanned::bare(kw_part("LET")),
            Spanned::bare(ExpressionPart::Identifier(value_name(
                "b",
                registry.registries(),
            ))),
            Spanned::bare(kw_part("=")),
            Spanned::bare(ExpressionPart::expression(
                program.brand(),
                &[Spanned::bare(ExpressionPart::Identifier(value_name(
                    "a",
                    registry.registries(),
                )))],
            )),
        ],
    );
    runtime.enter_block(
        root.id,
        vec![
            let_expr(&program, labels, "a", 10.0),
            working(&program, lookup_a),
        ],
        root,
    );

    runtime.execute().unwrap();
    assert!(matches!(root.lookup("b"), Some(KObject::Number(n)) if *n == 10.0));
}
