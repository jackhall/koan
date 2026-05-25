//! `register` arm of `machine::core` tests.

use super::super::{Resolution, RuntimeArena};
use crate::builtins::test_support::run_root_bare;
use crate::machine::core::kfunction::{Body, KFunction, NodeId};
use crate::machine::model::types::{Argument, ExpressionSignature, KType, SignatureElement, ReturnType};
use crate::machine::model::values::KObject;

use super::{unit_signature, body_no_op};

#[test]
fn bind_value_errors_on_same_scope_rebind() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let v1 = arena.alloc(KObject::Number(1.0));
    let v2 = arena.alloc(KObject::Number(2.0));
    scope.bind_value("x".to_string(), v1).unwrap();
    let err = scope.bind_value("x".to_string(), v2).unwrap_err();
    match &err.kind {
        crate::machine::core::KErrorKind::Rebind { name } => assert_eq!(name, "x"),
        _ => panic!("expected Rebind, got {err}"),
    }
}

#[test]
fn bind_value_allows_shadowing_in_child_scope() {
    let arena = RuntimeArena::new();
    let outer = run_root_bare(&arena);
    let v1 = arena.alloc(KObject::Number(1.0));
    outer.bind_value("x".to_string(), v1).unwrap();
    let inner = arena.alloc_scope(outer.child_for_call());
    let v2 = arena.alloc(KObject::Number(2.0));
    inner.bind_value("x".to_string(), v2).unwrap();
    assert!(matches!(inner.lookup("x"), Some(KObject::Number(n)) if *n == 2.0));
    assert!(matches!(outer.lookup("x"), Some(KObject::Number(n)) if *n == 1.0));
}

#[test]
fn register_function_dedupes_exact_signature() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let f1 = arena.alloc_function(KFunction::new(unit_signature(), Body::Builtin(body_no_op), scope));
    let obj1 = arena.alloc(KObject::KFunction(f1, None));
    scope.register_function("FOO".to_string(), f1, obj1).unwrap();
    let f2 = arena.alloc_function(KFunction::new(unit_signature(), Body::Builtin(body_no_op), scope));
    let obj2 = arena.alloc(KObject::KFunction(f2, None));
    let err = scope.register_function("FOO".to_string(), f2, obj2).unwrap_err();
    assert!(
        matches!(&err.kind, crate::machine::core::KErrorKind::DuplicateOverload { name, .. } if name == "FOO"),
        "expected DuplicateOverload, got {err}",
    );
}

/// Companion to `register_function_dedupes_exact_signature`: routing a structurally
/// identical but pointer-distinct `KFunction` through the LET path
/// (`bind_value(KObject::KFunction(...))`) must also trip `DuplicateOverload`. Pre-
/// façade the LET path only dedup'd by `ptr::eq`, so a fresh-arena-allocated function
/// with matching signature silently doubled the bucket. The unified `try_apply` closes
/// this gap. Uses a different name from the prior FN so the test focuses on bucket
/// dedupe rather than the `Rebind`-on-existing-name path.
#[test]
fn bind_value_with_kfunction_dedupes_exact_signature_with_existing_fn() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let f1 = arena.alloc_function(KFunction::new(unit_signature(), Body::Builtin(body_no_op), scope));
    let obj1 = arena.alloc(KObject::KFunction(f1, None));
    scope.register_function("FOO".to_string(), f1, obj1).unwrap();
    // Pointer-distinct, structurally identical signature — fresh arena allocation.
    let f2 = arena.alloc_function(KFunction::new(unit_signature(), Body::Builtin(body_no_op), scope));
    let obj2 = arena.alloc(KObject::KFunction(f2, None));
    let err = scope
        .bind_value("OTHER_NAME".to_string(), obj2)
        .unwrap_err();
    assert!(
        matches!(&err.kind, crate::machine::core::KErrorKind::DuplicateOverload { name, .. } if name == "OTHER_NAME"),
        "expected DuplicateOverload from LET path, got {err}",
    );
}

