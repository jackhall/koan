//! `queue` arm of `machine::core` tests.

use super::super::RuntimeArena;
use crate::builtins::test_support::run_root_bare;
use crate::machine::core::kfunction::{Body, KFunction};
use crate::machine::model::types::KType;
use crate::machine::model::values::KObject;

use super::{unit_signature, body_no_op};

/// Snapshot-iteration semantics: a re-entrant `bind_value` queues silently and only
/// becomes visible after `drain_pending`; the held iteration sees the pre-write state.
#[test]
fn add_during_active_data_borrow_queues_and_drains() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let pre = arena.alloc_object(KObject::Number(1.0));
    scope.bind_value("pre".to_string(), pre).unwrap();

    let new_entry = arena.alloc_object(KObject::Number(2.0));
    {
        let snapshot = scope.bindings().data();
        assert!(snapshot.contains_key("pre"));
        scope.bind_value("during".to_string(), new_entry).unwrap();
        assert!(!snapshot.contains_key("during"));
    }
    assert!(scope.bindings().data().get("during").is_none());
    scope.drain_pending();
    let after = scope.bindings().data();
    assert!(matches!(after.get("during"), Some(KObject::Number(n)) if *n == 2.0));
}

/// Companion to the queues-and-drains test above: the `debug_assert!` inside
/// `PendingQueue::drain` must fire when a deferred write surfaces a semantic `Err` on
/// retry. Sequence:
/// 1. Open a `data` borrow → forces step 2 to defer.
/// 2. `bind_value("a", obj1)` where `obj1` wraps `kfn1` → deferred.
/// 3. Drop the borrow.
/// 4. `register_function("b", kfn2, obj2)` where `kfn2` is pointer-distinct from
///    `kfn1` but has the same untyped signature → succeeds, seeds `kfn2` into the
///    bucket.
/// 5. `drain_pending()` retries step 2's deferred write. `try_apply` walks the bucket,
///    finds `kfn2` (pointer-distinct, structurally equal signature) → returns
///    `DuplicateOverload`. The `debug_assert!` fires.
#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "PendingQueue::drain")]
fn drain_debug_asserts_on_invariant_violation() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let kfn1 = arena.alloc_function(KFunction::new(unit_signature(), Body::Builtin(body_no_op), scope));
    let obj1 = arena.alloc_object(KObject::KFunction(kfn1, None));
    let kfn2 = arena.alloc_function(KFunction::new(unit_signature(), Body::Builtin(body_no_op), scope));
    let obj2 = arena.alloc_object(KObject::KFunction(kfn2, None));

    // 1. Hold an outer `data` borrow open so the bind in step 2 must defer.
    let snapshot = scope.bindings().data();
    // 2. Defers — borrow contention on `data`.
    scope.bind_value("a".to_string(), obj1).unwrap();
    // 3. Release the outer borrow so step 4's direct write can proceed.
    drop(snapshot);
    // 4. Succeeds and seeds `kfn2` into the functions bucket under the shared
    //    untyped signature.
    scope.register_function("b".to_string(), kfn2, obj2).unwrap();
    // 5. Retries the deferred `bind_value`. Bucket walk finds `kfn2` with a
    //    structurally-equal signature → `DuplicateOverload` → `debug_assert!` fires.
    scope.drain_pending();
}

/// Bare `FN` registration under a live `functions` borrow defers via `defer_function`;
/// drain replays through the `Function` arm and lands the binding in the per-signature
/// `functions` bucket only (no `data` mirror). Pins the FN-side mirror of
/// `add_during_active_data_borrow_queues_and_drains`, which only exercises the
/// `Value` arm — without this the `defer_function` constructor and the
/// `Function` arm's `Applied` branch never run in the suite.
#[test]
fn register_function_defers_and_drains_through_function_arm() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let kfn = arena.alloc_function(KFunction::new(unit_signature(), Body::Builtin(body_no_op), scope));
    let obj = arena.alloc_object(KObject::KFunction(kfn, None));
    let key = kfn.signature.untyped_key();
    {
        // Bare FN registration contends on `functions` (not `data`), so hold a live
        // `functions` borrow to force the defer.
        let snapshot = scope.bindings().functions();
        scope.register_function("g".to_string(), kfn, obj).unwrap();
        assert!(snapshot.get(&key).map(|b| b.is_empty()).unwrap_or(true));
    }
    scope.drain_pending();
    // Bare FN lands only in `functions`; nothing mirrors into `data`.
    assert!(scope.bindings().data().get("g").is_none());
    let funcs = scope.bindings().functions();
    assert!(funcs.get(&key).map(|b| !b.is_empty()).unwrap_or(false));
}

