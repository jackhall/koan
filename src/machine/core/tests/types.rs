//! `register_type` / `resolve_type` tests: type bindings land in `types` (not `data`),
//! `resolve_type` walks the outer chain, and inner scopes shadow outer type bindings.

use super::super::Scope;
use crate::builtins::test_support::run_root_bare;
use crate::machine::core::FrameSet;
use crate::machine::core::FrameStorage;
use crate::machine::model::types::KType;
use crate::machine::BindingIndex;

#[test]
fn register_type_inserts_into_types_map_not_data() {
    let region = FrameStorage::run_root();
    let scope = run_root_bare(&region);
    scope.register_type(
        "Foo".into(),
        KType::Number,
        BindingIndex::BUILTIN,
        FrameSet::empty(),
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
        FrameSet::empty(),
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
        FrameSet::empty(),
    );
    let child = region.brand().alloc_scope(Scope::child_under(root));
    child.register_type(
        "Foo".into(),
        KType::Str,
        BindingIndex::value(1),
        FrameSet::empty(),
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
    let cell = Sealed::seal(producer.resident_value_carrier(obj, &FrameSet::empty()));

    // A separate (open) consumer scope adopts the carrier.
    let consumer = producer.brand().alloc_scope(Scope::child_under(producer));
    let adopted: Carried = consumer.adopt_sealed(&cell);

    // Copy-free: the adopted borrow points at the very same object, not a relocated clone.
    assert!(std::ptr::eq(adopted.object(), obj));
}
