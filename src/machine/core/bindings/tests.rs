//! Unit coverage for the `types` map write primitives `try_register_type` and
//! `try_register_nominal`, plus the `pending_types` RAII guard lifecycle.

use super::*;
use crate::machine::core::arena::RuntimeArena;
use crate::machine::core::scope_id::ScopeId;
use crate::machine::model::types::{KType, UserTypeKind};
use crate::machine::model::values::KObject;

#[test]
fn try_register_type_inserts_into_types_map() {
    let arena = RuntimeArena::new();
    let bindings: Bindings<'_> = Bindings::new();
    let kt: &KType = arena.alloc(KType::Number);
    let outcome = bindings
        .try_register_type("Foo", kt, BindingIndex::BUILTIN)
        .expect("try_register_type should succeed on fresh bindings");
    assert!(matches!(outcome, ApplyOutcome::Applied));
    let (stored, _) = *bindings.types().get("Foo").expect("Foo should be in types map");
    assert!(std::ptr::eq(stored, kt));
    assert!(bindings.data().get("Foo").is_none());
}

#[test]
fn try_register_type_rejects_collision_with_rebind() {
    let arena = RuntimeArena::new();
    let bindings: Bindings<'_> = Bindings::new();
    let kt1: &KType = arena.alloc(KType::Number);
    let kt2: &KType = arena.alloc(KType::Str);
    bindings
        .try_register_type("Foo", kt1, BindingIndex::BUILTIN)
        .expect("first register should succeed");
    let err = match bindings.try_register_type("Foo", kt2, BindingIndex::BUILTIN) {
        Err(e) => e,
        Ok(_) => panic!("second register on same name should error, not succeed"),
    };
    assert!(matches!(err.kind, KErrorKind::Rebind { ref name } if name == "Foo"));
    let (stored, _) = *bindings.types().get("Foo").expect("Foo should still be present");
    assert!(std::ptr::eq(stored, kt1));
}

#[test]
fn try_register_type_yields_conflict_on_live_types_borrow() {
    let arena = RuntimeArena::new();
    let bindings: Bindings<'_> = Bindings::new();
    let kt: &KType = arena.alloc(KType::Number);
    let _r = bindings.types();
    let outcome = bindings
        .try_register_type("Foo", kt, BindingIndex::BUILTIN)
        .expect("conflict path returns Ok(Conflict), not Err");
    assert!(matches!(outcome, ApplyOutcome::Conflict));
    assert!(_r.get("Foo").is_none());
}

#[test]
fn try_register_type_clears_matching_placeholder() {
    let arena = RuntimeArena::new();
    let bindings: Bindings<'_> = Bindings::new();
    let kt: &KType = arena.alloc(KType::Number);
    bindings
        .try_install_placeholder("Bar".to_string(), NodeId(7), BindingIndex::BUILTIN)
        .expect("placeholder install should succeed on fresh bindings");
    assert!(bindings.placeholders().contains_key("Bar"));
    bindings
        .try_register_type("Bar", kt, BindingIndex::BUILTIN)
        .expect("type register should succeed and clear placeholder");
    assert!(!bindings.placeholders().contains_key("Bar"));
}

#[test]
fn try_register_type_does_not_touch_data_or_functions() {
    let arena = RuntimeArena::new();
    let bindings: Bindings<'_> = Bindings::new();
    let kt: &KType = arena.alloc(KType::Number);
    bindings
        .try_register_type("Foo", kt, BindingIndex::BUILTIN)
        .expect("register should succeed");
    assert!(bindings.data().is_empty());
    assert!(bindings.functions().is_empty());
}

#[test]
fn try_register_nominal_inserts_into_both_maps() {
    let arena = RuntimeArena::new();
    let bindings: Bindings<'_> = Bindings::new();
    let kt: &KType = arena.alloc(KType::Number);
    let obj: &KObject<'_> = arena.alloc(KObject::Number(1.0));
    let outcome = bindings
        .try_register_nominal("Foo", kt, obj, BindingIndex::BUILTIN)
        .expect("try_register_nominal should succeed on fresh bindings");
    assert!(matches!(outcome, ApplyOutcome::Applied));
    let (stored_kt, _) = *bindings.types().get("Foo").expect("Foo should be in types map");
    let (stored_obj, _) = *bindings.data().get("Foo").expect("Foo should be in data map");
    assert!(std::ptr::eq(stored_kt, kt));
    assert!(std::ptr::eq(stored_obj, obj));
}

