//! Unit tests for [`crate::machine::core::Bindings::lookup_value`],
//! [`crate::machine::core::Bindings::lookup_type`], and
//! [`crate::machine::core::Bindings::lookup_function`] — the visibility-aware
//! lookups the index-gated resolver walks.

use crate::builtins::test_support::{
    binder_name, mock_declaration_site, run_root_bare, type_name, value_name,
};
use crate::machine::ProducerId;
use crate::machine::core::kfunction::{Body, KFunction};
use crate::machine::core::{BindingIndex, FrameStorageExt, NameLookup, run_root_storage};
use crate::machine::model::KObject;
use crate::machine::model::{Argument, KType, ReturnType, SignatureDraft, SignatureElement};

use super::{body_no_op, unit_signature};
use crate::machine::model::RunRegistries;
use crate::machine::model::Scalar;

#[test]
fn lookup_value_chain_cutoff_none_admits_every_index() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let registries = RunRegistries::new();
    let value = region.brand().alloc_scalar(Scalar::Number(7.0));
    scope
        .bind_resident_for_test(
            value_name("late", &registries),
            value,
            BindingIndex::value(99),
            &registries,
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
    let registries = RunRegistries::new();
    let value = region.brand().alloc_scalar(Scalar::Number(7.0));
    scope
        .bind_resident_for_test(
            value_name("later", &registries),
            value,
            BindingIndex::value(5),
            &registries,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    assert!(
        scope
            .bindings()
            .lookup_value(value_name("later", &registries), Some(3))
            .is_none()
    );
}

#[test]
fn lookup_value_strict_less_than_admits_earlier_sibling() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let registries = RunRegistries::new();
    let value = region.brand().alloc_scalar(Scalar::Number(7.0));
    scope
        .bind_resident_for_test(
            value_name("earlier", &registries),
            value,
            BindingIndex::value(2),
            &registries,
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
    let registries = RunRegistries::new();
    scope
        .install_placeholder(
            binder_name("placeholder", &registries),
            ProducerId::for_test(2),
            BindingIndex::value(5),
            &registries,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    assert!(
        scope
            .bindings()
            .lookup_value(value_name("placeholder", &registries), Some(3))
            .is_none()
    );
    match scope
        .bindings()
        .lookup_value(value_name("placeholder", &registries), Some(9))
    {
        Some(NameLookup::Parked(id)) => assert_eq!(id, ProducerId::for_test(2)),
        _ => panic!("placeholder must be visible past its install index"),
    }
}

#[test]
fn lookup_type_chain_cutoff_none_admits_every_index() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let registries = RunRegistries::new();
    let _ = scope.register_type_direct(
        type_name("Tee", &registries),
        KType::NUMBER,
        mock_declaration_site(99),
        &registries,
        &mut crate::machine::WriteGate::for_test(),
    );
    assert!(matches!(
        scope.bindings().lookup_type(type_name("Tee", &registries), None),
        Some(NameLookup::Bound(kt)) if kt == KType::NUMBER,
    ));
}

#[test]
fn lookup_type_strict_less_than_hides_later_sibling() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let registries = RunRegistries::new();
    let _ = scope.register_type_direct(
        type_name("TyLate", &registries),
        KType::NUMBER,
        mock_declaration_site(5),
        &registries,
        &mut crate::machine::WriteGate::for_test(),
    );
    assert!(
        scope
            .bindings()
            .lookup_type(type_name("TyLate", &registries), Some(3))
            .is_none()
    );
    assert!(
        scope
            .bindings()
            .lookup_type(type_name("TyLate", &registries), Some(9))
            .is_some()
    );
}

