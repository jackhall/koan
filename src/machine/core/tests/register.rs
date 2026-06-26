//! `register` arm of `machine::core` tests.

use super::super::{BindingIndex, Resolution};
use crate::builtins::test_support::run_root_bare;
use crate::machine::core::kfunction::{Body, KFunction, NodeId};
use crate::machine::core::FrameStorage;
use crate::machine::model::types::{
    Argument, ExpressionSignature, KType, ReturnType, SignatureElement,
};
use crate::machine::model::values::KObject;

use super::{body_no_op, unit_signature};

// `BindingIndex::BUILTIN` is used throughout because these tests exercise the
// `Bindings` write rules (rebind, dedupe, placeholder lifecycle) rather than the
// chain-gated `Scope::resolve` path.

#[test]
fn bind_value_errors_on_same_scope_rebind() {
    let region = FrameStorage::run_root();
    let scope = run_root_bare(&region);
    let v1 = region.region().alloc_object(KObject::Number(1.0));
    let v2 = region.region().alloc_object(KObject::Number(2.0));
    scope
        .bind_value("x".to_string(), v1, BindingIndex::BUILTIN)
        .unwrap();
    let err = scope
        .bind_value("x".to_string(), v2, BindingIndex::BUILTIN)
        .unwrap_err();
    match &err.kind {
        crate::machine::core::KErrorKind::Rebind { name } => assert_eq!(name, "x"),
        _ => panic!("expected Rebind, got {err}"),
    }
}

#[test]
fn bind_value_allows_shadowing_in_child_scope() {
    let region = FrameStorage::run_root();
    let outer = run_root_bare(&region);
    let v1 = region.region().alloc_object(KObject::Number(1.0));
    outer
        .bind_value("x".to_string(), v1, BindingIndex::BUILTIN)
        .unwrap();
    let inner = region.region().alloc_scope(outer.child_for_call());
    let v2 = region.region().alloc_object(KObject::Number(2.0));
    inner
        .bind_value("x".to_string(), v2, BindingIndex::BUILTIN)
        .unwrap();
    assert!(matches!(inner.lookup("x"), Some(KObject::Number(n)) if *n == 2.0));
    assert!(matches!(outer.lookup("x"), Some(KObject::Number(n)) if *n == 1.0));
}

#[test]
fn register_function_dedupes_exact_signature() {
    let region = FrameStorage::run_root();
    let scope = run_root_bare(&region);
    let f1 = region.region().alloc_function(KFunction::new(
        unit_signature(),
        Body::Builtin(body_no_op),
        scope,
    ));
    let obj1 = region.region().alloc_object(KObject::KFunction(f1));
    scope
        .register_function("FOO".to_string(), f1, obj1, BindingIndex::BUILTIN)
        .unwrap();
    let f2 = region.region().alloc_function(KFunction::new(
        unit_signature(),
        Body::Builtin(body_no_op),
        scope,
    ));
    let obj2 = region.region().alloc_object(KObject::KFunction(f2));
    let err = scope
        .register_function("FOO".to_string(), f2, obj2, BindingIndex::BUILTIN)
        .unwrap_err();
    assert!(
        matches!(&err.kind, crate::machine::core::KErrorKind::DuplicateOverload { name, .. } if name == "FOO"),
        "expected DuplicateOverload, got {err}",
    );
}

/// Routing a structurally identical but pointer-distinct `KFunction` through the LET
/// path (`bind_value(KObject::KFunction(...))`) must also trip `DuplicateOverload` —
/// the unified `try_apply` shares the FN dedupe rule.
#[test]
fn bind_value_with_kfunction_dedupes_exact_signature_with_existing_fn() {
    let region = FrameStorage::run_root();
    let scope = run_root_bare(&region);
    let f1 = region.region().alloc_function(KFunction::new(
        unit_signature(),
        Body::Builtin(body_no_op),
        scope,
    ));
    let obj1 = region.region().alloc_object(KObject::KFunction(f1));
    scope
        .register_function("FOO".to_string(), f1, obj1, BindingIndex::BUILTIN)
        .unwrap();
    let f2 = region.region().alloc_function(KFunction::new(
        unit_signature(),
        Body::Builtin(body_no_op),
        scope,
    ));
    let obj2 = region.region().alloc_object(KObject::KFunction(f2));
    let err = scope
        .bind_value("OTHER_NAME".to_string(), obj2, BindingIndex::BUILTIN)
        .unwrap_err();
    assert!(
        matches!(&err.kind, crate::machine::core::KErrorKind::DuplicateOverload { name, .. } if name == "OTHER_NAME"),
        "expected DuplicateOverload from LET path, got {err}",
    );
}

