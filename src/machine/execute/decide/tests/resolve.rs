use crate::builtins::test_support::type_token;
use crate::builtins::test_support::{TestRun, binder_name, identifier_part, value_name};
use crate::machine::BindingIndex;
use crate::machine::ProducerId;
use crate::machine::core::{FrameStorageExt, program_storage, run_root_storage};
use crate::machine::execute::Resolution;
use crate::machine::execute::decide::resolve::resolve_name;
use crate::machine::model::Scalar;
use crate::machine::model::{Carried, KObject, KType};
use crate::machine::model::{ExpressionPart, WorkingExpression, WorkingPart};
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
            value_name("x", test_run.registries()),
            bound,
            BindingIndex::BUILTIN,
            test_run.registries(),
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    let part = identifier_part("x");
    match resolve_name(scope, &part, None, test_run.registries()) {
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
    let part = ExpressionPart::Type(type_token("Number"));
    match resolve_name(scope, &part, None, test_run.registries()) {
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
            &[Spanned::bare(WorkingPart::Ast(identifier_part("producer")))],
        ),
        scope,
    );
    let claim =
        ProducerId::from_scheduler_edge(test_run.runtime.install_edge_for_test(producer, scope));
    scope
        .install_placeholder(
            binder_name("fwd", test_run.registries()),
            claim,
            BindingIndex::BUILTIN,
            test_run.registries(),
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    let part = identifier_part("fwd");
    match resolve_name(scope, &part, None, test_run.registries()) {
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
    let part = ExpressionPart::Identifier(value_name("missing", test_run.registries()));
    match resolve_name(scope, &part, None, test_run.registries()) {
        Resolution::Unbound(name) => assert_eq!(name, "missing"),
        _ => panic!("expected Resolution::Unbound"),
    }
}
