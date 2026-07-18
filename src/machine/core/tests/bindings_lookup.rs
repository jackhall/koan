//! Unit tests for [`crate::machine::core::Bindings::lookup_value`],
//! [`crate::machine::core::Bindings::lookup_type`], and
//! [`crate::machine::core::Bindings::lookup_function`] — the visibility-aware
//! lookups the index-gated resolver walks.

use crate::builtins::test_support::run_root_bare;
use crate::machine::core::kfunction::{Body, KFunction, NodeId};
use crate::machine::core::StoredReach;
use crate::machine::core::{run_root_storage, BindingIndex, FrameStorageExt, NameLookup};
use crate::machine::model::KObject;
use crate::machine::model::{Argument, ExpressionSignature, KType, ReturnType, SignatureElement};

use super::{body_no_op, unit_signature};

#[test]
fn lookup_value_chain_cutoff_none_admits_every_index() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let value = region.brand().alloc_object(KObject::Number(7.0));
    scope
        .bind_value(
            "late".to_string(),
            value,
            BindingIndex::value(99),
            StoredReach::empty(),
        )
        .unwrap();
    match scope.bindings().lookup_value("late", None) {
        Some(NameLookup::Bound(KObject::Number(n))) => assert_eq!(*n, 7.0),
        _ => panic!("expected Value(Number(7.0))"),
    }
}

#[test]
fn lookup_value_strict_less_than_hides_later_sibling() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let value = region.brand().alloc_object(KObject::Number(7.0));
    scope
        .bind_value(
            "later".to_string(),
            value,
            BindingIndex::value(5),
            StoredReach::empty(),
        )
        .unwrap();
    assert!(scope.bindings().lookup_value("later", Some(3)).is_none());
}

#[test]
fn lookup_value_strict_less_than_admits_earlier_sibling() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let value = region.brand().alloc_object(KObject::Number(7.0));
    scope
        .bind_value(
            "earlier".to_string(),
            value,
            BindingIndex::value(2),
            StoredReach::empty(),
        )
        .unwrap();
    match scope.bindings().lookup_value("earlier", Some(5)) {
        Some(NameLookup::Bound(KObject::Number(n))) => assert_eq!(*n, 7.0),
        _ => panic!("expected Value(Number(7.0))"),
    }
}

#[test]
fn lookup_value_placeholder_filtered_same_as_value() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    scope
        .install_placeholder(
            "placeholder".to_string(),
            NodeId(2),
            BindingIndex::value(5),
            crate::machine::BindKind::Value,
        )
        .unwrap();
    assert!(scope
        .bindings()
        .lookup_value("placeholder", Some(3))
        .is_none());
    match scope.bindings().lookup_value("placeholder", Some(9)) {
        Some(NameLookup::Parked(id)) => assert_eq!(id, NodeId(2)),
        _ => panic!("placeholder must be visible past its install index"),
    }
}

#[test]
fn lookup_type_chain_cutoff_none_admits_every_index() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    scope.register_type("Tee".into(), KType::Number, BindingIndex::value(99));
    assert!(matches!(
        scope.bindings().lookup_type("Tee", None),
        Some(NameLookup::Bound(KType::Number)),
    ));
}

#[test]
fn lookup_type_strict_less_than_hides_later_sibling() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    scope.register_type("TyLate".into(), KType::Number, BindingIndex::value(5));
    assert!(scope.bindings().lookup_type("TyLate", Some(3)).is_none());
    assert!(scope.bindings().lookup_type("TyLate", Some(9)).is_some());
}

#[test]
fn lookup_function_chain_cutoff_none_returns_full_bucket() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let f = region.brand().alloc_function(KFunction::new(
        unit_signature(),
        Body::Builtin(body_no_op),
        scope,
        None,
        None,
    ));
    let obj = region
        .brand()
        .alloc_object_checked(KObject::KFunction(f))
        .expect("f was just allocated into region\'s own region");
    scope
        .register_function("FOO".to_string(), f, obj, BindingIndex::value(99))
        .unwrap();
    let key = f.signature.untyped_key();
    let lookup = scope.bindings().lookup_function(&key, None);
    assert_eq!(lookup.overloads.len(), 1);
    assert!(std::ptr::eq(lookup.overloads[0], f));
    assert!(lookup.pending.is_none());
}

