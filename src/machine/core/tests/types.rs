//! `register_type` / `resolve_type` tests: type bindings land in `types` (not `data`),
//! `resolve_type` walks the outer chain, and inner scopes shadow outer type bindings.

use std::rc::Rc;

use super::super::Scope;
use crate::builtins::test_support::{mock_declaration_site, run_root_bare};
use crate::machine::core::{run_root_storage, FramePins, FrameStorageExt};
use crate::machine::model::Carried;
use crate::machine::model::KType;
use crate::machine::{BindingIndex, DeclarationSite};

#[test]
fn register_type_inserts_into_types_map_not_data() {
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    scope.register_type("Foo".into(), KType::NUMBER, DeclarationSite::BUILTIN);
    assert!(scope.bindings().types().get("Foo").is_some());
    assert!(
        scope.bindings().data().get("Foo").is_none(),
        "type binding must not appear in data map",
    );
}

#[test]
fn resolve_type_walks_outer_chain_and_returns_none_past_root() {
    let region = run_root_storage();
    let root = run_root_bare(&region);
    root.register_type("Foo".into(), KType::NUMBER, DeclarationSite::BUILTIN);
    let child = region.brand().alloc_scope(Scope::child_under(root));
    assert!(matches!(child.resolve_type("Foo"), Some(kt) if kt == KType::NUMBER));
    assert!(
        child.resolve_type("Nope").is_none(),
        "unbound name past run-root yields None, not panic",
    );
}

#[test]
fn resolve_type_inner_scope_shadows_outer() {
    let region = run_root_storage();
    let root = run_root_bare(&region);
    // User (non-BUILTIN) types: a builtin is unshadowable and would resolve root-first,
    // so this exercises the user-vs-user innermost-wins walk.
    root.register_type("Foo".into(), KType::NUMBER, mock_declaration_site(1, 1));
    let child = region.brand().alloc_scope(Scope::child_under(root));
    child.register_type("Foo".into(), KType::STR, mock_declaration_site(2, 1));
    assert!(matches!(child.resolve_type("Foo"), Some(kt) if kt == KType::STR));
    assert!(matches!(root.resolve_type("Foo"), Some(kt) if kt == KType::NUMBER));
}

/// `adopt_sealed` re-anchors a producer's sealed carrier at the consumer scope's brand **without
/// copying**: the adopted borrow points at the very same object the producer sealed, and the
/// consumer's fold pins the reached region for the value's new lifetime.
#[test]
fn adopt_sealed_reanchors_the_same_value_copy_free() {
    use crate::machine::model::{Carried, KObject};
    use crate::witnessed::Delivered;

    let storage = run_root_storage();
    let producer = run_root_bare(&storage);
    // A value resident in the producer scope's region, sealed as its own delivery envelope pinned
    // by the frame that owns that region.
    let obj: &KObject = producer.brand().alloc_object(KObject::Number(42.0));
    let cell = Delivered::hosted(
        producer.seal_resident_value(Carried::Object(obj), None, false),
        std::rc::Rc::clone(&storage),
        crate::machine::core::FramePins::empty(),
    );

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
    use crate::machine::model::{Carried, KObject};
    use crate::machine::DeliveredCarried;
    use crate::witnessed::{Delivered, Sealed};
    use std::rc::Rc;

    // A value in the producer frame's own region, wrapped as a delivery envelope pinned by that
    // frame — the shape a delivered dep arrives in (host = the retention hold's owner).
    let producer_frame = run_root_storage();
    let cell: DeliveredCarried = Delivered::hosted(
        Sealed::seal(KoanRegion::alloc_witnessed(
            Rc::clone(&producer_frame),
            |r| Carried::Object(r.alloc_object(KObject::Number(9.0))),
        )),
        Rc::clone(&producer_frame),
        crate::machine::core::FramePins::empty(),
    );

    // A consumer scope in a *different* frame adopts the carrier — its reach-set folds the producer.
    let consumer_frame = run_root_storage();
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

/// `child_module_reach` mints the child scope's **own region alone** into the parent's arena: a
/// module value's only region borrow is its child scope, and that child region owns the union bundle
/// covering everything its members reach. So a member whose reach names a region foreign to *both*
/// the child and the parent (the shape a transparent `:!` ascription's nested member reach has) is
/// absent from the parent's description yet still pinned — transitively, through the child region's
/// own union — once every direct handle drops.
#[test]
fn child_module_reach_names_the_child_region_which_owns_its_members_reaches() {
    use crate::machine::core::arena::KoanRegion;
    use crate::machine::model::KObject;

    // A frame foreign to everything else here — the region a nested member's own reach names.
    let inner_storage = run_root_storage();
    let inner_weak = Rc::downgrade(&inner_storage);
    let inner_region_ptr: *const KoanRegion = inner_storage.region();

    let source_storage = run_root_storage();
    let source_weak = Rc::downgrade(&source_storage);
    let source_scope = run_root_bare(&source_storage);

    // Bind a member into `source_scope` whose reach names `inner_storage` — mirrors a nested module
    // member reaching into another module's own region. The mint folds the owning bundle into
    // `source_scope`'s region union, so nothing but that region keeps `inner_storage` alive.
    let obj: &KObject = source_scope.brand().alloc_object(KObject::Number(1.0));
    let (reach, borrows_home) =
        source_scope.mint_retained(&FramePins::singleton(Rc::clone(&inner_storage)));
    let sealed = source_scope.seal_resident_value(Carried::Object(obj), reach, borrows_home);
    source_scope
        .bind_value("m".to_string(), sealed, None, BindingIndex::value(0))
        .expect("bind should succeed");
    let source_region_ptr: *const KoanRegion = source_scope.region();

    let parent_storage = run_root_storage();
    let parent = run_root_bare(&parent_storage);
    let (child_reach, _borrows_into_parent) = parent.child_module_reach(source_scope);

    let members: Vec<*const KoanRegion> = child_reach
        .expect(
            "the child's own region is a foreign member of the parent, so the mint is non-empty",
        )
        .members()
        .iter()
        .map(|m| m.region() as *const KoanRegion)
        .collect();
    assert_eq!(
        members.as_slice(),
        &[source_region_ptr],
        "the description names the child scope's own region and nothing else",
    );
    assert!(
        !members.contains(&inner_region_ptr),
        "the member's foreign reach rides the child region's own union, not the parent's description",
    );

    // Drop every other handle: the parent's minted bundle is the sole pin on the child region, and
    // that region's own union is the sole pin on the member's foreign region.
    drop(source_storage);
    drop(inner_storage);
    assert!(
        source_weak.upgrade().is_some(),
        "the parent's minted bundle still pins the child's own region"
    );
    assert!(
        inner_weak.upgrade().is_some(),
        "the child region's own union still pins the member's foreign region"
    );
}
