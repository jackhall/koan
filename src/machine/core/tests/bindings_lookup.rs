//! Unit tests for [`crate::machine::core::Bindings::lookup_value`],
//! [`crate::machine::core::Bindings::lookup_type`], and
//! [`crate::machine::core::Bindings::lookup_function`] — the visibility-aware
//! lookups the index-gated resolver walks.
//!
//! Pins:
//! - nominal carve-out admits a binding tagged `nominal_binder: true` even
//!   when its index is at or past the consumer's cutoff;
//! - `chain_cutoff = None` disables the gate entirely (test fixtures /
//!   "scope is complete" arm);
//! - per-overload filter inside `lookup_function` returns only the visible
//!   overloads when a bucket carries siblings at different lexical indices.

use crate::builtins::test_support::run_root_bare;
use crate::machine::core::kfunction::{Body, KFunction, NodeId};
use crate::machine::core::{BindingIndex, FunctionLookup, Resolution, RuntimeArena};
use crate::machine::model::types::{
    Argument, ExpressionSignature, KType, ReturnType, SignatureElement,
};
use crate::machine::model::values::KObject;

use super::{body_no_op, unit_signature};

#[test]
fn lookup_value_chain_cutoff_none_admits_every_index() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let value = arena.alloc(KObject::Number(7.0));
    // Bind at a high index that a strict cutoff would hide.
    scope
        .bind_value("late".to_string(), value, BindingIndex::value(99))
        .unwrap();
    // `chain_cutoff = None` is the "scope is complete / unfiltered" arm — the
    // binding is visible regardless of its index.
    match scope.bindings().lookup_value("late", None) {
        Some(Resolution::Value(KObject::Number(n))) => assert_eq!(*n, 7.0),
        _ => panic!("expected Value(Number(7.0))"),
    }
}

#[test]
fn lookup_value_strict_less_than_hides_later_sibling() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let value = arena.alloc(KObject::Number(7.0));
    scope
        .bind_value("later".to_string(), value, BindingIndex::value(5))
        .unwrap();
    // Consumer at index 3 — `5 < 3` is false and the binder is not nominal,
    // so the lookup returns None.
    assert!(scope.bindings().lookup_value("later", Some(3)).is_none());
}

#[test]
fn lookup_value_strict_less_than_admits_earlier_sibling() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let value = arena.alloc(KObject::Number(7.0));
    scope
        .bind_value("earlier".to_string(), value, BindingIndex::value(2))
        .unwrap();
    match scope.bindings().lookup_value("earlier", Some(5)) {
        Some(Resolution::Value(KObject::Number(n))) => assert_eq!(*n, 7.0),
        _ => panic!("expected Value(Number(7.0))"),
    }
}

#[test]
fn lookup_value_nominal_binder_bypasses_cutoff() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let value = arena.alloc(KObject::Number(7.0));
    // Producer tagged `nominal_binder: true` at index 99 — the carve-out for
    // STRUCT / named UNION / SIG / FUNCTOR / MODULE.
    scope
        .bind_value("Nominal".to_string(), value, BindingIndex::nominal(99))
        .unwrap();
    match scope.bindings().lookup_value("Nominal", Some(1)) {
        Some(Resolution::Value(KObject::Number(n))) => assert_eq!(*n, 7.0),
        _ => panic!("nominal carve-out must admit the binding regardless of cutoff"),
    }
}

#[test]
fn lookup_value_placeholder_filtered_same_as_value() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    scope
        .install_placeholder("placeholder".to_string(), NodeId(2), BindingIndex::value(5))
        .unwrap();
    assert!(scope.bindings().lookup_value("placeholder", Some(3)).is_none());
    match scope.bindings().lookup_value("placeholder", Some(9)) {
        Some(Resolution::Placeholder(id)) => assert_eq!(id, NodeId(2)),
        _ => panic!("placeholder must be visible past its install index"),
    }
}

#[test]
fn lookup_type_chain_cutoff_none_admits_every_index() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    scope.register_type("Tee".into(), KType::Number, BindingIndex::value(99));
    assert!(matches!(
        scope.bindings().lookup_type("Tee", None),
        Some(KType::Number),
    ));
}

#[test]
fn lookup_type_strict_less_than_hides_later_sibling() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    scope.register_type("TyLate".into(), KType::Number, BindingIndex::value(5));
    assert!(scope.bindings().lookup_type("TyLate", Some(3)).is_none());
    assert!(scope.bindings().lookup_type("TyLate", Some(9)).is_some());
}

#[test]
fn lookup_type_nominal_binder_bypasses_cutoff() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    scope.register_type("Struct".into(), KType::Number, BindingIndex::nominal(99));
    assert!(scope.bindings().lookup_type("Struct", Some(1)).is_some());
}