/// The `ptr::eq` fast-path still allows intentional aliasing: `LET g = (f)` where the
/// same `&KFunction` is bound under a second name must succeed without
/// `DuplicateOverload`. This pins the rule that the bucket dedupe is silent-success on
/// pointer-equal entries and structural-rejection only on pointer-distinct ones.
#[test]
fn bind_value_with_kfunction_pointer_equal_alias_no_op() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let f = arena.alloc_function(KFunction::new(unit_signature(), Body::Builtin(body_no_op), scope));
    let obj1 = arena.alloc(KObject::KFunction(f, None));
    let obj2 = arena.alloc(KObject::KFunction(f, None));
    scope.bind_value("FIRST".to_string(), obj1).unwrap();
    // Re-binding under a *different* name with the same `&KFunction` pointer — the
    // intentional-alias case. Must succeed.
    scope.bind_value("ALIAS".to_string(), obj2).unwrap();
}

#[test]
fn register_function_allows_overload_with_different_arg_types() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let sig_num = ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![
            SignatureElement::Keyword("BAR".into()),
            SignatureElement::Argument(Argument { name: "v".into(), ktype: KType::Number }),
        ],
    };
    let sig_str = ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![
            SignatureElement::Keyword("BAR".into()),
            SignatureElement::Argument(Argument { name: "v".into(), ktype: KType::Str }),
        ],
    };
    let f1 = arena.alloc_function(KFunction::new(sig_num, Body::Builtin(body_no_op), scope));
    let f2 = arena.alloc_function(KFunction::new(sig_str, Body::Builtin(body_no_op), scope));
    let obj1 = arena.alloc(KObject::KFunction(f1, None));
    let obj2 = arena.alloc(KObject::KFunction(f2, None));
    scope.register_function("BAR".to_string(), f1, obj1).unwrap();
    scope.register_function("BAR".to_string(), f2, obj2).unwrap();
}

/// A bare `FN` keyword may coexist with a same-name value binding: `register_function`
/// touches only the `functions` bucket, never `data`, so it neither sees nor collides
/// with a value already in `data[name]`. The two namespaces stay independent — `resolve`
/// reads `data`, dispatch reads `functions`.
#[test]
fn register_function_coexists_with_same_name_value() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let v = arena.alloc(KObject::Number(1.0));
    scope.bind_value("FOO".to_string(), v).unwrap();
    let f = arena.alloc_function(KFunction::new(unit_signature(), Body::Builtin(body_no_op), scope));
    let obj = arena.alloc(KObject::KFunction(f, None));
    scope
        .register_function("FOO".to_string(), f, obj)
        .expect("bare FN registration must not collide with a same-name value");
    // The value binding survives untouched in `data`.
    assert!(matches!(scope.bindings().data().get("FOO").copied(), Some(KObject::Number(n)) if *n == 1.0));
    // The function landed in the dispatch bucket.
    let key = f.signature.untyped_key();
    assert!(scope.bindings().functions().get(&key).map(|b| !b.is_empty()).unwrap_or(false));
}

#[test]
fn resolve_returns_placeholder_when_only_placeholder_exists() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    scope.install_placeholder("x".to_string(), NodeId(7)).unwrap();
    match scope.resolve("x") {
        Resolution::Placeholder(id) => assert_eq!(id, NodeId(7)),
        _ => panic!("expected Placeholder"),
    }
}

#[test]
fn resolve_stops_at_first_hit_does_not_descend_outer() {
    let arena = RuntimeArena::new();
    let outer = run_root_bare(&arena);
    let v = arena.alloc(KObject::Number(1.0));
    outer.bind_value("x".to_string(), v).unwrap();
    let inner = arena.alloc_scope(outer.child_for_call());
    inner.install_placeholder("x".to_string(), NodeId(3)).unwrap();
    match inner.resolve("x") {
        Resolution::Placeholder(id) => assert_eq!(id, NodeId(3)),
        other => panic!(
            "expected Placeholder from inner — outer's Value should not shadow it. Got {}",
            match other {
                Resolution::Value(_) => "Value",
                Resolution::Placeholder(_) => "Placeholder",
                Resolution::Unbound => "Unbound",
            }
        ),
    }
}

#[test]
fn bind_value_clears_own_placeholder() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    scope.install_placeholder("x".to_string(), NodeId(2)).unwrap();
    let v = arena.alloc(KObject::Number(42.0));
    scope.bind_value("x".to_string(), v).unwrap();
    assert!(scope.bindings().placeholders().get("x").is_none());
    assert!(matches!(scope.resolve("x"), Resolution::Value(KObject::Number(n)) if *n == 42.0));
}
