//! `register` arm of `machine::core` tests.

use super::super::{BindingIndex, Resolution, RuntimeArena};
use crate::builtins::test_support::run_root_bare;
use crate::machine::core::kfunction::{Body, KFunction, NodeId};
use crate::machine::model::types::{Argument, ExpressionSignature, KType, SignatureElement, ReturnType};
use crate::machine::model::values::KObject;

use super::{unit_signature, body_no_op};

// These tests don't care about lexical visibility — they pre-date the index-gated
// resolution work and exercise the lower-level [`Bindings`] write rules (rebind,
// dedupe, placeholder lifecycle). `BindingIndex::BUILTIN` keeps every entry at the
// "always visible" tag for visibility tests; that's correct here because the tests
// don't read through `Scope::resolve`'s chain-gated path.

#[test]
fn bind_value_errors_on_same_scope_rebind() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let v1 = arena.alloc(KObject::Number(1.0));
    let v2 = arena.alloc(KObject::Number(2.0));
    scope.bind_value("x".to_string(), v1, BindingIndex::BUILTIN).unwrap();
    let err = scope.bind_value("x".to_string(), v2, BindingIndex::BUILTIN).unwrap_err();
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
    outer.bind_value("x".to_string(), v1, BindingIndex::BUILTIN).unwrap();
    let inner = arena.alloc_scope(outer.child_for_call());
    let v2 = arena.alloc(KObject::Number(2.0));
    inner.bind_value("x".to_string(), v2, BindingIndex::BUILTIN).unwrap();
    assert!(matches!(inner.lookup("x"), Some(KObject::Number(n)) if *n == 2.0));
    assert!(matches!(outer.lookup("x"), Some(KObject::Number(n)) if *n == 1.0));
}

#[test]
fn register_function_dedupes_exact_signature() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let f1 = arena.alloc_function(KFunction::new(unit_signature(), Body::Builtin(body_no_op), scope));
    let obj1 = arena.alloc(KObject::KFunction(f1, None));
    scope.register_function("FOO".to_string(), f1, obj1, BindingIndex::BUILTIN).unwrap();
    let f2 = arena.alloc_function(KFunction::new(unit_signature(), Body::Builtin(body_no_op), scope));
    let obj2 = arena.alloc(KObject::KFunction(f2, None));
    let err = scope.register_function("FOO".to_string(), f2, obj2, BindingIndex::BUILTIN).unwrap_err();
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
    scope.register_function("FOO".to_string(), f1, obj1, BindingIndex::BUILTIN).unwrap();
    // Pointer-distinct, structurally identical signature — fresh arena allocation.
    let f2 = arena.alloc_function(KFunction::new(unit_signature(), Body::Builtin(body_no_op), scope));
    let obj2 = arena.alloc(KObject::KFunction(f2, None));
    let err = scope
        .bind_value("OTHER_NAME".to_string(), obj2, BindingIndex::BUILTIN)
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
    scope.bind_value("FIRST".to_string(), obj1, BindingIndex::BUILTIN).unwrap();
    // Re-binding under a *different* name with the same `&KFunction` pointer — the
    // intentional-alias case. Must succeed.
    scope.bind_value("ALIAS".to_string(), obj2, BindingIndex::BUILTIN).unwrap();
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
    scope.register_function("BAR".to_string(), f1, obj1, BindingIndex::BUILTIN).unwrap();
    scope.register_function("BAR".to_string(), f2, obj2, BindingIndex::BUILTIN).unwrap();
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
    scope.bind_value("FOO".to_string(), v, BindingIndex::BUILTIN).unwrap();
    let f = arena.alloc_function(KFunction::new(unit_signature(), Body::Builtin(body_no_op), scope));
    let obj = arena.alloc(KObject::KFunction(f, None));
    scope
        .register_function("FOO".to_string(), f, obj, BindingIndex::BUILTIN)
        .expect("bare FN registration must not collide with a same-name value");
    // The value binding survives untouched in `data`.
    assert!(matches!(scope.bindings().data().get("FOO").map(|(o, _)| *o), Some(KObject::Number(n)) if *n == 1.0));
    // The function landed in the dispatch bucket.
    let key = f.signature.untyped_key();
    assert!(scope.bindings().functions().get(&key).map(|b| !b.is_empty()).unwrap_or(false));
}

#[test]
fn resolve_returns_placeholder_when_only_placeholder_exists() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    scope.install_placeholder("x".to_string(), NodeId(7), BindingIndex::BUILTIN).unwrap();
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
    outer.bind_value("x".to_string(), v, BindingIndex::BUILTIN).unwrap();
    let inner = arena.alloc_scope(outer.child_for_call());
    inner.install_placeholder("x".to_string(), NodeId(3), BindingIndex::BUILTIN).unwrap();
    match inner.resolve("x") {
        Resolution::Placeholder(id) => assert_eq!(id, NodeId(3)),
        other => panic!(
            "expected Placeholder from inner — outer's Value should not shadow it. Got {}",
            match other {
                Resolution::Value(_) => "Value",
                Resolution::Placeholder(_) => "Placeholder",
                Resolution::UnboundName => "Unbound",
            }
        ),
    }
}

#[test]
fn bind_value_clears_own_placeholder() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    scope.install_placeholder("x".to_string(), NodeId(2), BindingIndex::BUILTIN).unwrap();
    let v = arena.alloc(KObject::Number(42.0));
    scope.bind_value("x".to_string(), v, BindingIndex::BUILTIN).unwrap();
    assert!(scope.bindings().placeholders().get("x").is_none());
    assert!(matches!(scope.resolve("x"), Resolution::Value(KObject::Number(n)) if *n == 42.0));
}

