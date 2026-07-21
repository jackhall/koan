//! Unit coverage for the `types` map write primitive `try_register_type`, plus
//! the `pending_types` RAII guard lifecycle and the cross-kind exclusion that
//! makes the `data`/`types` partition structural (no name in both).

use super::*;
use crate::machine::core::arena::{run_root_storage, FrameStorageExt};
use crate::machine::model::KObject;
use crate::machine::model::KType;

/// A value binding round-trips the home-omitted foreign reach it was bound with: a carrier-oriented
/// read hands back exactly the `FrameSet` stored at bind time, so the read wrapper can name the
/// value's reach without reconstructing it from the value.
#[test]
fn data_binding_round_trips_stored_reach() {
    let storage = run_root_storage();
    let region = storage.brand();
    let bindings: Bindings<'_> = Bindings::new();
    let obj: &KObject = region.alloc_object(KObject::Number(1.0));
    // A synthetic foreign frame the value "reaches" — stored on the binding as its reach.
    let foreign = run_root_storage();
    let reach_set = FrameSet::singleton(foreign.clone());
    let reach = StoredReach::for_test(Some(&reach_set), false);
    bindings
        .try_bind_value("x", obj, BindingIndex::BUILTIN, reach)
        .expect("value bind should succeed");
    match bindings.lookup_value_carrier("x", None) {
        Some(NameLookup::Bound(hit)) => {
            assert!(std::ptr::eq(hit.obj, obj));
            assert!(
                hit.stored.foreign.is_some_and(
                    |f| matches!(f.members(), [only] if std::rc::Rc::ptr_eq(only, &foreign))
                ),
                "stored reach should round-trip the foreign frame",
            );
        }
        _ => panic!("expected a bound value carrier hit"),
    }
}

/// A carrier-oriented read copies the stored `Option<&FrameSet>` reference — no per-hit clone. Two
/// independent reads of the same binding hand back the *same* `&FrameSet` pointer, proving the read
/// path reuses the arena-hosted set rather than cloning a fresh one on every hit (the type-binding
/// memo relies on the same no-clone copy).
#[test]
fn value_binding_carrier_read_copies_the_reach_pointer_not_a_clone() {
    let storage = run_root_storage();
    let region = storage.brand();
    let bindings: Bindings<'_> = Bindings::new();
    let obj: &KObject = region.alloc_object(KObject::Number(1.0));
    let foreign = run_root_storage();
    let reach_set = FrameSet::singleton(foreign.clone());
    let reach = StoredReach::for_test(Some(&reach_set), false);
    bindings
        .try_bind_value("x", obj, BindingIndex::BUILTIN, reach)
        .expect("value bind should succeed");

    let first = match bindings.lookup_value_carrier("x", None) {
        Some(NameLookup::Bound(hit)) => hit.stored.foreign.expect("non-empty reach"),
        _ => panic!("expected a bound value carrier hit"),
    };
    let second = match bindings.lookup_value_carrier("x", None) {
        Some(NameLookup::Bound(hit)) => hit.stored.foreign.expect("non-empty reach"),
        _ => panic!("expected a bound value carrier hit"),
    };
    assert!(
        std::ptr::eq(first, second),
        "two reads of the same binding must return the same &FrameSet — a clone would allocate a \
         fresh Vec at a distinct address on every hit",
    );
    assert!(
        std::ptr::eq(first, &reach_set),
        "the stored reach is the exact reference bound in, not a copy of it",
    );
}

#[test]
fn try_register_type_inserts_into_types_map() {
    let bindings: Bindings<'_> = Bindings::new();
    let kt: KType = KType::NUMBER;
    let outcome = bindings
        .try_register_type("Foo", kt, BindingIndex::BUILTIN)
        .expect("try_register_type should succeed on fresh bindings");
    assert!(matches!(outcome, ApplyOutcome::Applied));
    let stored = bindings
        .types()
        .get("Foo")
        .expect("Foo should be in types map")
        .0;
    assert_eq!(stored, kt);
    assert!(bindings.data().get("Foo").is_none());
}

#[test]
fn try_register_type_rejects_collision_with_rebind() {
    let bindings: Bindings<'_> = Bindings::new();
    let kt1: KType = KType::NUMBER;
    let kt2: KType = KType::STR;
    bindings
        .try_register_type("Foo", kt1, BindingIndex::BUILTIN)
        .expect("first register should succeed");
    let err = match bindings.try_register_type("Foo", kt2, BindingIndex::BUILTIN) {
        Err(e) => e,
        Ok(_) => panic!("second register on same name should error, not succeed"),
    };
    assert!(matches!(err.kind, KErrorKind::Rebind { ref name } if name == "Foo"));
    let stored = bindings
        .types()
        .get("Foo")
        .expect("Foo should still be present")
        .0;
    assert_eq!(stored, kt1);
}

