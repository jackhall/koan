//! Unit coverage for the stage-1.2 `types` map and its `try_register_type` write
//! primitive, plus the stage-1.3 `try_register_nominal` atomic-install primitive.
//! `try_register_type` is now live (stage 1.4 wired `Scope::register_type` onto it);
//! `try_register_nominal` remains unused until stage 3 migrates STRUCT / UNION /
//! MODULE finalize paths onto it. These tests directly exercise `Bindings` against
//! `RuntimeArena`-allocated `&KType` / `&KObject` values.

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
    // Type-side storage is the only home for this binding — `data` stays empty.
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
    // First binding remains intact — the collision must not overwrite.
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
    // Live read borrow blocked the write; nothing was inserted.
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
    // Atomic install: both maps hold the exact pointers we supplied.
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
    // Pre-check rejected the transaction before either insert: data side untouched.
    assert!(bindings.data().get("Foo").is_none());
    // First types binding survives intact.
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
    // Pre-check rejected the transaction before either insert: types side untouched.
    assert!(bindings.types().get("Foo").is_none());
    // First data binding survives intact.
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
    // Borrow contention on `types` blocked the write: both maps untouched.
    assert!(_r.get("Foo").is_none());
    assert!(bindings.data().get("Foo").is_none());
}

/// Stage 3.0d scaffolding: `Bindings::new()` initializes `pending_types` empty.
/// No writer in 3.0 — the field is observable only as an empty map until stage 3.2
/// wires the SCC pre-registration pass.
#[test]
fn new_bindings_has_empty_pending_types() {
    let bindings: Bindings<'_> = Bindings::new();
    assert!(bindings.pending_types().is_empty());
}

/// Stage 3.2: the SCC cycle-close sweep pre-installs each member's identity via
/// `try_register_type`. The eventual `try_register_nominal` call observes the
/// matching pre-installed identity and writes only the carrier into `data`. Pins
/// the idempotent arm against regression.
#[test]
fn try_register_nominal_is_idempotent_against_matching_pre_installed_types() {
    let arena = RuntimeArena::new();
    let bindings: Bindings<'_> = Bindings::new();
    // Build two pointer-distinct but value-equal KTypes — cycle-close and finalize
    // each alloc their own.
    let kt_pre: &KType = arena.alloc(KType::UserType {
        kind: UserTypeKind::Struct,
        scope_id: ScopeId::from_raw(0, 0xDEAD_BEEF),
        name: "Foo".into(),
    });
    let kt_finalize: &KType = arena.alloc(KType::UserType {
        kind: UserTypeKind::Struct,
        scope_id: ScopeId::from_raw(0, 0xDEAD_BEEF),
        name: "Foo".into(),
    });
    assert!(!std::ptr::eq(kt_pre, kt_finalize), "alloc should produce distinct pointers");
    assert_eq!(*kt_pre, *kt_finalize, "values must be equal");
    let obj: &KObject<'_> = arena.alloc(KObject::Number(1.0));
    bindings.try_register_type("Foo", kt_pre, BindingIndex::BUILTIN).unwrap();
    // try_register_nominal: types[Foo] already populated with matching identity,
    // data[Foo] empty → idempotent path, write only data.
    let outcome = bindings
        .try_register_nominal("Foo", kt_finalize, obj, BindingIndex::BUILTIN)
        .expect("idempotent arm should succeed");
    assert!(matches!(outcome, ApplyOutcome::Applied));
    // The types entry keeps the PRE-installed pointer (not the finalize's).
    let (stored_kt, _) = *bindings.types().get("Foo").expect("Foo in types");
    assert!(std::ptr::eq(stored_kt, kt_pre));
    // The data entry is the finalize's carrier.
    let (stored_obj, _) = *bindings.data().get("Foo").expect("Foo in data");
    assert!(std::ptr::eq(stored_obj, obj));
}

/// RAII guard: dropping the value returned by `insert_pending_type` removes the
/// matching entry from `pending_types`. Pins the lifecycle invariant that the
/// guard — not any caller-side `remove` — is the sole removal path.
#[test]
fn pending_binder_guard_drop_removes_entry() {
    use crate::machine::model::ast::KExpression;
    let bindings: Box<Bindings<'static>> = Box::default();
    let bindings: &'static Bindings<'static> = Box::leak(bindings);
    let entry = PendingTypeEntry {
        kind: UserTypeKind::Struct,
        scope_id: ScopeId::from_raw(0, 0xBEEF),
        schema_expr: KExpression::new(Vec::new()),
        edges: Vec::new(),
    };
    {
        let _guard = bindings.insert_pending_type("Foo".into(), entry);
        assert!(bindings.pending_types().contains_key("Foo"));
    } // guard drops here
    assert!(
        !bindings.pending_types().contains_key("Foo"),
        "guard Drop should have removed the pending_types entry",
    );
}

/// Guard Drop tolerates an entry that has already been removed (e.g. a future
/// path where finalize drains explicitly). The current code never double-removes,
/// but the Drop must not panic if the map turns out to be empty for the name.
#[test]
fn pending_binder_guard_drop_tolerates_absent_entry() {
    use crate::machine::model::ast::KExpression;
    let bindings: Box<Bindings<'static>> = Box::default();
    let bindings: &'static Bindings<'static> = Box::leak(bindings);
    let entry = PendingTypeEntry {
        kind: UserTypeKind::Struct,
        scope_id: ScopeId::from_raw(0, 0xBEEF),
        schema_expr: KExpression::new(Vec::new()),
        edges: Vec::new(),
    };
    let guard = bindings.insert_pending_type("Foo".into(), entry);
    // Pull the entry out from under the guard.
    bindings.pending_remove("Foo");
    // Guard drop should silently succeed.
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
