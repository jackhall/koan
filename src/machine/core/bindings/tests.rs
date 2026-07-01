//! Unit coverage for the `types` map write primitive `try_register_type`, plus
//! the `pending_types` RAII guard lifecycle and the cross-kind exclusion that
//! makes the `data`/`types` partition structural (no name in both).

use super::*;
use crate::machine::core::arena::FrameStorage;
use crate::machine::core::scope_id::ScopeId;
use crate::machine::model::types::{KKind, KType};
use crate::machine::model::values::KObject;

#[test]
fn try_register_type_inserts_into_types_map() {
    let storage = FrameStorage::run_root();
    let region = storage.brand();
    let bindings: Bindings<'_> = Bindings::new();
    let kt: &KType = region.alloc_ktype(KType::Number);
    let outcome = bindings
        .try_register_type("Foo", kt, BindingIndex::BUILTIN, FrameSet::empty())
        .expect("try_register_type should succeed on fresh bindings");
    assert!(matches!(outcome, ApplyOutcome::Applied));
    let stored = bindings
        .types()
        .get("Foo")
        .expect("Foo should be in types map")
        .0;
    assert!(std::ptr::eq(stored, kt));
    assert!(bindings.data().get("Foo").is_none());
}

#[test]
fn try_register_type_rejects_collision_with_rebind() {
    let storage = FrameStorage::run_root();
    let region = storage.brand();
    let bindings: Bindings<'_> = Bindings::new();
    let kt1: &KType = region.alloc_ktype(KType::Number);
    let kt2: &KType = region.alloc_ktype(KType::Str);
    bindings
        .try_register_type("Foo", kt1, BindingIndex::BUILTIN, FrameSet::empty())
        .expect("first register should succeed");
    let err = match bindings.try_register_type("Foo", kt2, BindingIndex::BUILTIN, FrameSet::empty())
    {
        Err(e) => e,
        Ok(_) => panic!("second register on same name should error, not succeed"),
    };
    assert!(matches!(err.kind, KErrorKind::Rebind { ref name } if name == "Foo"));
    let stored = bindings
        .types()
        .get("Foo")
        .expect("Foo should still be present")
        .0;
    assert!(std::ptr::eq(stored, kt1));
}

#[test]
fn try_register_type_yields_conflict_on_live_types_borrow() {
    let storage = FrameStorage::run_root();
    let region = storage.brand();
    let bindings: Bindings<'_> = Bindings::new();
    let kt: &KType = region.alloc_ktype(KType::Number);
    let _r = bindings.types();
    let outcome = bindings
        .try_register_type("Foo", kt, BindingIndex::BUILTIN, FrameSet::empty())
        .expect("conflict path returns Ok(Conflict), not Err");
    assert!(matches!(outcome, ApplyOutcome::Conflict));
    assert!(_r.get("Foo").is_none());
}

#[test]
fn try_register_type_clears_matching_placeholder() {
    let storage = FrameStorage::run_root();
    let region = storage.brand();
    let bindings: Bindings<'_> = Bindings::new();
    let kt: &KType = region.alloc_ktype(KType::Number);
    bindings
        .try_install_placeholder(
            "Bar".to_string(),
            NodeId(7),
            BindingIndex::BUILTIN,
            BindKind::Type,
        )
        .expect("placeholder install should succeed on fresh bindings");
    assert!(bindings.placeholders().contains_key("Bar"));
    bindings
        .try_register_type("Bar", kt, BindingIndex::BUILTIN, FrameSet::empty())
        .expect("type register should succeed and clear placeholder");
    assert!(!bindings.placeholders().contains_key("Bar"));
}

#[test]
fn try_register_type_does_not_touch_data_or_functions() {
    let storage = FrameStorage::run_root();
    let region = storage.brand();
    let bindings: Bindings<'_> = Bindings::new();
    let kt: &KType = region.alloc_ktype(KType::Number);
    bindings
        .try_register_type("Foo", kt, BindingIndex::BUILTIN, FrameSet::empty())
        .expect("register should succeed");
    assert!(bindings.data().is_empty());
    assert!(bindings.functions().is_empty());
}

// --- Cross-kind exclusion (AC1/AC4) -----------------------------------------
// Each declarator routes to one of these write primitives (LET-value →
// `try_bind_value`; LET-type-alias / VAL / NEWTYPE-sigil → `try_register_type`;
// MODULE / SIG / UNION / NEWTYPE-record / RECURSIVE-finalize →
// `try_register_type_upsert`; module/USING replay → `try_bulk_install_from`), so
// proving the primitive rejects a cross-kind collision proves it for every bind
// site. The reverse — a bare `FN`, which binds neither `data` nor `types` — is
// exempt; that is covered Scope-side in `core::tests::register`.

#[test]
fn value_bind_then_type_register_is_rebind() {
    let storage = FrameStorage::run_root();
    let region = storage.brand();
    let bindings: Bindings<'_> = Bindings::new();
    let val: &KObject = region.alloc_object(KObject::Number(7.0));
    let kt: &KType = region.alloc_ktype(KType::Number);
    bindings
        .try_bind_value("x", val, BindingIndex::BUILTIN, FrameSet::empty())
        .expect("value bind should succeed on fresh bindings");
    let err = match bindings.try_register_type("x", kt, BindingIndex::BUILTIN, FrameSet::empty()) {
        Err(e) => e,
        Ok(_) => panic!("registering a type over a committed value must be rejected"),
    };
    assert!(matches!(err.kind, KErrorKind::Rebind { ref name } if name == "x"));
    // The value survives untouched; nothing landed in `types`.
    assert!(bindings.types().get("x").is_none());
    assert!(bindings.data().get("x").is_some());
}

