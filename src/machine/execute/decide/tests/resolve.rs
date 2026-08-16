use crate::builtins::test_support::TestRun;
use crate::machine::BindingIndex;
use crate::machine::ProducerId;
use crate::machine::core::{FrameStorageExt, program_storage, run_root_storage};
use crate::machine::execute::Resolution;
use crate::machine::execute::decide::resolve::{TypeLeafChannels, resolve_name};
use crate::machine::model::Scalar;
use crate::machine::model::{Carried, KObject, KType};
use crate::machine::model::{ExpressionPart, TypeIdentifier, WorkingExpression, WorkingPart};
use crate::source::Spanned;

#[test]
fn resolve_name_identifier_resolved() {
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
    match resolve_name(
        scope,
        &part,
        None,
        &test_run.types,
        TypeLeafChannels::ValueChannelFirst,
    ) {
        Resolution::Resolved(delivered) => assert!(
            matches!(delivered.open_at().value(), Carried::Object(KObject::Number(n)) if *n == 7.0),
            "expected Resolution::Resolved(Number(7.0))",
        ),
        _ => panic!("expected Resolution::Resolved(Number)"),
    }
}

#[test]
fn resolve_name_type_resolved() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let part = ExpressionPart::Type(TypeIdentifier::leaf("Number"));
    match resolve_name(
        scope,
        &part,
        None,
        &test_run.types,
        TypeLeafChannels::ValueChannelFirst,
    ) {
        Resolution::Resolved(ref delivered)
            if matches!(delivered.open_at().value(), Carried::Type(KType::NUMBER)) => {}
        other => {
            let kind = match other {
                Resolution::Resolved(_) => "Resolved(other)",
                Resolution::Parked(_) => "Parked",
                Resolution::Unbound(_) => "Unbound",
            };
            panic!("expected Resolved(Type(Number)), got {kind}");
        }
    }
}

#[test]
fn resolve_name_parked() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let producer = test_run.dispatch_in_scope(
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
    match resolve_name(
        scope,
        &part,
        None,
        &test_run.types,
        TypeLeafChannels::ValueChannelFirst,
    ) {
        Resolution::Parked(p) => assert_eq!(p, claim),
        _ => panic!("expected Resolution::Parked(claim)"),
    }
}

#[test]
fn resolve_name_unbound() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let part = ExpressionPart::Identifier("missing");
    match resolve_name(
        scope,
        &part,
        None,
        &test_run.types,
        TypeLeafChannels::ValueChannelFirst,
    ) {
        Resolution::Unbound(name) => assert_eq!(name, "missing"),
        _ => panic!("expected Resolution::Unbound"),
    }
}
