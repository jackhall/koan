use crate::builtins::test_support::TestRun;
use crate::machine::core::StoredReach;
use crate::machine::core::{run_root_storage, FrameStorageExt};
use crate::machine::execute::dispatch::{
    producer_disposition, resolve_name_part, ProducerDisposition,
};
use crate::machine::model::{Carried, KObject, KType};
use crate::machine::model::{ExpressionPart, KExpression, TypeIdentifier};
use crate::machine::BindingIndex;
use crate::machine::NameOutcome;
use crate::source::Spanned;

#[test]
fn resolve_name_part_identifier_resolved() {
    let region = run_root_storage();
    let test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let bound = region.brand().alloc_object(KObject::Number(7.0));
    scope
        .bind_value(
            "x".to_string(),
            bound,
            BindingIndex::BUILTIN,
            StoredReach::for_test(None, false),
        )
        .unwrap();
    let part = ExpressionPart::Identifier("x".to_string());
    match resolve_name_part(
        scope,
        &part,
        test_run.runtime.scheduler(),
        None,
        &test_run.types,
    ) {
        Ok(NameOutcome::Resolved(Carried::Object(KObject::Number(n)))) => assert_eq!(*n, 7.0),
        _ => panic!("expected NameOutcome::Resolved(Number)"),
    }
}

#[test]
fn resolve_name_part_type_resolved() {
    let region = run_root_storage();
    let test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let part = ExpressionPart::Type(TypeIdentifier::leaf("Number".to_string()));
    match resolve_name_part(
        scope,
        &part,
        test_run.runtime.scheduler(),
        None,
        &test_run.types,
    ) {
        Ok(NameOutcome::Resolved(Carried::Type(KType::NUMBER))) => {}
        other => {
            let kind = match other {
                Ok(NameOutcome::Resolved(_)) => "Resolved(other)",
                Ok(NameOutcome::Parked(_)) => "Parked",
                Ok(NameOutcome::Unbound(_)) => "Unbound",
                Err(_) => "Err",
            };
            panic!("expected Resolved(Type(Number)), got {kind}");
        }
    }
}

#[test]
fn resolve_name_part_parked() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let producer = test_run.runtime.dispatch_in_scope(
        KExpression::new(vec![Spanned::bare(ExpressionPart::Identifier("_".into()))]),
        scope,
    );
    scope
        .install_placeholder(
            "fwd".to_string(),
            producer,
            BindingIndex::BUILTIN,
            crate::machine::BindKind::Value,
        )
        .unwrap();
    let part = ExpressionPart::Identifier("fwd".to_string());
    match resolve_name_part(
        scope,
        &part,
        test_run.runtime.scheduler(),
        None,
        &test_run.types,
    ) {
        Ok(NameOutcome::Parked(p)) => assert_eq!(p, producer),
        _ => panic!("expected NameOutcome::Parked(producer)"),
    }
}

#[test]
fn resolve_name_part_unbound() {
    let region = run_root_storage();
    let test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let part = ExpressionPart::Identifier("missing".to_string());
    match resolve_name_part(
        scope,
        &part,
        test_run.runtime.scheduler(),
        None,
        &test_run.types,
    ) {
        Ok(NameOutcome::Unbound(name)) => assert_eq!(name, "missing"),
        _ => panic!("expected NameOutcome::Unbound"),
    }
}

/// The consumer-ful dependence check returns `Cycle` when a slot would park on itself — the
/// cycle arm `resolve_name_part` no longer carries (it screens consumer-less) lives here.
#[test]
fn producer_disposition_self_park_is_cycle() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let slot = test_run.runtime.dispatch_in_scope(
        KExpression::new(vec![Spanned::bare(ExpressionPart::Identifier(
            "self_ref".into(),
        ))]),
        scope,
    );
    match producer_disposition(test_run.runtime.scheduler(), slot, slot) {
        ProducerDisposition::Cycle => {}
        _ => panic!("expected ProducerDisposition::Cycle"),
    }
}