#[test]
fn value_bind_then_type_upsert_is_rebind() {
    let storage = FrameStorage::run_root();
    let region = storage.brand();
    let bindings: Bindings<'_> = Bindings::new();
    let val: &KObject = region.alloc_object(KObject::Number(7.0));
    let kt: &KType = region.alloc_ktype(KType::Number);
    bindings
        .try_bind_value("x", val, BindingIndex::BUILTIN, FrameSet::empty())
        .expect("value bind should succeed");
    let err = match bindings.try_register_type_upsert(
        "x",
        kt,
        BindingIndex::BUILTIN,
        FrameSet::empty(),
    ) {
        Err(e) => e,
        Ok(_) => panic!("upserting a type over a committed value must be rejected"),
    };
    assert!(matches!(err.kind, KErrorKind::Rebind { ref name } if name == "x"));
    assert!(bindings.types().get("x").is_none());
}

#[test]
fn type_register_then_value_bind_is_rebind() {
    let storage = FrameStorage::run_root();
    let region = storage.brand();
    let bindings: Bindings<'_> = Bindings::new();
    let kt: &KType = region.alloc_ktype(KType::Number);
    let val: &KObject = region.alloc_object(KObject::Number(7.0));
    bindings
        .try_register_type("T", kt, BindingIndex::BUILTIN, FrameSet::empty())
        .expect("type register should succeed on fresh bindings");
    let err = match bindings.try_bind_value("T", val, BindingIndex::BUILTIN, FrameSet::empty()) {
        Err(e) => e,
        Ok(_) => panic!("binding a value over a committed type must be rejected"),
    };
    assert!(matches!(err.kind, KErrorKind::Rebind { ref name } if name == "T"));
    assert!(bindings.data().get("T").is_none());
    assert!(bindings.types().get("T").is_some());
}

#[test]
fn type_upsert_then_value_bind_is_rebind() {
    let storage = FrameStorage::run_root();
    let region = storage.brand();
    let bindings: Bindings<'_> = Bindings::new();
    let kt: &KType = region.alloc_ktype(KType::Number);
    let val: &KObject = region.alloc_object(KObject::Number(7.0));
    bindings
        .try_register_type_upsert("T", kt, BindingIndex::BUILTIN, FrameSet::empty())
        .expect("type upsert should succeed");
    let err = match bindings.try_bind_value("T", val, BindingIndex::BUILTIN, FrameSet::empty()) {
        Err(e) => e,
        Ok(_) => panic!("binding a value over an upserted type must be rejected"),
    };
    assert!(matches!(err.kind, KErrorKind::Rebind { ref name } if name == "T"));
    assert!(bindings.data().get("T").is_none());
}

#[test]
fn bulk_install_rejects_value_colliding_with_committed_type() {
    let storage = FrameStorage::run_root();
    let region = storage.brand();
    // `dst` already holds `Foo` as a type; replaying a source whose `data` binds
    // `Foo` as a value must be rejected — `try_bulk_install_from` routes through
    // `try_apply` (`write_data == true`), so the cross-kind check fires.
    let dst: Bindings<'_> = Bindings::new();
    let kt: &KType = region.alloc_ktype(KType::Number);
    dst.try_register_type("Foo", kt, BindingIndex::BUILTIN, FrameSet::empty())
        .expect("type register should succeed");
    let src: Bindings<'_> = Bindings::new();
    let val: &KObject = region.alloc_object(KObject::Number(7.0));
    src.try_bind_value("Foo", val, BindingIndex::BUILTIN, FrameSet::empty())
        .expect("source value bind should succeed");
    let err = dst
        .try_bulk_install_from(&src)
        .expect_err("bulk-installing a value over a committed type must be rejected");
    assert!(matches!(err.kind, KErrorKind::Rebind { ref name } if name == "Foo"));
    assert!(dst.data().get("Foo").is_none());
}

#[test]
fn new_bindings_has_empty_pending_types() {
    let bindings: Bindings<'_> = Bindings::new();
    assert!(bindings.pending_types().is_empty());
}

/// Dropping the value returned by `insert_pending_type` is the sole removal path
/// for a `pending_types` entry outside `#[cfg(test)]`.
#[test]
fn pending_binder_guard_drop_removes_entry() {
    use crate::machine::model::ast::KExpression;
    let bindings: Box<Bindings<'static>> = Box::default();
    let bindings: &'static Bindings<'static> = Box::leak(bindings);
    let entry = PendingTypeEntry {
        kind: KKind::NewType,
        scope_id: ScopeId::from_raw(0, 0xBEEF),
        schema_expr: KExpression::new(Vec::new()),
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
        kind: KKind::NewType,
        scope_id: ScopeId::from_raw(0, 0xBEEF),
        schema_expr: KExpression::new(Vec::new()),
    };
    let guard = bindings.insert_pending_type("Foo".into(), entry);
    bindings.pending_remove("Foo");
    drop(guard);
    assert!(!bindings.pending_types().contains_key("Foo"));
}