/// `Value`-arm `Conflict` re-queue: when the outer `data` borrow stays live
/// across the first `drain_pending`, the retry hits the same `try_borrow_mut`
/// failure → `Ok(Conflict)` → push onto `still_pending` → tail extends back
/// into the queue. Once the borrow drops the next drain applies. Pins the
/// `Value`-arm `Conflict` branch and the `still_pending` extend tail.
#[test]
fn drain_requeues_value_on_persistent_borrow_conflict() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let obj = arena.alloc_object(KObject::Number(7.0));

    let snapshot = scope.bindings().data();
    scope.bind_value("v".to_string(), obj).unwrap();
    scope.drain_pending();
    assert!(!snapshot.contains_key("v"));
    drop(snapshot);
    scope.drain_pending();
    assert!(matches!(scope.bindings().data().get("v"), Some(KObject::Number(n)) if *n == 7.0));
}

/// `Function`-arm `Conflict` re-queue — same shape as the `Value` variant, but the
/// deferred write is a bare `register_function`, which contends on `functions` rather
/// than `data`.
#[test]
fn drain_requeues_function_on_persistent_borrow_conflict() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let kfn = arena.alloc_function(KFunction::new(unit_signature(), Body::Builtin(body_no_op), scope));
    let obj = arena.alloc_object(KObject::KFunction(kfn, None));
    let key = kfn.signature.untyped_key();

    let snapshot = scope.bindings().functions();
    scope.register_function("g".to_string(), kfn, obj).unwrap();
    scope.drain_pending();
    assert!(snapshot.get(&key).map(|b| b.is_empty()).unwrap_or(true));
    drop(snapshot);
    scope.drain_pending();
    let funcs = scope.bindings().functions();
    assert!(funcs.get(&key).map(|b| !b.is_empty()).unwrap_or(false));
}

/// `Type`-arm `Conflict` re-queue. The defer is induced by a live `types`
/// read borrow rather than a `data` borrow (Type's retry calls
/// `try_register_type`, which only contends on `types`).
#[test]
fn drain_requeues_type_on_persistent_borrow_conflict() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);

    let snapshot = scope.bindings().types();
    scope.register_type("Foo".to_string(), KType::Number);
    scope.drain_pending();
    assert!(!snapshot.contains_key("Foo"));
    drop(snapshot);
    scope.drain_pending();
    assert!(scope.bindings().types().contains_key("Foo"));
}

/// `Function`-arm `Err` debug-assert. Companion to
/// `drain_debug_asserts_on_invariant_violation` (which deferred a `Value`):
/// here the deferred write itself is a `Function` whose retry walks the
/// bucket, finds a pointer-distinct same-signature function seeded between
/// defer and drain, and surfaces `DuplicateOverload`.
#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "PendingQueue::drain")]
fn drain_debug_asserts_on_function_arm_invariant_violation() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let kfn1 = arena.alloc_function(KFunction::new(unit_signature(), Body::Builtin(body_no_op), scope));
    let obj1 = arena.alloc_object(KObject::KFunction(kfn1, None));
    let kfn2 = arena.alloc_function(KFunction::new(unit_signature(), Body::Builtin(body_no_op), scope));
    let obj2 = arena.alloc_object(KObject::KFunction(kfn2, None));

    // Bare FN registration contends on `functions`, so hold a `functions` borrow to
    // force step "a" to defer.
    let snapshot = scope.bindings().functions();
    scope.register_function("a".to_string(), kfn1, obj1).unwrap();
    drop(snapshot);
    scope.register_function("b".to_string(), kfn2, obj2).unwrap();
    scope.drain_pending();
}

/// `Type`-arm `Err` debug-assert. A direct `register_type` between
/// `defer_type` and drain seeds a different `KType` under the same name; the
/// retry sees `types[name]` populated → `Err(Rebind)` → debug-assert fires.
#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "PendingQueue::drain")]
fn drain_debug_asserts_on_type_arm_invariant_violation() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let snapshot = scope.bindings().types();
    scope.register_type("Foo".to_string(), KType::Number);
    drop(snapshot);
    scope.register_type("Foo".to_string(), KType::Str);
    scope.drain_pending();
}
