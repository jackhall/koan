//! `register_type` / `resolve_type` tests: type bindings land in `types` (not `data`),
//! `resolve_type` walks the outer chain, and inner scopes shadow outer type bindings.

use super::super::Scope;
use crate::builtins::test_support::run_root_bare;
use crate::machine::core::FrameStorage;
use crate::machine::core::StoredReach;
use crate::machine::model::types::KType;
use crate::machine::BindingIndex;
use crate::machine::CarrierWitness;

#[test]
fn register_type_inserts_into_types_map_not_data() {
    let region = FrameStorage::run_root();
    let scope = run_root_bare(&region);
    scope.register_type(
        "Foo".into(),
        KType::Number,
        BindingIndex::BUILTIN,
        StoredReach::empty(),
    );
    assert!(scope.bindings().types().get("Foo").is_some());
    assert!(
        scope.bindings().data().get("Foo").is_none(),
        "type binding must not appear in data map",
    );
}

#[test]
fn resolve_type_walks_outer_chain_and_returns_none_past_root() {
    let region = FrameStorage::run_root();
    let root = run_root_bare(&region);
    root.register_type(
        "Foo".into(),
        KType::Number,
        BindingIndex::BUILTIN,
        StoredReach::empty(),
    );
    let child = region.brand().alloc_scope(Scope::child_under(root));
    assert!(matches!(child.resolve_type("Foo"), Some(KType::Number)));
    assert!(
        child.resolve_type("Nope").is_none(),
        "unbound name past run-root yields None, not panic",
    );
}

#[test]
fn resolve_type_inner_scope_shadows_outer() {
    let region = FrameStorage::run_root();
    let root = run_root_bare(&region);
    // User (non-BUILTIN) types: a builtin is unshadowable and would resolve root-first,
    // so this exercises the user-vs-user innermost-wins walk.
    root.register_type(
        "Foo".into(),
        KType::Number,
        BindingIndex::value(1),
        StoredReach::empty(),
    );
    let child = region.brand().alloc_scope(Scope::child_under(root));
    child.register_type(
        "Foo".into(),
        KType::Str,
        BindingIndex::value(1),
        StoredReach::empty(),
    );
    assert!(matches!(child.resolve_type("Foo"), Some(KType::Str)));
    assert!(matches!(root.resolve_type("Foo"), Some(KType::Number)));
}

/// `adopt_sealed` re-anchors a producer's sealed carrier at the consumer scope's brand **without
/// copying**: the adopted borrow points at the very same object the producer sealed, and the
/// consumer's fold pins the reached region for the value's new lifetime.
#[test]
fn adopt_sealed_reanchors_the_same_value_copy_free() {
    use crate::machine::model::values::{Carried, KObject};
    use crate::witnessed::Sealed;

    let storage = FrameStorage::run_root();
    let producer = run_root_bare(&storage);
    // A value resident in the producer scope's region, sealed as its own carrier.
    let obj: &KObject = producer.brand().alloc_object(KObject::Number(42.0));
    let cell = Sealed::seal(producer.resident_value_carrier(obj, None, false));

    // A separate (open) consumer scope adopts the carrier.
    let consumer = producer.brand().alloc_scope(Scope::child_under(producer));
    let adopted: Carried = consumer.adopt_sealed(&cell);

    // Copy-free: the adopted borrow points at the very same object, not a relocated clone.
    assert!(std::ptr::eq(adopted.object(), obj));
}

/// Miri pin shape for `adopt_sealed`'s reattach: a value produced in a **foreign** frame's region,
/// sealed as its carrier, is adopted into a consumer scope in a different frame. After every direct
/// producer handle is dropped, the consumer scope's reach-set (folded by `adopt_sealed`) is the sole
/// pin on the producer region the re-anchored borrow reads — so reading it must not dangle.
#[test]
fn adopt_sealed_reach_fold_pins_the_producer_region_after_drop() {
    use crate::machine::core::arena::KoanRegionExt;
    use crate::machine::core::KoanRegion;
    use crate::machine::model::values::{Carried, CarriedFamily, KObject};
    use crate::witnessed::Sealed;
    use std::rc::Rc;

    // A value in the producer frame's own region, sealed witnessed by that frame.
    let producer_frame = FrameStorage::run_root();
    let cell: Sealed<CarriedFamily, CarrierWitness> = Sealed::seal(KoanRegion::alloc_witnessed(
        Rc::clone(&producer_frame),
        |r| Carried::Object(r.alloc_object(KObject::Number(9.0))),
    ));

    // A consumer scope in a *different* frame adopts the carrier — its reach-set folds the producer.
    let consumer_frame = FrameStorage::run_root();
    let consumer = run_root_bare(&consumer_frame);
    let adopted: Carried = consumer.adopt_sealed(&cell);

    // Drop every direct producer handle: the consumer scope's reach-set now solely pins the region
    // the adopted borrow reads into.
    drop(cell);
    drop(producer_frame);

    // Read the adopted value after the producer handles are gone — Miri confirms no use-after-free.
    match adopted {
        Carried::Object(KObject::Number(n)) => assert_eq!(*n, 9.0),
        _ => panic!("expected the adopted Number value"),
    }
}
