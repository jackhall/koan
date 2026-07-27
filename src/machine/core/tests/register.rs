//! `register` arm of `machine::core` tests.

use super::super::{BindingIndex, DeclarationSite, NameLookup, Scope};
use crate::builtins::test_support::{mock_declaration_site, run_root_bare};
use crate::machine::core::kfunction::{Body, KFunction, NodeId};
use crate::machine::core::{run_root_storage, FrameStorageExt};
use crate::machine::model::Carried;
use crate::machine::model::KObject;
use crate::machine::model::TypeRegistry;
use crate::machine::model::{Argument, ExpressionSignature, KType, ReturnType, SignatureElement};

use super::{body_no_op, unit_signature};
use crate::machine::CarrierWitness;

// `BindingIndex::BUILTIN` is used throughout because these tests exercise the
// `Bindings` write rules (rebind, dedupe, placeholder lifecycle) rather than the
// chain-gated `Scope::resolve` path.

#[test]
fn bind_value_direct_errors_on_same_scope_rebind() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let v1 = region.brand().alloc_object(KObject::Number(1.0));
    let v2 = region.brand().alloc_object(KObject::Number(2.0));
    scope
        .bind_resident_for_test(
            "x".to_string(),
            v1,
            BindingIndex::BUILTIN,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    let err = scope
        .bind_resident_for_test(
            "x".to_string(),
            v2,
            BindingIndex::BUILTIN,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap_err();
    match &err.kind {
        crate::machine::core::KErrorKind::Rebind { name } => assert_eq!(name, "x"),
        _ => panic!("expected Rebind, got {err}"),
    }
}

#[test]
fn bind_value_direct_allows_shadowing_in_child_scope() {
    let region = run_root_storage();
    let outer = run_root_bare(&region);
    let v1 = region.brand().alloc_object(KObject::Number(1.0));
    outer
        .bind_resident_for_test(
            "x".to_string(),
            v1,
            BindingIndex::BUILTIN,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    let inner = region.brand().alloc_scope(outer.child_for_call());
    let v2 = region.brand().alloc_object(KObject::Number(2.0));
    inner
        .bind_resident_for_test(
            "x".to_string(),
            v2,
            BindingIndex::BUILTIN,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    assert!(matches!(inner.lookup("x"), Some(KObject::Number(n)) if *n == 2.0));
    assert!(matches!(outer.lookup("x"), Some(KObject::Number(n)) if *n == 1.0));
}

#[test]
fn close_marks_scope_and_is_idempotent_reads_still_work() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let v = region.brand().alloc_object(KObject::Number(1.0));
    scope
        .bind_resident_for_test(
            "x".to_string(),
            v,
            BindingIndex::BUILTIN,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    assert!(!scope.is_closed());
    scope.close();
    assert!(scope.is_closed());
    scope.close(); // idempotent
    assert!(scope.is_closed());
    // Reads stay legal after close — only binds are rejected.
    assert!(matches!(scope.lookup("x"), Some(KObject::Number(n)) if *n == 1.0));
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "closed scope")]
fn bind_after_close_panics() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    scope.close();
    let v = region.brand().alloc_object(KObject::Number(1.0));
    let _ = scope.bind_resident_for_test(
        "x".to_string(),
        v,
        BindingIndex::BUILTIN,
        &mut crate::machine::WriteGate::for_test(),
    );
}

#[test]
fn close_is_per_scope_open_child_still_binds() {
    let region = run_root_storage();
    let outer = run_root_bare(&region);
    outer.close();
    let inner = region.brand().alloc_scope(outer.child_for_call());
    let v = region.brand().alloc_object(KObject::Number(2.0));
    inner
        .bind_resident_for_test(
            "x".to_string(),
            v,
            BindingIndex::BUILTIN,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    assert!(matches!(inner.lookup("x"), Some(KObject::Number(n)) if *n == 2.0));
    assert!(!inner.is_closed());
}

#[test]
fn register_function_dedupes_exact_signature() {
    let types = TypeRegistry::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let f1 = region.brand().alloc_function(KFunction::new(
        unit_signature(),
        Body::Builtin(body_no_op),
        scope,
        false,
        &types,
    ));
    scope
        .register_function_direct(
            "FOO".to_string(),
            f1,
            BindingIndex::BUILTIN,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    let f2 = region.brand().alloc_function(KFunction::new(
        unit_signature(),
        Body::Builtin(body_no_op),
        scope,
        false,
        &types,
    ));
    let err = scope
        .register_function_direct(
            "FOO".to_string(),
            f2,
            BindingIndex::BUILTIN,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap_err();
    assert!(
        matches!(&err.kind, crate::machine::core::KErrorKind::DuplicateOverload { name, .. } if name == "FOO"),
        "expected DuplicateOverload, got {err}",
    );
}

/// Routing a structurally identical but pointer-distinct `KFunction` through the LET
/// path (a `WriteOp::Callable` over a `KObject::KFunction`) must also trip `DuplicateOverload` —
/// the unified `try_apply` shares the FN dedupe rule.
#[test]
fn bind_value_direct_with_kfunction_dedupes_exact_signature_with_existing_fn() {
    let types = TypeRegistry::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let f1 = region.brand().alloc_function(KFunction::new(
        unit_signature(),
        Body::Builtin(body_no_op),
        scope,
        false,
        &types,
    ));
    scope
        .register_function_direct(
            "FOO".to_string(),
            f1,
            BindingIndex::BUILTIN,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    let f2 = region.brand().alloc_function(KFunction::new(
        unit_signature(),
        Body::Builtin(body_no_op),
        scope,
        false,
        &types,
    ));
    let obj2 = region
        .brand()
        .alloc_object_checked(KObject::KFunction(f2), &types)
        .expect("f was just allocated into region's own region");
    let err = scope
        .bind_resident_for_test(
            "OTHER_NAME".to_string(),
            obj2,
            BindingIndex::BUILTIN,
            &mut crate::machine::WriteGate::for_test(),
        )
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
fn bind_value_direct_with_kfunction_pointer_equal_alias_no_op() {
    let types = TypeRegistry::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let f = region.brand().alloc_function(KFunction::new(
        unit_signature(),
        Body::Builtin(body_no_op),
        scope,
        false,
        &types,
    ));
    let obj1 = region
        .brand()
        .alloc_object_checked(KObject::KFunction(f), &types)
        .expect("f was just allocated into region's own region");
    let obj2 = region
        .brand()
        .alloc_object_checked(KObject::KFunction(f), &types)
        .expect("f was just allocated into region's own region");
    scope
        .bind_resident_for_test(
            "FIRST".to_string(),
            obj1,
            BindingIndex::BUILTIN,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    scope
        .bind_resident_for_test(
            "ALIAS".to_string(),
            obj2,
            BindingIndex::BUILTIN,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
}

#[test]
fn register_function_allows_overload_with_different_arg_types() {
    let types = TypeRegistry::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let sig_num = ExpressionSignature {
        return_type: ReturnType::Resolved(KType::ANY),
        elements: vec![
            SignatureElement::Keyword("BAR".into()),
            SignatureElement::Argument(Argument {
                name: "v".into(),
                ktype: KType::NUMBER,
            }),
        ],
    };
    let sig_str = ExpressionSignature {
        return_type: ReturnType::Resolved(KType::ANY),
        elements: vec![
            SignatureElement::Keyword("BAR".into()),
            SignatureElement::Argument(Argument {
                name: "v".into(),
                ktype: KType::STR,
            }),
        ],
    };
    let f1 = region.brand().alloc_function(KFunction::new(
        sig_num,
        Body::Builtin(body_no_op),
        scope,
        false,
        &types,
    ));
    let f2 = region.brand().alloc_function(KFunction::new(
        sig_str,
        Body::Builtin(body_no_op),
        scope,
        false,
        &types,
    ));
    scope
        .register_function_direct(
            "BAR".to_string(),
            f1,
            BindingIndex::BUILTIN,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    scope
        .register_function_direct(
            "BAR".to_string(),
            f2,
            BindingIndex::BUILTIN,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
}

/// `register_function` touches only `functions`, never `data`, so a bare FN may
/// coexist with a same-name value binding. The two namespaces stay independent.
#[test]
fn register_function_coexists_with_same_name_value() {
    let types = TypeRegistry::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let v = region.brand().alloc_object(KObject::Number(1.0));
    scope
        .bind_resident_for_test(
            "FOO".to_string(),
            v,
            BindingIndex::BUILTIN,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    let f = region.brand().alloc_function(KFunction::new(
        unit_signature(),
        Body::Builtin(body_no_op),
        scope,
        false,
        &types,
    ));
    scope
        .register_function_direct(
            "FOO".to_string(),
            f,
            BindingIndex::BUILTIN,
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("bare FN registration must not collide with a same-name value");
    assert!(matches!(scope.lookup("FOO"), Some(KObject::Number(n)) if *n == 1.0));
    let key = f.signature.untyped_key();
    assert!(scope
        .bindings()
        .functions()
        .get(&key)
        .map(|b| !b.is_empty())
        .unwrap_or(false));
}

/// The cross-kind exclusion guards the value/type partition, but a bare FN binds
/// neither `data` nor `types` (it writes only `functions`, `write_data == false`),
/// so it is exempt: a same-name type and a bare FN coexist.
#[test]
fn register_function_coexists_with_same_name_type() {
    let types = TypeRegistry::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let _ = scope.register_type_direct(
        "Foo".to_string(),
        KType::NUMBER,
        DeclarationSite::BUILTIN,
        &mut crate::machine::WriteGate::for_test(),
    );
    let f = region.brand().alloc_function(KFunction::new(
        unit_signature(),
        Body::Builtin(body_no_op),
        scope,
        false,
        &types,
    ));
    scope
        .register_function_direct(
            "Foo".to_string(),
            f,
            BindingIndex::BUILTIN,
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("bare FN registration must not collide with a same-name type");
    assert!(scope.bindings().types().get("Foo").is_some());
    let key = f.signature.untyped_key();
    assert!(scope
        .bindings()
        .functions()
        .get(&key)
        .map(|b| !b.is_empty())
        .unwrap_or(false));
}

/// `lookup_member` (the one classified ATTR lookup) yields exactly one result per name: a
/// value-classified bind surfaces as `Value`, a type-classified bind as `Type`, and an unbound
/// name as `None`. The cross-kind exclusion keeps a name from being both, so it never ambiguates.
#[test]
fn lookup_member_classifies_value_and_type_unambiguously() {
    use crate::machine::core::MemberResolution;
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let v = region.brand().alloc_object(KObject::Number(1.0));
    scope
        .bind_resident_for_test(
            "val".to_string(),
            v,
            BindingIndex::BUILTIN,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    let _ = scope.register_type_direct(
        "Ty".to_string(),
        KType::NUMBER,
        DeclarationSite::BUILTIN,
        &mut crate::machine::WriteGate::for_test(),
    );
    let bindings = scope.bindings();
    assert!(matches!(
        bindings.lookup_member("val", None),
        Some(MemberResolution::Value(sealed))
            if matches!(sealed.open_at(&region).value(), Carried::Object(KObject::Number(n)) if *n == 1.0)
    ));
    assert!(matches!(
        bindings.lookup_member("Ty", None),
        Some(MemberResolution::Type { kt, .. }) if kt == KType::NUMBER
    ));
    assert!(bindings.lookup_member("absent", None).is_none());
}

#[test]
fn resolve_returns_placeholder_when_only_placeholder_exists() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    scope
        .install_placeholder(
            "x".to_string(),
            NodeId(7),
            BindingIndex::BUILTIN,
            crate::machine::model::BindKind::Value,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    match scope.resolve("x") {
        Some(NameLookup::Parked(id)) => assert_eq!(id, NodeId(7)),
        _ => panic!("expected Placeholder"),
    }
}

#[test]
fn resolve_stops_at_first_hit_does_not_descend_outer() {
    let region = run_root_storage();
    let outer = run_root_bare(&region);
    let v = region.brand().alloc_object(KObject::Number(1.0));
    outer
        .bind_resident_for_test(
            "x".to_string(),
            v,
            BindingIndex::BUILTIN,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    let inner = region.brand().alloc_scope(outer.child_for_call());
    inner
        .install_placeholder(
            "x".to_string(),
            NodeId(3),
            BindingIndex::BUILTIN,
            crate::machine::model::BindKind::Value,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    match inner.resolve("x") {
        Some(NameLookup::Parked(id)) => assert_eq!(id, NodeId(3)),
        other => panic!(
            "expected Placeholder from inner — outer's Value should not shadow it. Got {}",
            match other {
                Some(NameLookup::Bound(_)) => "Bound",
                Some(NameLookup::Parked(_)) => "Parked",
                None => "Unbound",
            }
        ),
    }
}

#[test]
fn bind_value_direct_clears_own_placeholder() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    scope
        .install_placeholder(
            "x".to_string(),
            NodeId(2),
            BindingIndex::BUILTIN,
            crate::machine::model::BindKind::Value,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    let v = region.brand().alloc_object(KObject::Number(42.0));
    scope
        .bind_resident_for_test(
            "x".to_string(),
            v,
            BindingIndex::BUILTIN,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    assert!(scope.bindings().placeholders().get("x").is_none());
    assert!(
        matches!(scope.resolve("x"), Some(NameLookup::Bound(KObject::Number(n))) if *n == 42.0)
    );
}

// Visibility-gate unit tests: exercise `Scope::resolve_with_chain` /
// `Scope::resolve_type_with_chain` directly so the index-gated predicate's semantics
// are pinned independent of the scheduler.

#[test]
fn visibility_chain_none_sees_every_entry() {
    use crate::machine::core::LexicalFrame;
    use std::rc::Rc;
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let v = region.brand().alloc_object(KObject::Number(7.0));
    scope
        .bind_resident_for_test(
            "late".to_string(),
            v,
            BindingIndex::value(99),
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    // A chain whose `index_for(scope.id) = None` treats the scope as complete:
    // every entry is visible regardless of index.
    let other_scope_id = crate::machine::core::ScopeId::next();
    let unrelated: Rc<LexicalFrame> = LexicalFrame::root(other_scope_id, 1);
    assert!(matches!(
        scope.resolve_with_chain("late", Some(&unrelated)),
        Some(NameLookup::Bound(KObject::Number(n))) if *n == 7.0,
    ));
}

#[test]
fn visibility_strict_less_than_hides_later_sibling() {
    use crate::machine::core::LexicalFrame;
    use std::rc::Rc;
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let v = region.brand().alloc_object(KObject::Number(7.0));
    scope
        .bind_resident_for_test(
            "later".to_string(),
            v,
            BindingIndex::value(5),
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    // Cutoff 3, producer at 5 → `5 < 3` is false → invisible.
    let consumer: Rc<LexicalFrame> = LexicalFrame::root(scope.id, 3);
    assert!(scope.resolve_with_chain("later", Some(&consumer)).is_none());
}

#[test]
fn visibility_strict_less_than_admits_earlier_sibling() {
    use crate::machine::core::LexicalFrame;
    use std::rc::Rc;
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let v = region.brand().alloc_object(KObject::Number(7.0));
    scope
        .bind_resident_for_test(
            "earlier".to_string(),
            v,
            BindingIndex::value(2),
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    let consumer: Rc<LexicalFrame> = LexicalFrame::root(scope.id, 5);
    assert!(matches!(
        scope.resolve_with_chain("earlier", Some(&consumer)),
        Some(NameLookup::Bound(KObject::Number(n))) if *n == 7.0,
    ));
}

#[test]
fn visibility_self_index_hidden_under_strict_less_than() {
    use crate::machine::core::LexicalFrame;
    use std::rc::Rc;
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let v = region.brand().alloc_object(KObject::Number(7.0));
    scope
        .bind_resident_for_test(
            "self_idx".to_string(),
            v,
            BindingIndex::value(3),
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    // Cutoff equal to producer idx (e.g. `LET x = x`): `3 < 3` is false.
    let consumer: Rc<LexicalFrame> = LexicalFrame::root(scope.id, 3);
    assert!(scope
        .resolve_with_chain("self_idx", Some(&consumer))
        .is_none());
}

#[test]
fn visibility_placeholder_filtered_same_as_value() {
    use crate::machine::core::LexicalFrame;
    use std::rc::Rc;
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    scope
        .install_placeholder(
            "ph".to_string(),
            NodeId(2),
            BindingIndex::value(5),
            crate::machine::model::BindKind::Value,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    let consumer: Rc<LexicalFrame> = LexicalFrame::root(scope.id, 3);
    assert!(scope.resolve_with_chain("ph", Some(&consumer)).is_none());
    let consumer_after: Rc<LexicalFrame> = LexicalFrame::root(scope.id, 9);
    assert!(matches!(
        scope.resolve_with_chain("ph", Some(&consumer_after)),
        Some(NameLookup::Parked(_)),
    ));
}

#[test]
fn visibility_type_side_gate_mirrors_value_side() {
    use crate::machine::core::LexicalFrame;
    use std::rc::Rc;
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let _ = scope.register_type_direct(
        "TyLate".to_string(),
        KType::NUMBER,
        mock_declaration_site(1, 5),
        &mut crate::machine::WriteGate::for_test(),
    );
    let consumer_before: Rc<LexicalFrame> = LexicalFrame::root(scope.id, 3);
    assert!(scope
        .resolve_type_with_chain("TyLate", Some(&consumer_before))
        .is_none());
    let consumer_after: Rc<LexicalFrame> = LexicalFrame::root(scope.id, 9);
    assert!(scope
        .resolve_type_with_chain("TyLate", Some(&consumer_after))
        .is_some());
}

/// Partition strictness at a SIG decl scope: `child_under_sig` mints an ordinary `Bindings`
/// (no slot-table carve-out), so a value-token key written straight to `types` is rejected by
/// `partition_guard` exactly like on any other scope's bindings.
#[test]
fn sig_scope_bindings_reject_value_token_type_write() {
    let region = run_root_storage();
    let outer = run_root_bare(&region);
    let sig_scope = region
        .brand()
        .alloc_scope(Scope::child_under_sig(outer, "S".to_string()));
    let kt: KType = KType::NUMBER;
    let error = match sig_scope.bindings().write_type(
        "compare",
        kt,
        DeclarationSite::BUILTIN,
        crate::machine::core::bindings::TypeWritePolicy::Insert,
        &mut crate::machine::WriteGate::for_test(),
    ) {
        Err(e) => e,
        Ok(_) => panic!("a value-token key must never enter `types`, even on a SIG decl scope"),
    };
    assert!(
        matches!(&error.kind, crate::machine::core::KErrorKind::ShapeError(msg) if msg.contains("is a value token")),
        "expected the token-class partition error, got {error}",
    );
}

/// The **mirror seal** states the `data` entry's own claim: a `LET`-bound callable's `functions`
/// bucket entry carries the carrier projected off the very seal the value binds under, so the
/// bucket names every region the callable reaches and dispatch's pick escapes with those pins
/// proven.
#[test]
fn function_mirror_seals_the_data_entrys_own_claim() {
    use crate::machine::core::FrameCoverage;
    use std::rc::Rc;

    let types = TypeRegistry::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let f = region.brand().alloc_function(KFunction::new(
        unit_signature(),
        Body::Builtin(body_no_op),
        scope,
        false,
        &types,
    ));
    let obj = region
        .brand()
        .alloc_object_checked(KObject::KFunction(f), &types)
        .expect("f was just allocated into this region");

    // A foreign region the callable's captured environment stands for: minted into the binding
    // scope's arena exactly as a bind door's mint would, so the seal carries a real reach.
    let foreign = run_root_storage();
    let (reach, borrows_home) = scope.mint_retained(&[&FrameCoverage::of(Rc::clone(&foreign))]);
    let sealed = scope.seal_resident(
        Carried::Object(obj),
        CarrierWitness::new(borrows_home, reach),
    );
    let mirror = scope
        .seal_function_mirror(&sealed)
        .expect("the bound value wraps a callable");
    scope
        .bind_value_direct(
            "f".to_string(),
            sealed,
            Some(mirror),
            BindingIndex::BUILTIN,
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("a fresh callable bind lands");

    let lookup = scope
        .bindings()
        .lookup_function(&f.signature.untyped_key(), None);
    assert_eq!(lookup.overloads.len(), 1, "the mirror wrote one overload");
    assert!(
        scope
            .open_function(&lookup.overloads[0])
            .reach_covers(foreign.region()),
        "the bucket entry names the foreign region the bound value reaches",
    );
}

/// A bare `FN` binds no value, so it has no `data` twin to derive a claim from: it seals the
/// **exact empty** reach. `FN` allocates the callable into the very scope it captures, so its only
/// region borrow is home — which every read of it already pins.
#[test]
fn bare_fn_registration_seals_the_empty_reach() {
    let types = TypeRegistry::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let f = region.brand().alloc_function(KFunction::new(
        unit_signature(),
        Body::Builtin(body_no_op),
        scope,
        false,
        &types,
    ));
    scope
        .register_function_direct(
            "FOO".to_string(),
            f,
            BindingIndex::BUILTIN,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();
    let foreign = run_root_storage();
    let lookup = scope
        .bindings()
        .lookup_function(&f.signature.untyped_key(), None);
    assert_eq!(lookup.overloads.len(), 1);
    assert!(
        !scope
            .open_function(&lookup.overloads[0])
            .reach_covers(foreign.region()),
        "a bare FN reaches nothing foreign",
    );
}
