//! Unit tests for [`crate::machine::core::Bindings::lookup_value`],
//! [`crate::machine::core::Bindings::lookup_type`], and
//! [`crate::machine::core::Bindings::lookup_function`] — the visibility-aware
//! lookups the index-gated resolver walks.

use crate::builtins::test_support::{mock_declaration_site, run_root_bare};
use crate::machine::ProducerId;
use crate::machine::core::kfunction::{Body, KFunction};
use crate::machine::core::{BindingIndex, FrameStorageExt, NameLookup, run_root_storage};
use crate::machine::model::KObject;
use crate::machine::model::TypeRegistry;
use crate::machine::model::UntypedKeyProbe;
use crate::machine::model::{Argument, KType, ReturnType, SignatureDraft, SignatureElement};

use super::{body_no_op, unit_signature};
use crate::machine::model::Scalar;

#[test]
fn lookup_value_chain_cutoff_none_admits_every_index() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let value = region.brand().alloc_scalar(Scalar::Number(7.0));
    scope
        .bind_resident_for_test(
            "late".to_string(),
            value,
            BindingIndex::value(99),
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    match scope.lookup_value_here_for_test("late", None) {
        Some(NameLookup::Bound(KObject::Number(n))) => assert_eq!(*n, 7.0),
        _ => panic!("expected Value(Number(7.0))"),
    }
}

#[test]
fn lookup_value_strict_less_than_hides_later_sibling() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let value = region.brand().alloc_scalar(Scalar::Number(7.0));
    scope
        .bind_resident_for_test(
            "later".to_string(),
            value,
            BindingIndex::value(5),
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    assert!(scope.bindings().lookup_value("later", Some(3)).is_none());
}

#[test]
fn lookup_value_strict_less_than_admits_earlier_sibling() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let value = region.brand().alloc_scalar(Scalar::Number(7.0));
    scope
        .bind_resident_for_test(
            "earlier".to_string(),
            value,
            BindingIndex::value(2),
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    match scope.lookup_value_here_for_test("earlier", Some(5)) {
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
            ProducerId::for_test(2),
            BindingIndex::value(5),
            crate::machine::model::BindKind::Value,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    assert!(
        scope
            .bindings()
            .lookup_value("placeholder", Some(3))
            .is_none()
    );
    match scope.bindings().lookup_value("placeholder", Some(9)) {
        Some(NameLookup::Parked(id)) => assert_eq!(id, ProducerId::for_test(2)),
        _ => panic!("placeholder must be visible past its install index"),
    }
}

#[test]
fn lookup_type_chain_cutoff_none_admits_every_index() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let _ = scope.register_type_direct(
        "Tee".into(),
        KType::NUMBER,
        mock_declaration_site(99),
        &mut crate::machine::WriteGate::for_test(),
    );
    assert!(matches!(
        scope.bindings().lookup_type("Tee", None),
        Some(NameLookup::Bound(kt)) if kt == KType::NUMBER,
    ));
}

#[test]
fn lookup_type_strict_less_than_hides_later_sibling() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let _ = scope.register_type_direct(
        "TyLate".into(),
        KType::NUMBER,
        mock_declaration_site(5),
        &mut crate::machine::WriteGate::for_test(),
    );
    assert!(scope.bindings().lookup_type("TyLate", Some(3)).is_none());
    assert!(scope.bindings().lookup_type("TyLate", Some(9)).is_some());
}