#[test]
fn try_register_nominal_rejects_collision_in_types_with_rebind() {
    let arena = RuntimeArena::new();
    let bindings: Bindings<'_> = Bindings::new();
    let kt_existing: &KType = arena.alloc(KType::Number);
    let kt_new: &KType = arena.alloc(KType::Str);
    let obj: &KObject<'_> = arena.alloc(KObject::Number(1.0));
    bindings
        .try_register_type("Foo", kt_existing, BindingIndex::BUILTIN)
        .expect("pre-seed types[Foo] should succeed");
    let err = match bindings.try_register_nominal("Foo", kt_new, obj, BindingIndex::BUILTIN) {
        Err(e) => e,
        Ok(_) => panic!("collision on types side must Err(Rebind), not Ok"),
    };
    assert!(matches!(err.kind, KErrorKind::Rebind { ref name } if name == "Foo"));
    // Pre-check rejected the transaction before either insert.
    assert!(bindings.data().get("Foo").is_none());
    let (stored, _) = *bindings.types().get("Foo").expect("Foo should still be in types");
    assert!(std::ptr::eq(stored, kt_existing));
}

#[test]
fn try_register_nominal_rejects_collision_in_data_with_rebind() {
    let arena = RuntimeArena::new();
    let bindings: Bindings<'_> = Bindings::new();
    let kt: &KType = arena.alloc(KType::Number);
    let obj_existing: &KObject<'_> = arena.alloc(KObject::Number(42.0));
    let obj_new: &KObject<'_> = arena.alloc(KObject::Number(7.0));
    bindings
        .try_bind_value("Foo", obj_existing, BindingIndex::BUILTIN)
        .expect("pre-seed data[Foo] should succeed");
    let err = match bindings.try_register_nominal("Foo", kt, obj_new, BindingIndex::BUILTIN) {
        Err(e) => e,
        Ok(_) => panic!("collision on data side must Err(Rebind), not Ok"),
    };
    assert!(matches!(err.kind, KErrorKind::Rebind { ref name } if name == "Foo"));
    // Pre-check rejected the transaction before either insert.
    assert!(bindings.types().get("Foo").is_none());
    let (stored, _) = *bindings.data().get("Foo").expect("Foo should still be in data");
    assert!(std::ptr::eq(stored, obj_existing));
}

#[test]
fn try_register_nominal_yields_conflict_on_live_types_borrow() {
    let arena = RuntimeArena::new();
    let bindings: Bindings<'_> = Bindings::new();
    let kt: &KType = arena.alloc(KType::Number);
    let obj: &KObject<'_> = arena.alloc(KObject::Number(1.0));
    let _r = bindings.types();
    let outcome = bindings
        .try_register_nominal("Foo", kt, obj, BindingIndex::BUILTIN)
        .expect("conflict path returns Ok(Conflict), not Err");
    assert!(matches!(outcome, ApplyOutcome::Conflict));
    assert!(_r.get("Foo").is_none());
    assert!(bindings.data().get("Foo").is_none());
}

#[test]
fn new_bindings_has_empty_pending_types() {
    let bindings: Bindings<'_> = Bindings::new();
    assert!(bindings.pending_types().is_empty());
}

