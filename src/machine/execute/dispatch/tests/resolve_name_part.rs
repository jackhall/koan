use crate::builtins::default_scope;
use crate::machine::execute::dispatch::resolve_name_part;
use crate::machine::execute::KoanRuntime;
use crate::machine::model::ast::{ExpressionPart, KExpression, TypeIdentifier};
use crate::machine::model::{Carried, KObject, KType};
use crate::machine::core::FrameStorage;
use crate::machine::NameOutcome;
use crate::machine::BindingIndex;
use crate::source::Spanned;

#[test]
fn resolve_name_part_identifier_resolved() {
    let region = FrameStorage::run_root();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let bound = region.region().alloc_object(KObject::Number(7.0));
    scope
        .bind_value("x".to_string(), bound, BindingIndex::BUILTIN)
        .unwrap();
    let part = ExpressionPart::Identifier("x".to_string());
    let sched = KoanRuntime::new();
    match resolve_name_part(scope, &part, sched.scheduler(), None, None) {
        NameOutcome::Resolved(Carried::Object(KObject::Number(n))) => assert_eq!(*n, 7.0),
        _ => panic!("expected NameOutcome::Resolved(Number)"),
    }
}

#[test]
fn resolve_name_part_type_resolved() {
    let region = FrameStorage::run_root();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let part = ExpressionPart::Type(TypeIdentifier::leaf("Number".to_string()));
    let sched = KoanRuntime::new();
    match resolve_name_part(scope, &part, sched.scheduler(), None, None) {
        NameOutcome::Resolved(Carried::Type(KType::Number)) => {}
        other => {
            let kind = match other {
                NameOutcome::Resolved(_) => "Resolved(other)",
                NameOutcome::Parked(_) => "Parked",
                NameOutcome::ProducerErrored(_) => "ProducerErrored",
                NameOutcome::Unbound(_) => "Unbound",
                NameOutcome::Cycle(_) => "Cycle",
            };
            panic!("expected Resolved(Type(Number)), got {kind}");
        }
    }
}

#[test]
fn resolve_name_part_parked() {
    let region = FrameStorage::run_root();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let mut sched = KoanRuntime::new();
    let producer = sched.dispatch_in_scope(
        KExpression::new(vec![Spanned::bare(ExpressionPart::Identifier("_".into()))]),
        scope,
    );
    scope
        .install_placeholder("fwd".to_string(), producer, BindingIndex::BUILTIN)
        .unwrap();
    let part = ExpressionPart::Identifier("fwd".to_string());
    match resolve_name_part(scope, &part, sched.scheduler(), None, None) {
        NameOutcome::Parked(p) => assert_eq!(p, producer),
        _ => panic!("expected NameOutcome::Parked(producer)"),
    }
}

#[test]
fn resolve_name_part_unbound() {
    let region = FrameStorage::run_root();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let part = ExpressionPart::Identifier("missing".to_string());
    let sched = KoanRuntime::new();
    match resolve_name_part(scope, &part, sched.scheduler(), None, None) {
        NameOutcome::Unbound(name) => assert_eq!(name, "missing"),
        _ => panic!("expected NameOutcome::Unbound"),
    }
}

/// A `consumer` argument that matches its own producer returns `Cycle`, not `Parked`.
#[test]
fn resolve_name_part_self_park_is_cycle() {
    let region = FrameStorage::run_root();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let mut sched = KoanRuntime::new();
    let slot = sched.dispatch_in_scope(
        KExpression::new(vec![Spanned::bare(ExpressionPart::Identifier(
            "self_ref".into(),
        ))]),
        scope,
    );
    scope
        .install_placeholder("self_ref".to_string(), slot, BindingIndex::BUILTIN)
        .unwrap();
    let part = ExpressionPart::Identifier("self_ref".to_string());
    match resolve_name_part(scope, &part, sched.scheduler(), None, Some(slot)) {
        NameOutcome::Cycle(name) => assert_eq!(name, "self_ref"),
        _ => panic!("expected NameOutcome::Cycle"),
    }
}