#[test]
fn lookup_function_chain_cutoff_none_returns_full_bucket() {
    let registries = RunRegistries::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let cell = KFunction::alloc_captured(
        scope,
        unit_signature(),
        Body::Builtin(body_no_op),
        &registries,
    );
    let f = cell.adopt_into(scope.brand().handle());
    scope
        .register_function_direct(
            "FOO".to_string(),
            &cell,
            BindingIndex::value(99),
            &registries,
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
    let registries = RunRegistries::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    // Two overloads sharing the same bucket key but differing on a value-side
    // argument shape so they coexist in `functions[key]`.
    let sig_num = SignatureDraft {
        return_type: ReturnType::Resolved(KType::ANY),
        elements: vec![
            SignatureElement::keyword("BAR"),
            SignatureElement::Argument(Argument {
                name: crate::machine::model::BinderSymbol::of("v")
                    .expect("a test fixture parameter is a value token"),
                ktype: KType::NUMBER,
            }),
        ],
    };
    let sig_str = SignatureDraft {
        return_type: ReturnType::Resolved(KType::ANY),
        elements: vec![
            SignatureElement::keyword("BAR"),
            SignatureElement::Argument(Argument {
                name: crate::machine::model::BinderSymbol::of("v")
                    .expect("a test fixture parameter is a value token"),
                ktype: KType::STR,
            }),
        ],
    };
    let key = sig_num.untyped_key();
    debug_assert_eq!(key, sig_str.untyped_key(), "untyped keys must collide");
    let early = KFunction::alloc_captured(scope, sig_num, Body::Builtin(body_no_op), &registries);
    let late = KFunction::alloc_captured(scope, sig_str, Body::Builtin(body_no_op), &registries);
    let f_early = early.adopt_into(scope.brand().handle());
    scope
        .register_function_direct(
            "BAR".to_string(),
            &early,
            BindingIndex::value(2),
            &registries,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    scope
        .register_function_direct(
            "BAR".to_string(),
            &late,
            BindingIndex::value(7),
            &registries,
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
    let registries = RunRegistries::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let f = KFunction::alloc_captured(
        scope,
        unit_signature(),
        Body::Builtin(body_no_op),
        &registries,
    );
    scope
        .register_function_direct(
            "FOO".to_string(),
            &f,
            BindingIndex::value(2),
            &registries,
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
    let registries = RunRegistries::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let f = KFunction::alloc_captured(
        scope,
        unit_signature(),
        Body::Builtin(body_no_op),
        &registries,
    );
    scope
        .register_function_direct(
            "FOO".to_string(),
            &f,
            BindingIndex::value(9),
            &registries,
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

/// Retirement reaches every bucket the statement claimed. One bucket-keyed binder claims each
/// inner-call bucket it declares at its own `BindingIndex`, so retiring that index drops all of
/// them in one array read; a sibling statement's claim and a finalized overload both survive.
#[test]
fn retirement_drops_every_bucket_the_statement_claimed() {
    let registries = RunRegistries::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let f = KFunction::alloc_captured(
        scope,
        unit_signature(),
        Body::Builtin(body_no_op),
        &registries,
    );
    scope
        .register_function_direct(
            "FOO".to_string(),
            &f,
            BindingIndex::value(1),
            &registries,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    let sealed_key = f.open(|f| f.signature.untyped_key());
    let other_key = SignatureDraft {
        return_type: ReturnType::Resolved(KType::ANY),
        elements: vec![SignatureElement::keyword("BAR")],
    }
    .untyped_key();

    // Statement 2 declares two inner-call buckets; statement 3 is a sibling sharing one.
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

    scope.retire_claims(
        BindingIndex::value(2),
        &mut crate::machine::WriteGate::for_test(),
    );

    let bindings = scope.bindings();
    // The failed binder's claims are gone from both keys it reached.
    assert!(bindings.pending_overload_entries(&sealed_key).is_empty());
    assert_eq!(
        bindings
            .pending_overload_entries(&other_key)
            .iter()
            .map(|p| p.producer)
            .collect::<Vec<_>>(),
        vec![ProducerId::for_test(8)],
    );
    // The finalized overload sharing a key with a retired claim survives.
    assert_eq!(
        bindings.lookup_function(&sealed_key, None).overloads.len(),
        1
    );

    // A key only ever claimed publishes no dispatch surface at all — the claim never sat in
    // `functions`, so there is no bucket to empty.
    bindings.retire_claims(
        BindingIndex::value(3),
        &mut crate::machine::WriteGate::for_test(),
    );
    assert!(bindings.pending_overload_entries(&other_key).is_empty());
    assert!(!bindings.functions().contains_key(other_key.as_slice()));
    assert!(bindings.functions().contains_key(sealed_key.as_slice()));
}
