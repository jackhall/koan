//! `queue` arm of `machine::core` tests.

use crate::builtins::test_support::run_root_bare;
use crate::machine::core::kfunction::{Body, KFunction};
use crate::machine::core::StoredReach;
use crate::machine::core::{run_root_storage, FrameStorageExt};
use crate::machine::model::types::KType;
use crate::machine::model::values::KObject;
use crate::machine::BindingIndex;

use super::{body_no_op, unit_signature};

/// A re-entrant `bind_value` under a live `data` borrow queues silently; the held
/// iteration sees the pre-write state and the write surfaces only after `drain_pending`.
#[test]
fn add_during_active_data_borrow_queues_and_drains() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let pre = region.brand().alloc_object(KObject::Number(1.0));
    scope
        .bind_value(
            "pre".to_string(),
            pre,
            BindingIndex::BUILTIN,
            StoredReach::empty(),
        )
        .unwrap();

    let new_entry = region.brand().alloc_object(KObject::Number(2.0));
    {
        let snapshot = scope.bindings().data();
        assert!(snapshot.contains_key("pre"));
        scope
            .bind_value(
                "during".to_string(),
                new_entry,
                BindingIndex::BUILTIN,
                StoredReach::empty(),
            )
            .unwrap();
        assert!(!snapshot.contains_key("during"));
    }
    assert!(scope.bindings().data().get("during").is_none());
    scope.drain_pending();
    let after = scope.bindings().data();
    assert!(
        matches!(after.get("during").map(|(o, _, _)| *o), Some(KObject::Number(n)) if *n == 2.0)
    );
}

/// `PendingQueue::drain`'s `debug_assert!` must fire when a deferred `Value` write
/// surfaces a semantic `Err` on retry — here, a same-signature function seeded into
/// the bucket between defer and drain that the retry resolves to `DuplicateOverload`.
#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "PendingQueue::drain")]
fn drain_debug_asserts_on_invariant_violation() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let kfn1 = region.brand().alloc_function(KFunction::new(
        unit_signature(),
        Body::Builtin(body_no_op),
        scope,
        None,
        None,
        false,
    ));
    let obj1 = region.brand().alloc_object(KObject::KFunction(kfn1));
    let kfn2 = region.brand().alloc_function(KFunction::new(
        unit_signature(),
        Body::Builtin(body_no_op),
        scope,
        None,
        None,
        false,
    ));
    let obj2 = region.brand().alloc_object(KObject::KFunction(kfn2));

    let snapshot = scope.bindings().data();
    scope
        .bind_value(
            "a".to_string(),
            obj1,
            BindingIndex::BUILTIN,
            StoredReach::empty(),
        )
        .unwrap();
    drop(snapshot);
    scope
        .register_function("b".to_string(), kfn2, obj2, BindingIndex::BUILTIN)
        .unwrap();
    scope.drain_pending();
}

/// FN-side mirror of `add_during_active_data_borrow_queues_and_drains`: bare
/// `register_function` under a live `functions` borrow defers and replays through the
/// `Function` arm, landing in `functions` only (no `data` mirror).
#[test]
fn register_function_defers_and_drains_through_function_arm() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let kfn = region.brand().alloc_function(KFunction::new(
        unit_signature(),
        Body::Builtin(body_no_op),
        scope,
        None,
        None,
        false,
    ));
    let obj = region.brand().alloc_object(KObject::KFunction(kfn));
    let key = kfn.signature.untyped_key();
    {
        let snapshot = scope.bindings().functions();
        scope
            .register_function("g".to_string(), kfn, obj, BindingIndex::BUILTIN)
            .unwrap();
        assert!(snapshot.get(&key).map(|b| b.is_empty()).unwrap_or(true));
    }
    scope.drain_pending();
    assert!(scope.bindings().data().get("g").is_none());
    let funcs = scope.bindings().functions();
    assert!(funcs.get(&key).map(|b| !b.is_empty()).unwrap_or(false));
}