#[test]
fn lookup_function_filters_per_overload_visibility() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    // Two overloads sharing the same bucket key but differing on a value-side
    // argument shape so they coexist in `functions[key]`.
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
    let f_early = region.brand().alloc_function(KFunction::new(
        sig_num,
        Body::Builtin(body_no_op),
        scope,
        None,
        None,
    ));
    let f_late = region.brand().alloc_function(KFunction::new(
        sig_str,
        Body::Builtin(body_no_op),
        scope,
        None,
        None,
    ));
    let obj_early = region
        .brand()
        .alloc_object_checked(KObject::KFunction(f_early))
        .expect("f was just allocated into region\'s own region");
    let obj_late = region
        .brand()
        .alloc_object_checked(KObject::KFunction(f_late))
        .expect("f was just allocated into region\'s own region");
    scope
        .register_function(
            "BAR".to_string(),
            f_early,
            obj_early,
            BindingIndex::value(2),
        )
        .unwrap();
    scope
        .register_function("BAR".to_string(), f_late, obj_late, BindingIndex::value(7))
        .unwrap();
    let visible_early = scope.bindings().lookup_function(&key, Some(5));
    assert_eq!(
        visible_early.overloads.len(),
        1,
        "only the earlier-sibling overload is visible"
    );
    assert!(std::ptr::eq(visible_early.overloads[0], f_early));
    let visible_both = scope.bindings().lookup_function(&key, Some(9));
    assert_eq!(visible_both.overloads.len(), 2);
}

#[test]
fn lookup_function_surfaces_pending_overload_when_bucket_empty() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    // No bucket for this key, but a pending-overload entry stands in for an
    // in-flight FN producer.
    let sig = unit_signature();
    let key = sig.untyped_key();
    scope
        .install_pending_overload(key.clone(), NodeId(11), BindingIndex::value(2))
        .unwrap();
    let visible = scope.bindings().lookup_function(&key, Some(5));
    assert!(visible.overloads.is_empty());
    assert_eq!(visible.pending, Some(NodeId(11)));
    // Filtered out: no overloads and no visible pending — the old `None`.
    let hidden = scope.bindings().lookup_function(&key, Some(1));
    assert!(hidden.overloads.is_empty());
    assert!(hidden.pending.is_none());
}

#[test]
fn lookup_function_surfaces_pending_overload_alongside_bucket() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let f = region.brand().alloc_function(KFunction::new(
        unit_signature(),
        Body::Builtin(body_no_op),
        scope,
        None,
        None,
    ));
    let obj = region
        .brand()
        .alloc_object_checked(KObject::KFunction(f))
        .expect("f was just allocated into region\'s own region");
    scope
        .register_function("FOO".to_string(), f, obj, BindingIndex::value(2))
        .unwrap();
    let key = f.signature.untyped_key();
    // A pending sibling is recorded alongside a finalized overload (no longer a
    // no-op): the scope walk parks the bucket until the sibling finalizes.
    scope
        .install_pending_overload(key.clone(), NodeId(99), BindingIndex::value(3))
        .unwrap();
    let lookup = scope.bindings().lookup_function(&key, Some(9));
    assert_eq!(lookup.overloads.len(), 1);
    assert_eq!(lookup.pending, Some(NodeId(99)));
}

#[test]
fn lookup_function_empty_bucket_under_full_filter_surfaces_no_overloads() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let f = region.brand().alloc_function(KFunction::new(
        unit_signature(),
        Body::Builtin(body_no_op),
        scope,
        None,
        None,
    ));
    let obj = region
        .brand()
        .alloc_object_checked(KObject::KFunction(f))
        .expect("f was just allocated into region\'s own region");
    scope
        .register_function("FOO".to_string(), f, obj, BindingIndex::value(9))
        .unwrap();
    let key = f.signature.untyped_key();
    // Empty-after-filter must surface an empty `overloads` with no pending, so
    // the dispatch walker keeps walking ancestors.
    let lookup = scope.bindings().lookup_function(&key, Some(3));
    assert!(lookup.overloads.is_empty());
    assert!(lookup.pending.is_none());
}
