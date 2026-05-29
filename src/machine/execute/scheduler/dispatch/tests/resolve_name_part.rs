use crate::builtins::default_scope;
use crate::machine::NameOutcome;
use crate::machine::core::source::Spanned;
use crate::machine::execute::Scheduler;
use crate::machine::execute::scheduler::dispatch::resolve_name_part;
use crate::machine::model::ast::{ExpressionPart, KExpression, TypeExpr};
use crate::machine::model::{KObject, KType};
use crate::machine::{BindingIndex, RuntimeArena};

/// Resolved-Identifier path: bare Identifier in scope.bindings.data returns
/// `NameOutcome::Resolved(&obj)` pointing at the bound carrier.
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

/// Resolved-Type path: bare leaf `Type` token whose name lives in
/// `bindings.types` routes through `coerce_type_token_value` and returns the
/// `KTypeValue` synthesis. The builtin `Number` registered at default_scope
/// satisfies this without extra setup.
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

/// Parked path: a Dispatch slot installed as a `binder_name` placeholder against the
/// name resolves to `NameOutcome::Parked(producer)`. Mimics a forward LET binder
/// by manually installing a placeholder against a fresh slot.
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

/// Unbound path: a name with no binding and no placeholder returns
/// `NameOutcome::Unbound(name)`.
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

/// Cycle path: when `consumer` is provided and matches the producer (self-park),
/// returns `NameOutcome::Cycle(name)` rather than `Parked`.
#[test]
fn resolve_name_part_self_park_is_cycle() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    let slot = sched.add(
        crate::machine::execute::nodes::NodeWork::dispatch(KExpression::new(vec![
            Spanned::bare(ExpressionPart::Identifier("self_ref".into())),
        ])),
        scope,
    );
    scope.install_placeholder("self_ref".to_string(), slot, BindingIndex::BUILTIN).unwrap();
    let part = ExpressionPart::Identifier("self_ref".to_string());
    match resolve_name_part(scope, &part, &sched, Some(slot)) {
        NameOutcome::Cycle(name) => assert_eq!(name, "self_ref"),
        _ => panic!("expected NameOutcome::Cycle"),
    }
}