// Visibility-gate unit tests. Exercises the index-gated predicate
// [`crate::machine::core::scope::visible`] directly through
// `Scope::resolve_with_chain` / `Scope::resolve_type_with_chain`, decoupled from
// the scheduler so the gate's semantics are pinned at the [`Bindings`] surface.

#[test]
fn visibility_chain_none_sees_every_entry() {
    use std::rc::Rc;
    use crate::machine::core::LexicalFrame;
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    // Bind at a high index that would normally be invisible from a low-cutoff chain.
    let v = arena.alloc(KObject::Number(7.0));
    scope.bind_value("late".to_string(), v, BindingIndex::value(99)).unwrap();
    // A chain whose `index_for(scope.id) = None` (no frame on the chain mentions
    // this scope) treats the scope as "complete" — every entry is visible.
    let other_scope_id = crate::machine::core::ScopeId::next();
    let unrelated: Rc<LexicalFrame> = LexicalFrame::root(other_scope_id, 1);
    assert!(matches!(
        scope.resolve_with_chain("late", Some(&unrelated)),
        Resolution::Value(KObject::Number(n)) if *n == 7.0,
    ));
}

#[test]
fn visibility_strict_less_than_hides_later_sibling() {
    use std::rc::Rc;
    use crate::machine::core::LexicalFrame;
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let v = arena.alloc(KObject::Number(7.0));
    // Producer at index 5.
    scope.bind_value("later".to_string(), v, BindingIndex::value(5)).unwrap();
    // Consumer at index 3 → cutoff 3 → `5 < 3` is false → invisible.
    let consumer: Rc<LexicalFrame> = LexicalFrame::root(scope.id, 3);
    assert!(matches!(
        scope.resolve_with_chain("later", Some(&consumer)),
        Resolution::UnboundName,
    ));
}

#[test]
fn visibility_strict_less_than_admits_earlier_sibling() {
    use std::rc::Rc;
    use crate::machine::core::LexicalFrame;
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let v = arena.alloc(KObject::Number(7.0));
    scope.bind_value("earlier".to_string(), v, BindingIndex::value(2)).unwrap();
    let consumer: Rc<LexicalFrame> = LexicalFrame::root(scope.id, 5);
    assert!(matches!(
        scope.resolve_with_chain("earlier", Some(&consumer)),
        Resolution::Value(KObject::Number(n)) if *n == 7.0,
    ));
}

#[test]
fn visibility_nominal_binder_bypasses_cutoff() {
    use std::rc::Rc;
    use crate::machine::core::LexicalFrame;
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let v = arena.alloc(KObject::Number(7.0));
    // Producer tagged `nominal_binder: true` at index 99 — the D7 carve-out.
    scope.bind_value("nominal_late".to_string(), v, BindingIndex::nominal(99)).unwrap();
    // Consumer at index 1 sees the binding even though `99 < 1` is false.
    let consumer: Rc<LexicalFrame> = LexicalFrame::root(scope.id, 1);
    assert!(matches!(
        scope.resolve_with_chain("nominal_late", Some(&consumer)),
        Resolution::Value(KObject::Number(n)) if *n == 7.0,
    ));
}

#[test]
fn visibility_self_index_hidden_under_strict_less_than() {
    use std::rc::Rc;
    use crate::machine::core::LexicalFrame;
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let v = arena.alloc(KObject::Number(7.0));
    scope.bind_value("self_idx".to_string(), v, BindingIndex::value(3)).unwrap();
    // Same index as the binding (e.g. `LET x = x` shape): `3 < 3` is false.
    let consumer: Rc<LexicalFrame> = LexicalFrame::root(scope.id, 3);
    assert!(matches!(
        scope.resolve_with_chain("self_idx", Some(&consumer)),
        Resolution::UnboundName,
    ));
}

#[test]
fn visibility_placeholder_filtered_same_as_value() {
    use std::rc::Rc;
    use crate::machine::core::LexicalFrame;
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    scope
        .install_placeholder("ph".to_string(), NodeId(2), BindingIndex::value(5))
        .unwrap();
    let consumer: Rc<LexicalFrame> = LexicalFrame::root(scope.id, 3);
    // Placeholder at idx 5 is invisible to a consumer at cutoff 3.
    assert!(matches!(
        scope.resolve_with_chain("ph", Some(&consumer)),
        Resolution::UnboundName,
    ));
    // From the same chain but a higher cutoff, the placeholder is visible.
    let consumer_after: Rc<LexicalFrame> = LexicalFrame::root(scope.id, 9);
    assert!(matches!(
        scope.resolve_with_chain("ph", Some(&consumer_after)),
        Resolution::Placeholder(_),
    ));
}

#[test]
fn visibility_type_side_gate_mirrors_value_side() {
    use std::rc::Rc;
    use crate::machine::core::LexicalFrame;
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    scope.register_type("TyLate".to_string(), KType::Number, BindingIndex::value(5));
    // Cutoff below the type's idx → hidden.
    let consumer_before: Rc<LexicalFrame> = LexicalFrame::root(scope.id, 3);
    assert!(scope.resolve_type_with_chain("TyLate", Some(&consumer_before)).is_none());
    // Cutoff above → visible.
    let consumer_after: Rc<LexicalFrame> = LexicalFrame::root(scope.id, 9);
    assert!(scope.resolve_type_with_chain("TyLate", Some(&consumer_after)).is_some());
}