#[test]
fn lookup_function_chain_cutoff_none_returns_full_bucket() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let f = arena
        .alloc_function(KFunction::new(unit_signature(), Body::Builtin(body_no_op), scope));
    let obj = arena.alloc(KObject::KFunction(f, None));
    scope
        .register_function("FOO".to_string(), f, obj, BindingIndex::value(99))
        .unwrap();
    let key = f.signature.untyped_key();
    match scope.bindings().lookup_function(&key, None) {
        FunctionLookup::Bucket(survivors) => {
            assert_eq!(survivors.len(), 1);
            assert!(std::ptr::eq(survivors[0], f));
        }
        _ => panic!("expected Bucket with one overload"),
    }
}

#[test]
fn lookup_function_filters_per_overload_visibility() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    // Two overloads sharing the same bucket key (one keyword), differing on a
    // value-side argument shape so they coexist in `functions[key]`.
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
    let key = sig_num.untyped_key();
    debug_assert_eq!(key, sig_str.untyped_key(), "untyped keys must collide");
    let f_early = arena.alloc_function(KFunction::new(sig_num, Body::Builtin(body_no_op), scope));
    let f_late = arena.alloc_function(KFunction::new(sig_str, Body::Builtin(body_no_op), scope));
    let obj_early = arena.alloc(KObject::KFunction(f_early, None));
    let obj_late = arena.alloc(KObject::KFunction(f_late, None));
    // The early overload installs at idx 2 (visible from cutoff 5); the late
    // overload installs at idx 7 (invisible from cutoff 5).
    scope
        .register_function("BAR".to_string(), f_early, obj_early, BindingIndex::value(2))
        .unwrap();
    scope
        .register_function("BAR".to_string(), f_late, obj_late, BindingIndex::value(7))
        .unwrap();
    match scope.bindings().lookup_function(&key, Some(5)) {
        FunctionLookup::Bucket(survivors) => {
            assert_eq!(survivors.len(), 1, "only the earlier-sibling overload is visible");
            assert!(std::ptr::eq(survivors[0], f_early));
        }
        _ => panic!("expected Bucket with one visible overload"),
    }
    // The same call with a higher cutoff sees both overloads.
    match scope.bindings().lookup_function(&key, Some(9)) {
        FunctionLookup::Bucket(survivors) => {
            assert_eq!(survivors.len(), 2);
        }
        _ => panic!("expected Bucket with both overloads"),
    }
}

#[test]
fn lookup_function_falls_through_to_pending_overload() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    // Synthesize a bucket key the call would compute; nothing in `functions`
    // for that key, but a pending-overload entry stands in for the in-flight
    // FN/FUNCTOR producer.
    let sig = unit_signature();
    let key = sig.untyped_key();
    scope
        .install_pending_overload(key.clone(), NodeId(11), BindingIndex::value(2))
        .unwrap();
    match scope.bindings().lookup_function(&key, Some(5)) {
        FunctionLookup::Pending(producer) => assert_eq!(producer, NodeId(11)),
        _ => panic!("expected Pending(NodeId(11))"),
    }
    // A cutoff below the pending entry's index hides it.
    assert!(matches!(
        scope.bindings().lookup_function(&key, Some(1)),
        FunctionLookup::None,
    ));
}

#[test]
fn lookup_function_bucket_shadows_pending_overload() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let f = arena
        .alloc_function(KFunction::new(unit_signature(), Body::Builtin(body_no_op), scope));
    let obj = arena.alloc(KObject::KFunction(f, None));
    scope
        .register_function("FOO".to_string(), f, obj, BindingIndex::value(2))
        .unwrap();
    let key = f.signature.untyped_key();
    // Pending install after the bucket is already populated is a silent no-op
    // (see `try_install_pending_overload`'s contract); `lookup_function` still
    // returns the live bucket.
    scope
        .install_pending_overload(key.clone(), NodeId(99), BindingIndex::value(3))
        .unwrap();
    match scope.bindings().lookup_function(&key, Some(9)) {
        FunctionLookup::Bucket(survivors) => assert_eq!(survivors.len(), 1),
        _ => panic!("live bucket must shadow a pending entry"),
    }
}

#[test]
fn lookup_function_empty_bucket_under_full_filter_returns_none_not_bucket() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let f = arena
        .alloc_function(KFunction::new(unit_signature(), Body::Builtin(body_no_op), scope));
    let obj = arena.alloc(KObject::KFunction(f, None));
    scope
        .register_function("FOO".to_string(), f, obj, BindingIndex::value(9))
        .unwrap();
    let key = f.signature.untyped_key();
    // Cutoff 3 hides the only overload — the empty-after-filter case must
    // surface as `None`, not as `Bucket(vec![])`. This is the contract the
    // dispatch walker relies on to keep walking ancestors.
    assert!(matches!(
        scope.bindings().lookup_function(&key, Some(3)),
        FunctionLookup::None,
    ));
}