/// `Value`-arm `Conflict` re-queue: if the outer `data` borrow stays live across
/// the first `drain_pending`, the retry hits the same `try_borrow_mut` failure and
/// extends back onto `still_pending`; once the borrow drops the next drain applies.
#[test]
fn drain_requeues_value_on_persistent_borrow_conflict() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let obj = region.brand().alloc_object(KObject::Number(7.0));

    let snapshot = scope.bindings().data();
    scope
        .bind_value(
            "v".to_string(),
            obj,
            BindingIndex::BUILTIN,
            StoredReach::empty(),
        )
        .unwrap();
    scope.drain_pending();
    assert!(!snapshot.contains_key("v"));
    drop(snapshot);
    scope.drain_pending();
    assert!(
        matches!(scope.bindings().data().get("v").map(|(o, _, _)| *o), Some(KObject::Number(n)) if *n == 7.0)
    );
}

/// `Function`-arm `Conflict` re-queue — same shape as the `Value` variant, but the
/// deferred write is a `register_function`, contending on `functions` rather than `data`.
#[test]
fn drain_requeues_function_on_persistent_borrow_conflict() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let kfn = region.brand().alloc_function(KFunction::new(
        unit_signature(),
        Body::Builtin(body_no_op),
        scope,
        None,
        None,
        false,
    ));
    let obj = region.brand().alloc_object(KObject::KFunction(kfn));
    let key = kfn.signature.untyped_key();

    let snapshot = scope.bindings().functions();
    scope
        .register_function("g".to_string(), kfn, obj, BindingIndex::BUILTIN)
        .unwrap();
    scope.drain_pending();
    assert!(snapshot.get(&key).map(|b| b.is_empty()).unwrap_or(true));
    drop(snapshot);
    scope.drain_pending();
    let funcs = scope.bindings().functions();
    assert!(funcs.get(&key).map(|b| !b.is_empty()).unwrap_or(false));
}

/// `Type`-arm `Conflict` re-queue. The defer is induced by a live `types`
/// read borrow; `try_register_type` only contends on `types`.
#[test]
fn drain_requeues_type_on_persistent_borrow_conflict() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);

    let snapshot = scope.bindings().types();
    scope.register_type(
        "Foo".to_string(),
        KType::Number,
        BindingIndex::BUILTIN,
        StoredReach::empty(),
    );
    scope.drain_pending();
    assert!(!snapshot.contains_key("Foo"));
    drop(snapshot);
    scope.drain_pending();
    assert!(scope.bindings().types().contains_key("Foo"));
}

/// `Function`-arm `Err` debug-assert: deferred `Function` retry finds a
/// pointer-distinct same-signature entry seeded between defer and drain and
/// surfaces `DuplicateOverload`.
#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "PendingQueue::drain")]
fn drain_debug_asserts_on_function_arm_invariant_violation() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let kfn1 = region.brand().alloc_function(KFunction::new(
        unit_signature(),
        Body::Builtin(body_no_op),
        scope,
        None,
        None,
        false,
    ));
    let obj1 = region.brand().alloc_object(KObject::KFunction(kfn1));
    let kfn2 = region.brand().alloc_function(KFunction::new(
        unit_signature(),
        Body::Builtin(body_no_op),
        scope,
        None,
        None,
        false,
    ));
    let obj2 = region.brand().alloc_object(KObject::KFunction(kfn2));

    let snapshot = scope.bindings().functions();
    scope
        .register_function("a".to_string(), kfn1, obj1, BindingIndex::BUILTIN)
        .unwrap();
    drop(snapshot);
    scope
        .register_function("b".to_string(), kfn2, obj2, BindingIndex::BUILTIN)
        .unwrap();
    scope.drain_pending();
}

/// `Type`-arm `Err` debug-assert: a direct `register_type` between defer and drain
/// seeds a different `KType` under the same name, so the retry sees `types[name]`
/// populated and surfaces `Rebind`.
#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "PendingQueue::drain")]
fn drain_debug_asserts_on_type_arm_invariant_violation() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let snapshot = scope.bindings().types();
    scope.register_type(
        "Foo".to_string(),
        KType::Number,
        BindingIndex::BUILTIN,
        StoredReach::empty(),
    );
    drop(snapshot);
    scope.register_type(
        "Foo".to_string(),
        KType::Str,
        BindingIndex::BUILTIN,
        StoredReach::empty(),
    );
    scope.drain_pending();
}
