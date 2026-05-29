use crate::builtins::default_scope;
use crate::machine::NameOutcome;
use crate::machine::core::source::Spanned;
use crate::machine::execute::Scheduler;
use crate::machine::execute::dispatch::resolve_name_part;
use crate::machine::model::ast::{ExpressionPart, KExpression, TypeExpr};
use crate::machine::model::{KObject, KType};
use crate::machine::{BindingIndex, RuntimeArena};

#[test]
fn resolve_name_part_identifier_resolved() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let bound = arena.alloc(KObject::Number(7.0));
    scope.bind_value("x".to_string(), bound, BindingIndex::BUILTIN).unwrap();
    let part = ExpressionPart::Identifier("x".to_string());
    let sched = Scheduler::new();
    match resolve_name_part(scope, &part, &sched, None) {
        NameOutcome::Resolved(KObject::Number(n)) => assert_eq!(*n, 7.0),
        _ => panic!("expected NameOutcome::Resolved(Number)"),
    }
}

#[test]
fn resolve_name_part_type_resolved() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let part = ExpressionPart::Type(TypeExpr::leaf("Number".to_string()));
    let sched = Scheduler::new();
    match resolve_name_part(scope, &part, &sched, None) {
        NameOutcome::Resolved(KObject::KTypeValue(KType::Number)) => {}
        other => {
            let kind = match other {
                NameOutcome::Resolved(_) => "Resolved(other)",
                NameOutcome::Parked(_) => "Parked",
                NameOutcome::ProducerErrored(_) => "ProducerErrored",
                NameOutcome::Unbound(_) => "Unbound",
                NameOutcome::Cycle(_) => "Cycle",
            };
            panic!("expected Resolved(KTypeValue(Number)), got {kind}");
        }
    }
}

#[test]
fn resolve_name_part_parked() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    let producer = sched.add_dispatch(
        KExpression::new(vec![Spanned::bare(ExpressionPart::Identifier("_".into()))]),
        scope,
    );
    scope.install_placeholder("fwd".to_string(), producer, BindingIndex::BUILTIN).unwrap();
    let part = ExpressionPart::Identifier("fwd".to_string());
    match resolve_name_part(scope, &part, &sched, None) {
        NameOutcome::Parked(p) => assert_eq!(p, producer),
        _ => panic!("expected NameOutcome::Parked(producer)"),
    }
}

#[test]
fn resolve_name_part_unbound() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let part = ExpressionPart::Identifier("missing".to_string());
    let sched = Scheduler::new();
    match resolve_name_part(scope, &part, &sched, None) {
        NameOutcome::Unbound(name) => assert_eq!(name, "missing"),
        _ => panic!("expected NameOutcome::Unbound"),
    }
}

/// A `consumer` argument that matches its own producer returns `Cycle`, not `Parked`.
#[test]
fn resolve_name_part_self_park_is_cycle() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    let slot = sched.add_dispatch(
        KExpression::new(vec![Spanned::bare(ExpressionPart::Identifier(
            "self_ref".into(),
        ))]),
        scope,
    );
    scope.install_placeholder("self_ref".to_string(), slot, BindingIndex::BUILTIN).unwrap();
    let part = ExpressionPart::Identifier("self_ref".to_string());
    match resolve_name_part(scope, &part, &sched, Some(slot)) {
        NameOutcome::Cycle(name) => assert_eq!(name, "self_ref"),
        _ => panic!("expected NameOutcome::Cycle"),
    }
}