#[test]
fn try_register_type_yields_conflict_on_live_types_borrow() {
    let bindings: Bindings<'_> = Bindings::new();
    let kt: KType = KType::NUMBER;
    let _r = bindings.types();
    let outcome = bindings
        .try_register_type("Foo", kt, BindingIndex::BUILTIN)
        .expect("conflict path returns Ok(Conflict), not Err");
    assert!(matches!(outcome, ApplyOutcome::Conflict));
    assert!(_r.get("Foo").is_none());
}

#[test]
fn try_register_type_clears_matching_placeholder() {
    let bindings: Bindings<'_> = Bindings::new();
    let kt: KType = KType::NUMBER;
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
        .try_register_type("Bar", kt, BindingIndex::BUILTIN)
        .expect("type register should succeed and clear placeholder");
    assert!(!bindings.placeholders().contains_key("Bar"));
}

#[test]
fn try_register_type_does_not_touch_data_or_functions() {
    let bindings: Bindings<'_> = Bindings::new();
    let kt: KType = KType::NUMBER;
    bindings
        .try_register_type("Foo", kt, BindingIndex::BUILTIN)
        .expect("register should succeed");
    assert!(bindings.data().is_empty());
    assert!(bindings.functions().is_empty());
}

// --- Cross-kind exclusion (AC1/AC4) -----------------------------------------
// Each declarator routes to one of these write primitives (LET-value →
// `try_bind_value`; LET-type-alias / VAL / NEWTYPE-sigil → `try_register_type`;
// MODULE / SIG / UNION / NEWTYPE-record / RECURSIVE-finalize →
// `try_register_type_upsert`; module/USING replay → `try_bulk_install_from`).
// `partition_guard` is the single enforcement point every one of these primitives calls, so
// `value_token_may_not_bind_type_side` / `type_token_may_not_bind_value_side` below — exercised
// against a plain `Bindings::new()` — prove the exclusion for every bind site: a name's token
// class fixes which map it may ever enter, so the same name can never land in both. The reverse —
// a bare `FN`, which binds neither `data` nor `types` — is exempt; that is covered Scope-side in
// `core::tests::register`.

#[test]
fn new_bindings_has_empty_pending_types() {
    let bindings: Bindings<'_> = Bindings::new();
    assert!(bindings.pending_types().is_empty());
}

/// Dropping the value returned by `insert_pending_type` is the sole removal path
/// for a `pending_types` entry outside `#[cfg(test)]`.
#[test]
fn pending_binder_guard_drop_removes_entry() {
    let bindings: Box<Bindings<'static>> = Box::default();
    let bindings: &'static Bindings<'static> = Box::leak(bindings);
    {
        let _guard = bindings.insert_pending_type("Foo".into());
        assert!(bindings.pending_types().contains("Foo"));
    }
    assert!(
        !bindings.pending_types().contains("Foo"),
        "guard Drop should have removed the pending_types entry",
    );
}

/// Guard Drop must tolerate an already-removed entry without panicking.
#[test]
fn pending_binder_guard_drop_tolerates_absent_entry() {
    let bindings: Box<Bindings<'static>> = Box::default();
    let bindings: &'static Bindings<'static> = Box::leak(bindings);
    let guard = bindings.insert_pending_type("Foo".into());
    bindings.pending_remove("Foo");
    drop(guard);
    assert!(!bindings.pending_types().contains("Foo"));
}

/// The token-class partition: `types` and `data` are different universes, and a name's token class
/// decides which one it belongs to. A value token may not name a type…
#[test]
fn value_token_may_not_bind_type_side() {
    let bindings: Bindings<'_> = Bindings::new();
    let kt: KType = KType::NUMBER;
    let error = match bindings.try_register_type("int_ord", kt, BindingIndex::BUILTIN) {
        Err(e) => e,
        Ok(_) => panic!("a value token names a value, not a type"),
    };
    assert!(
        matches!(&error.kind, KErrorKind::ShapeError(msg) if msg.contains("is a value token")),
        "expected the token-class partition error, got {error}",
    );
    assert!(bindings.types().get("int_ord").is_none());
}

/// …and a Type token may not name a value. Together these commit every name to exactly one
/// universe: the partition admits no exception, so a cross-kind collision — the same name
/// landing in both maps — is unconstructible.
#[test]
fn type_token_may_not_bind_value_side() {
    let storage = run_root_storage();
    let region = storage.brand();
    let bindings: Bindings<'_> = Bindings::new();
    let val: &KObject = region.alloc_object(KObject::Number(7.0));
    let error = match bindings.try_bind_value(
        "IntOrd",
        val,
        BindingIndex::BUILTIN,
        StoredReach::for_test(None, false),
    ) {
        Err(e) => e,
        Ok(_) => panic!("a Type token names a type, not a value"),
    };
    assert!(
        matches!(&error.kind, KErrorKind::ShapeError(msg) if msg.contains("is a Type token")),
        "expected the token-class partition error, got {error}",
    );
    assert!(bindings.data().get("IntOrd").is_none());
}