/// Intentional aliasing: `LET g = (f)` binding the same `&KFunction` under a second
/// name must succeed — bucket dedupe is silent-success on pointer-equal entries and
/// structural-rejection only on pointer-distinct ones.
#[test]
fn bind_value_with_kfunction_pointer_equal_alias_no_op() {
    let region = FrameStorage::run_root();
    let scope = run_root_bare(&region);
    let f = region.region().alloc_function(KFunction::new(
        unit_signature(),
        Body::Builtin(body_no_op),
        scope,
    ));
    let obj1 = region.region().alloc_object(KObject::KFunction(f));
    let obj2 = region.region().alloc_object(KObject::KFunction(f));
    scope
        .bind_value("FIRST".to_string(), obj1, BindingIndex::BUILTIN)
        .unwrap();
    scope
        .bind_value("ALIAS".to_string(), obj2, BindingIndex::BUILTIN)
        .unwrap();
}

#[test]
fn register_function_allows_overload_with_different_arg_types() {
    let region = FrameStorage::run_root();
    let scope = run_root_bare(&region);
    let sig_num = ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![
            SignatureElement::Keyword("BAR".into()),
            SignatureElement::Argument(Argument {
                name: "v".into(),
                ktype: KType::Number,
            }),
        ],
    };
    let sig_str = ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![
            SignatureElement::Keyword("BAR".into()),
            SignatureElement::Argument(Argument {
                name: "v".into(),
                ktype: KType::Str,
            }),
        ],
    };
    let f1 =
        region
            .region()
            .alloc_function(KFunction::new(sig_num, Body::Builtin(body_no_op), scope));
    let f2 =
        region
            .region()
            .alloc_function(KFunction::new(sig_str, Body::Builtin(body_no_op), scope));
    let obj1 = region.region().alloc_object(KObject::KFunction(f1));
    let obj2 = region.region().alloc_object(KObject::KFunction(f2));
    scope
        .register_function("BAR".to_string(), f1, obj1, BindingIndex::BUILTIN)
        .unwrap();
    scope
        .register_function("BAR".to_string(), f2, obj2, BindingIndex::BUILTIN)
        .unwrap();
}

/// `register_function` touches only `functions`, never `data`, so a bare FN may
/// coexist with a same-name value binding. The two namespaces stay independent.
#[test]
fn register_function_coexists_with_same_name_value() {
    let region = FrameStorage::run_root();
    let scope = run_root_bare(&region);
    let v = region.region().alloc_object(KObject::Number(1.0));
    scope
        .bind_value("FOO".to_string(), v, BindingIndex::BUILTIN)
        .unwrap();
    let f = region.region().alloc_function(KFunction::new(
        unit_signature(),
        Body::Builtin(body_no_op),
        scope,
    ));
    let obj = region.region().alloc_object(KObject::KFunction(f));
    scope
        .register_function("FOO".to_string(), f, obj, BindingIndex::BUILTIN)
        .expect("bare FN registration must not collide with a same-name value");
    assert!(
        matches!(scope.bindings().data().get("FOO").map(|(o, _)| *o), Some(KObject::Number(n)) if *n == 1.0)
    );
    let key = f.signature.untyped_key();
    assert!(scope
        .bindings()
        .functions()
        .get(&key)
        .map(|b| !b.is_empty())
        .unwrap_or(false));
}

#[test]
fn resolve_returns_placeholder_when_only_placeholder_exists() {
    let region = FrameStorage::run_root();
    let scope = run_root_bare(&region);
    scope
        .install_placeholder("x".to_string(), NodeId(7), BindingIndex::BUILTIN)
        .unwrap();
    match scope.resolve("x") {
        Resolution::Placeholder(id) => assert_eq!(id, NodeId(7)),
        _ => panic!("expected Placeholder"),
    }
}

#[test]
fn resolve_stops_at_first_hit_does_not_descend_outer() {
    let region = FrameStorage::run_root();
    let outer = run_root_bare(&region);
    let v = region.region().alloc_object(KObject::Number(1.0));
    outer
        .bind_value("x".to_string(), v, BindingIndex::BUILTIN)
        .unwrap();
    let inner = region.region().alloc_scope(outer.child_for_call());
    inner
        .install_placeholder("x".to_string(), NodeId(3), BindingIndex::BUILTIN)
        .unwrap();
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
    let region = FrameStorage::run_root();
    let scope = run_root_bare(&region);
    scope
        .install_placeholder("x".to_string(), NodeId(2), BindingIndex::BUILTIN)
        .unwrap();
    let v = region.region().alloc_object(KObject::Number(42.0));
    scope
        .bind_value("x".to_string(), v, BindingIndex::BUILTIN)
        .unwrap();
    assert!(scope.bindings().placeholders().get("x").is_none());
    assert!(matches!(scope.resolve("x"), Resolution::Value(KObject::Number(n)) if *n == 42.0));
}

