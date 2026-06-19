//! Unit coverage for the `types` map write primitive `try_register_type`, plus
//! the `pending_types` RAII guard lifecycle.

use super::*;
use crate::machine::core::arena::RuntimeArena;
use crate::machine::core::scope_id::ScopeId;
use crate::machine::model::types::{KKind, KType};

#[test]
fn try_register_type_inserts_into_types_map() {
    let arena = RuntimeArena::new();
    let bindings: Bindings<'_> = Bindings::new();
    let kt: &KType = arena.alloc_ktype(KType::Number);
    let outcome = bindings
        .try_register_type("Foo", kt, BindingIndex::BUILTIN)
        .expect("try_register_type should succeed on fresh bindings");
    assert!(matches!(outcome, ApplyOutcome::Applied));
    let (stored, _) = *bindings
        .types()
        .get("Foo")
        .expect("Foo should be in types map");
    assert!(std::ptr::eq(stored, kt));
    assert!(bindings.data().get("Foo").is_none());
}

#[test]
fn try_register_type_rejects_collision_with_rebind() {
    let arena = RuntimeArena::new();
    let bindings: Bindings<'_> = Bindings::new();
    let kt1: &KType = arena.alloc_ktype(KType::Number);
    let kt2: &KType = arena.alloc_ktype(KType::Str);
    bindings
        .try_register_type("Foo", kt1, BindingIndex::BUILTIN)
        .expect("first register should succeed");
    let err = match bindings.try_register_type("Foo", kt2, BindingIndex::BUILTIN) {
        Err(e) => e,
        Ok(_) => panic!("second register on same name should error, not succeed"),
    };
    assert!(matches!(err.kind, KErrorKind::Rebind { ref name } if name == "Foo"));
    let (stored, _) = *bindings
        .types()
        .get("Foo")
        .expect("Foo should still be present");
    assert!(std::ptr::eq(stored, kt1));
}

#[test]
fn try_register_type_yields_conflict_on_live_types_borrow() {
    let arena = RuntimeArena::new();
    let bindings: Bindings<'_> = Bindings::new();
    let kt: &KType = arena.alloc_ktype(KType::Number);
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
    let kt: &KType = arena.alloc_ktype(KType::Number);
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
    let kt: &KType = arena.alloc_ktype(KType::Number);
    bindings
        .try_register_type("Foo", kt, BindingIndex::BUILTIN)
        .expect("register should succeed");
    assert!(bindings.data().is_empty());
    assert!(bindings.functions().is_empty());
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
