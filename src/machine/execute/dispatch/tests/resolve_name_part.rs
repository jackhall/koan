use crate::builtins::test_support::TestRun;
use crate::machine::BindingIndex;
use crate::machine::NameOutcome;
use crate::machine::core::ProducerId;
use crate::machine::core::{FrameStorageExt, program_storage, run_root_storage};
use crate::machine::execute::dispatch::resolve_name_part;
use crate::machine::model::Scalar;
use crate::machine::model::{Carried, KObject, KType};
use crate::machine::model::{ExpressionPart, TypeIdentifier, WorkingExpression, WorkingPart};
use crate::source::Spanned;

#[test]
fn resolve_name_part_identifier_resolved() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let bound = region.brand().alloc_scalar(Scalar::Number(7.0));
    scope
        .bind_resident_for_test(
            "x".to_string(),
            bound,
            BindingIndex::BUILTIN,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    let part = ExpressionPart::Identifier("x");
    match resolve_name_part(scope, &part, None, &test_run.types) {
        NameOutcome::Resolved(delivered) => assert!(
            matches!(delivered.open_at().value(), Carried::Object(KObject::Number(n)) if *n == 7.0),
            "expected NameOutcome::Resolved(Number(7.0))",
        ),
        _ => panic!("expected NameOutcome::Resolved(Number)"),
    }
}

#[test]
fn resolve_name_part_type_resolved() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let part = ExpressionPart::Type(TypeIdentifier::leaf("Number"));
    match resolve_name_part(scope, &part, None, &test_run.types) {
        NameOutcome::Resolved(ref delivered)
            if matches!(delivered.open_at().value(), Carried::Type(KType::NUMBER)) => {}
        other => {
            let kind = match other {
                NameOutcome::Resolved(_) => "Resolved(other)",
                NameOutcome::Parked(_) => "Parked",
                NameOutcome::Unbound(_) => "Unbound",
            };
            panic!("expected Resolved(Type(Number)), got {kind}");
        }
    }
}

#[test]
fn resolve_name_part_parked() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let producer = test_run.runtime.dispatch_in_scope(
        WorkingExpression::new(
            scope.brand(),
            vec![Spanned::bare(WorkingPart::Ast(ExpressionPart::Identifier(
                "_",
            )))],
        ),
        scope,
    );
    let claim =
        ProducerId::from_scheduler_edge(test_run.runtime.install_edge_for_test(producer, scope));
    scope
        .install_placeholder(
            "fwd".to_string(),
            claim,
            BindingIndex::BUILTIN,
            crate::machine::model::BindKind::Value,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    let part = ExpressionPart::Identifier("fwd");
    match resolve_name_part(scope, &part, None, &test_run.types) {
        NameOutcome::Parked(p) => assert_eq!(p, claim),
        _ => panic!("expected NameOutcome::Parked(claim)"),
    }
}

#[test]
fn resolve_name_part_unbound() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let part = ExpressionPart::Identifier("missing");
    match resolve_name_part(scope, &part, None, &test_run.types) {
        NameOutcome::Unbound(name) => assert_eq!(name, "missing"),
        _ => panic!("expected NameOutcome::Unbound"),
    }
}

/// The one pre-wiring question a decide still asks: parking a slot on its own claim edge would
/// close a wake cycle, so the walk drops that source rather than proposing the edge.
#[test]
fn self_park_source_would_create_cycle() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let slot = test_run.runtime.dispatch_in_scope(
        WorkingExpression::new(
            scope.brand(),
            vec![Spanned::bare(WorkingPart::Ast(ExpressionPart::Identifier(
                "self_ref",
            )))],
        ),
        scope,
    );
    let claim = test_run.runtime.install_edge_for_test(slot, scope);
    assert!(
        test_run
            .runtime
            .scheduler()
            .would_create_cycle_from(claim, slot)
    );
}