// Visibility-gate unit tests: exercise `Scope::resolve_with_chain` /
// `Scope::resolve_type_with_chain` directly so the index-gated predicate's semantics
// are pinned independent of the scheduler.

#[test]
fn visibility_chain_none_sees_every_entry() {
    use crate::machine::core::LexicalFrame;
    use std::rc::Rc;
    let region = FrameStorage::run_root();
    let scope = run_root_bare(&region);
    let v = region.region().alloc_object(KObject::Number(7.0));
    scope
        .bind_value("late".to_string(), v, BindingIndex::value(99))
        .unwrap();
    // A chain whose `index_for(scope.id) = None` treats the scope as complete:
    // every entry is visible regardless of index.
    let other_scope_id = crate::machine::core::ScopeId::next();
    let unrelated: Rc<LexicalFrame> = LexicalFrame::root(other_scope_id, 1);
    assert!(matches!(
        scope.resolve_with_chain("late", Some(&unrelated)),
        Resolution::Value(KObject::Number(n)) if *n == 7.0,
    ));
}

#[test]
fn visibility_strict_less_than_hides_later_sibling() {
    use crate::machine::core::LexicalFrame;
    use std::rc::Rc;
    let region = FrameStorage::run_root();
    let scope = run_root_bare(&region);
    let v = region.region().alloc_object(KObject::Number(7.0));
    scope
        .bind_value("later".to_string(), v, BindingIndex::value(5))
        .unwrap();
    // Cutoff 3, producer at 5 → `5 < 3` is false → invisible.
    let consumer: Rc<LexicalFrame> = LexicalFrame::root(scope.id, 3);
    assert!(matches!(
        scope.resolve_with_chain("later", Some(&consumer)),
        Resolution::UnboundName,
    ));
}

#[test]
fn visibility_strict_less_than_admits_earlier_sibling() {
    use crate::machine::core::LexicalFrame;
    use std::rc::Rc;
    let region = FrameStorage::run_root();
    let scope = run_root_bare(&region);
    let v = region.region().alloc_object(KObject::Number(7.0));
    scope
        .bind_value("earlier".to_string(), v, BindingIndex::value(2))
        .unwrap();
    let consumer: Rc<LexicalFrame> = LexicalFrame::root(scope.id, 5);
    assert!(matches!(
        scope.resolve_with_chain("earlier", Some(&consumer)),
        Resolution::Value(KObject::Number(n)) if *n == 7.0,
    ));
}

#[test]
fn visibility_self_index_hidden_under_strict_less_than() {
    use crate::machine::core::LexicalFrame;
    use std::rc::Rc;
    let region = FrameStorage::run_root();
    let scope = run_root_bare(&region);
    let v = region.region().alloc_object(KObject::Number(7.0));
    scope
        .bind_value("self_idx".to_string(), v, BindingIndex::value(3))
        .unwrap();
    // Cutoff equal to producer idx (e.g. `LET x = x`): `3 < 3` is false.
    let consumer: Rc<LexicalFrame> = LexicalFrame::root(scope.id, 3);
    assert!(matches!(
        scope.resolve_with_chain("self_idx", Some(&consumer)),
        Resolution::UnboundName,
    ));
}

#[test]
fn visibility_placeholder_filtered_same_as_value() {
    use crate::machine::core::LexicalFrame;
    use std::rc::Rc;
    let region = FrameStorage::run_root();
    let scope = run_root_bare(&region);
    scope
        .install_placeholder("ph".to_string(), NodeId(2), BindingIndex::value(5))
        .unwrap();
    let consumer: Rc<LexicalFrame> = LexicalFrame::root(scope.id, 3);
    assert!(matches!(
        scope.resolve_with_chain("ph", Some(&consumer)),
        Resolution::UnboundName,
    ));
    let consumer_after: Rc<LexicalFrame> = LexicalFrame::root(scope.id, 9);
    assert!(matches!(
        scope.resolve_with_chain("ph", Some(&consumer_after)),
        Resolution::Placeholder(_),
    ));
}

#[test]
fn visibility_type_side_gate_mirrors_value_side() {
    use crate::machine::core::LexicalFrame;
    use std::rc::Rc;
    let region = FrameStorage::run_root();
    let scope = run_root_bare(&region);
    scope.register_type("TyLate".to_string(), KType::Number, BindingIndex::value(5));
    let consumer_before: Rc<LexicalFrame> = LexicalFrame::root(scope.id, 3);
    assert!(scope
        .resolve_type_with_chain("TyLate", Some(&consumer_before))
        .is_none());
    let consumer_after: Rc<LexicalFrame> = LexicalFrame::root(scope.id, 9);
    assert!(scope
        .resolve_type_with_chain("TyLate", Some(&consumer_after))
        .is_some());
}