/// The SCC cycle-close sweep pre-installs each member's identity via
/// `try_register_type`; the eventual `try_register_nominal` observes the
/// matching identity and writes only the carrier into `data`.
#[test]
fn try_register_nominal_is_idempotent_against_matching_pre_installed_types() {
    let arena = RuntimeArena::new();
    let bindings: Bindings<'_> = Bindings::new();
    // Pointer-distinct but value-equal KTypes: cycle-close and finalize each alloc their own.
    let kt_pre: &KType = arena.alloc(KType::UserType {
        kind: UserTypeKind::struct_sentinel(),
        scope_id: ScopeId::from_raw(0, 0xDEAD_BEEF),
        name: "Foo".into(),
    });
    let kt_finalize: &KType = arena.alloc(KType::UserType {
        kind: UserTypeKind::struct_sentinel(),
        scope_id: ScopeId::from_raw(0, 0xDEAD_BEEF),
        name: "Foo".into(),
    });
    assert!(!std::ptr::eq(kt_pre, kt_finalize), "alloc should produce distinct pointers");
    assert_eq!(*kt_pre, *kt_finalize, "values must be equal");
    let obj: &KObject<'_> = arena.alloc(KObject::Number(1.0));
    bindings.try_register_type("Foo", kt_pre, BindingIndex::BUILTIN).unwrap();
    let outcome = bindings
        .try_register_nominal("Foo", kt_finalize, obj, BindingIndex::BUILTIN)
        .expect("idempotent arm should succeed");
    assert!(matches!(outcome, ApplyOutcome::Applied));
    // types keeps the PRE-installed pointer, data takes the finalize's carrier.
    let (stored_kt, _) = *bindings.types().get("Foo").expect("Foo in types");
    assert!(std::ptr::eq(stored_kt, kt_pre));
    let (stored_obj, _) = *bindings.data().get("Foo").expect("Foo in data");
    assert!(std::ptr::eq(stored_obj, obj));
}

/// Dropping the value returned by `insert_pending_type` is the sole removal path
/// for a `pending_types` entry outside `#[cfg(test)]`.
#[test]
fn pending_binder_guard_drop_removes_entry() {
    use crate::machine::model::ast::KExpression;
    let bindings: Box<Bindings<'static>> = Box::default();
    let bindings: &'static Bindings<'static> = Box::leak(bindings);
    let entry = PendingTypeEntry {
        kind: UserTypeKind::struct_sentinel(),
        scope_id: ScopeId::from_raw(0, 0xBEEF),
        schema_expr: KExpression::new(Vec::new()),
        edges: Vec::new(),
    };
    {
        let _guard = bindings.insert_pending_type("Foo".into(), entry);
        assert!(bindings.pending_types().contains_key("Foo"));
    }
    assert!(
        !bindings.pending_types().contains_key("Foo"),
        "guard Drop should have removed the pending_types entry",
    );
}

/// Guard Drop must tolerate an already-removed entry without panicking.
#[test]
fn pending_binder_guard_drop_tolerates_absent_entry() {
    use crate::machine::model::ast::KExpression;
    let bindings: Box<Bindings<'static>> = Box::default();
    let bindings: &'static Bindings<'static> = Box::leak(bindings);
    let entry = PendingTypeEntry {
        kind: UserTypeKind::struct_sentinel(),
        scope_id: ScopeId::from_raw(0, 0xBEEF),
        schema_expr: KExpression::new(Vec::new()),
        edges: Vec::new(),
    };
    let guard = bindings.insert_pending_type("Foo".into(), entry);
    bindings.pending_remove("Foo");
    drop(guard);
    assert!(!bindings.pending_types().contains_key("Foo"));
}

#[test]
fn try_register_nominal_clears_matching_placeholder() {
    let arena = RuntimeArena::new();
    let bindings: Bindings<'_> = Bindings::new();
    let kt: &KType = arena.alloc(KType::Number);
    let obj: &KObject<'_> = arena.alloc(KObject::Number(1.0));
    bindings
        .try_install_placeholder("Bar".to_string(), NodeId(7), BindingIndex::BUILTIN)
        .expect("placeholder install should succeed on fresh bindings");
    assert!(bindings.placeholders().contains_key("Bar"));
    bindings
        .try_register_nominal("Bar", kt, obj, BindingIndex::BUILTIN)
        .expect("nominal register should succeed and clear placeholder");
    assert!(!bindings.placeholders().contains_key("Bar"));
}