#[test]
fn lookup_function_chain_cutoff_none_returns_full_bucket() {
    let types = TypeRegistry::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let cell = KFunction::alloc_captured(
        scope,
        unit_signature(),
        Body::Builtin(body_no_op),
        false,
        &types,
    );
    let f = cell.adopt_into(scope.brand().handle());
    scope
        .register_function_direct(
            "FOO".to_string(),
            &cell,
            BindingIndex::value(99),
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    let key = f.signature.untyped_key();
    let lookup = scope.bindings().lookup_function(&key, None);
    assert_eq!(lookup.overloads.len(), 1);
    assert!(std::ptr::eq(
        scope.open_function(&lookup.overloads[0]).value(),
        f
    ));
    assert!(lookup.pending.is_none());
}

#[test]
fn lookup_function_filters_per_overload_visibility() {
    let types = TypeRegistry::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    // Two overloads sharing the same bucket key but differing on a value-side
    // argument shape so they coexist in `functions[key]`.
    let sig_num = SignatureDraft {
        return_type: ReturnType::Resolved(KType::ANY),
        elements: vec![
            SignatureElement::Keyword("BAR"),
            SignatureElement::Argument(Argument {
                name: "v",
                ktype: KType::NUMBER,
            }),
        ],
    };
    let sig_str = SignatureDraft {
        return_type: ReturnType::Resolved(KType::ANY),
        elements: vec![
            SignatureElement::Keyword("BAR"),
            SignatureElement::Argument(Argument {
                name: "v",
                ktype: KType::STR,
            }),
        ],
    };
    let key = sig_num.untyped_key();
    debug_assert_eq!(key, sig_str.untyped_key(), "untyped keys must collide");
    let early = KFunction::alloc_captured(scope, sig_num, Body::Builtin(body_no_op), false, &types);
    let late = KFunction::alloc_captured(scope, sig_str, Body::Builtin(body_no_op), false, &types);
    let f_early = early.adopt_into(scope.brand().handle());
    scope
        .register_function_direct(
            "BAR".to_string(),
            &early,
            BindingIndex::value(2),
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    scope
        .register_function_direct(
            "BAR".to_string(),
            &late,
            BindingIndex::value(7),
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    let visible_early = scope.bindings().lookup_function(&key, Some(5));
    assert_eq!(
        visible_early.overloads.len(),
        1,
        "only the earlier-sibling overload is visible"
    );
    assert!(std::ptr::eq(
        scope.open_function(&visible_early.overloads[0]).value(),
        f_early
    ));
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
        .install_pending_overload(
            key.clone(),
            ProducerId::for_test(11),
            BindingIndex::value(2),
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    let visible = scope.bindings().lookup_function(&key, Some(5));
    assert!(visible.overloads.is_empty());
    assert_eq!(visible.pending, Some(ProducerId::for_test(11)));
    // Filtered out: no overloads and no visible pending — the old `None`.
    let hidden = scope.bindings().lookup_function(&key, Some(1));
    assert!(hidden.overloads.is_empty());
    assert!(hidden.pending.is_none());
}

#[test]
fn lookup_function_surfaces_pending_overload_alongside_bucket() {
    let types = TypeRegistry::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let f = KFunction::alloc_captured(
        scope,
        unit_signature(),
        Body::Builtin(body_no_op),
        false,
        &types,
    );
    scope
        .register_function_direct(
            "FOO".to_string(),
            &f,
            BindingIndex::value(2),
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    let key = f.open(|f| f.signature.untyped_key());
    // A pending sibling is recorded alongside a finalized overload (no longer a
    // no-op): the scope walk parks the bucket until the sibling finalizes.
    scope
        .install_pending_overload(
            key.clone(),
            ProducerId::for_test(99),
            BindingIndex::value(3),
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    let lookup = scope.bindings().lookup_function(&key, Some(9));
    assert_eq!(lookup.overloads.len(), 1);
    assert_eq!(lookup.pending, Some(ProducerId::for_test(99)));
}

#[test]
fn lookup_function_empty_bucket_under_full_filter_surfaces_no_overloads() {
    let types = TypeRegistry::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let f = KFunction::alloc_captured(
        scope,
        unit_signature(),
        Body::Builtin(body_no_op),
        false,
        &types,
    );
    scope
        .register_function_direct(
            "FOO".to_string(),
            &f,
            BindingIndex::value(9),
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    let key = f.open(|f| f.signature.untyped_key());
    // Empty-after-filter must surface an empty `overloads` with no pending, so
    // the dispatch walker keeps walking ancestors.
    let lookup = scope.bindings().lookup_function(&key, Some(3));
    assert!(lookup.overloads.is_empty());
    assert!(lookup.pending.is_none());
}

/// The producer-failure sweep reaches dispatch buckets. One bucket-keyed binder claims a slot in
/// every inner-call bucket it declares, so the purge keys on the producer each pending slot
/// carries and spans all of them; a sibling producer's claim and a finalized overload both
/// survive, and a bucket the purge empties loses its key.
#[test]
fn clear_placeholders_purges_every_bucket_the_binder_claimed() {
    let types = TypeRegistry::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let f = KFunction::alloc_captured(
        scope,
        unit_signature(),
        Body::Builtin(body_no_op),
        false,
        &types,
    );
    scope
        .register_function_direct(
            "FOO".to_string(),
            &f,
            BindingIndex::value(1),
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    let sealed_key = f.open(|f| f.signature.untyped_key());
    let other_key = SignatureDraft {
        return_type: ReturnType::Resolved(KType::ANY),
        elements: vec![SignatureElement::Keyword("BAR")],
    }
    .untyped_key();

    // Edge 7 is one binder declaring two inner-call buckets; edge 8 is a sibling sharing one.
    for (key, claim, idx) in [
        (&sealed_key, ProducerId::for_test(7), 2),
        (&other_key, ProducerId::for_test(7), 2),
        (&other_key, ProducerId::for_test(8), 3),
    ] {
        scope
            .install_pending_overload(
                key.clone(),
                claim,
                BindingIndex::value(idx),
                &mut crate::machine::WriteGate::for_test(),
            )
            .unwrap();
    }

    scope.clear_placeholders_for_producers(
        &[ProducerId::for_test(7)],
        &mut crate::machine::WriteGate::for_test(),
    );

    let bindings = scope.bindings();
    // The failed binder's claims are gone from both buckets it reached.
    assert!(bindings.pending_overload_entries(&sealed_key).is_empty());
    assert_eq!(
        bindings
            .pending_overload_entries(&other_key)
            .iter()
            .map(|p| p.producer)
            .collect::<Vec<_>>(),
        vec![ProducerId::for_test(8)],
    );
    // The finalized overload sharing a bucket with a purged claim survives.
    assert_eq!(
        bindings.lookup_function(&sealed_key, None).overloads.len(),
        1
    );

    // Purging the last claim in a bucket that holds nothing else drops the key.
    bindings.clear_placeholders_for_producers(
        &[ProducerId::for_test(8)],
        &mut crate::machine::WriteGate::for_test(),
    );
    assert!(
        !bindings
            .functions()
            .contains_key(&UntypedKeyProbe(&other_key))
    );
    assert!(
        bindings
            .functions()
            .contains_key(&UntypedKeyProbe(&sealed_key))
    );
}
