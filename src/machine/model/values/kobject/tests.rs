use super::*;
use crate::builtins::test_support::type_name;
use crate::builtins::test_support::type_token;
use crate::machine::model::RunRegistries;
use crate::machine::model::types::{RecursiveGroupWindow, RelativeSchema};
use crate::machine::model::values::KKey;
use std::collections::HashMap;

/// A singleton newtype member handle named `name` over `repr`.
fn newtype_singleton(name: &str, repr: KType, registries: &RunRegistries) -> KType {
    RecursiveGroupWindow::seal_singleton(
        type_name(name, registries),
        RelativeSchema::NewType(repr),
        None,
        &registries.types,
    )
}

/// Mint the zero-dep fold door a container test needs, over a fresh root region, as two `let`
/// bindings in the caller's own scope: `forge_for_test` is the sanctioned test-only placement mint
/// (no enclosing fold engine required). A statement macro (not a function returning the pair) so
/// `door`'s borrow of `storage` lives in the same frame it was minted in, never crossing a return.
macro_rules! container_door {
    ($storage:ident, $door:ident) => {
        use crate::machine::core::{FoldingBrand, FrameStorageExt, run_root_storage};
        use crate::witnessed::FoldedPlacement;
        let $storage = run_root_storage();
        let owned_cells = crate::machine::core::FrameCoverage::empty();
        let $door = FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(
            $storage.brand().handle(),
        ))
        .with_holder(&owned_cells);
    };
}

#[test]
fn ktype_of_homogeneous_number_list() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    container_door!(_storage, door);
    let l: KObject<'_> = KObject::list(
        door,
        vec![KObject::Number(1.0), KObject::Number(2.0)],
        types,
    );
    assert_eq!(l.ktype(), types.list(KType::NUMBER));
}

#[test]
fn ktype_of_mixed_list_is_list_any() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    container_door!(_storage, door);
    let l: KObject<'_> = KObject::list(
        door,
        vec![KObject::Number(1.0), KObject::KString("x")],
        types,
    );
    assert_eq!(l.ktype(), types.list(KType::ANY));
}

#[test]
fn ktype_of_empty_list_is_list_any() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    container_door!(_storage, door);
    let l: KObject<'_> = KObject::list(door, vec![], types);
    assert_eq!(l.ktype(), types.list(KType::ANY));
}

#[test]
fn ktype_of_nested_list() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    container_door!(_storage, door);
    let inner: KObject<'_> = KObject::list(door, vec![KObject::Number(1.0)], types);
    let outer: KObject<'_> = KObject::list(door, vec![inner], types);
    assert_eq!(outer.ktype(), types.list(types.list(KType::NUMBER)));
}

#[test]
fn ktype_of_dict_string_number() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    container_door!(_storage, door);
    let mut map: HashMap<KKey, KObject<'_>> = HashMap::new();
    map.insert(KKey::String("a"), KObject::Number(1.0));
    map.insert(KKey::String("b"), KObject::Number(2.0));
    let d: KObject<'_> = KObject::dict(door, map, types);
    assert_eq!(d.ktype(), types.dict(KType::STR, KType::NUMBER));
}

#[test]
fn ktype_of_empty_dict_is_dict_any_any() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    container_door!(_storage, door);
    let map: HashMap<KKey, KObject<'_>> = HashMap::new();
    let d: KObject<'_> = KObject::dict(door, map, types);
    assert_eq!(d.ktype(), types.dict(KType::ANY, KType::ANY));
}

#[test]
fn matches_value_list_number_rejects_string_element() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    container_door!(_storage, door);
    let t = types.list(KType::NUMBER);
    let bad: KObject<'_> = KObject::list(
        door,
        vec![KObject::Number(1.0), KObject::KString("x")],
        types,
    );
    assert!(!t.matches_value(&bad, &registries));
}

#[test]
fn matches_value_list_number_accepts_all_numbers() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    container_door!(_storage, door);
    let t = types.list(KType::NUMBER);
    let good: KObject<'_> = KObject::list(
        door,
        vec![KObject::Number(1.0), KObject::Number(2.0)],
        types,
    );
    assert!(t.matches_value(&good, &registries));
}

#[test]
fn matches_value_list_any_accepts_any_list() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    container_door!(_storage, door);
    let t = types.list(KType::ANY);
    let mixed: KObject<'_> = KObject::list(
        door,
        vec![KObject::Number(1.0), KObject::KString("x")],
        types,
    );
    assert!(t.matches_value(&mixed, &registries));
}

/// Carrier is authoritative for `ktype()`: a stamped `List<Any>` reports `Any`
/// even when contents would join to `Number`.
#[test]
fn list_with_type_carrier_is_authoritative_for_ktype() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    container_door!(_storage, door);
    let list_any = types.list(KType::ANY);
    // Contents join to `Number`; the stamp re-tags the shared substrate to `List<Any>`.
    let stamped = KObject::list(
        door,
        vec![KObject::Number(1.0), KObject::Number(2.0)],
        types,
    )
    .stamp_type(list_any, types);
    assert_eq!(stamped.ktype(), list_any);
}

/// A variant value carries its identity handle directly: an erased carrier holds the bare member
/// reference, a stamped carrier the applied `ConstructorApply` over it.
#[test]
fn variant_ktype_erased_vs_applied() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    container_door!(_storage, door);
    let window = RecursiveGroupWindow::for_binder(
        type_token("Result"),
        vec![type_token("Ok"), type_token("Error")],
    );
    window.fill_member(0, RelativeSchema::NewType(KType::ANY), types);
    let sealed = window
        .fill_member(1, RelativeSchema::NewType(KType::ANY), types)
        .expect("a two-member window seals on its second fill");
    let ok = sealed.members[0];

    let erased = KObject::wrapped_hold(door, &KObject::Number(1.0), ok);
    let erased_handle = erased.ktype();
    match types.node(erased_handle) {
        TypeNode::SetMember { name, .. } => assert_eq!(name, type_token("Ok")),
        _ => panic!("expected SetMember, got {erased_handle:?}"),
    }
    let arguments = Record::from_pairs([(
        crate::builtins::test_support::binder_token("Ok"),
        KType::NUMBER,
    )]);
    let applied = KObject::wrapped_hold(
        door,
        &KObject::Number(1.0),
        types.constructor_apply(ok, arguments.clone()),
    );
    match types.node(applied.ktype()) {
        TypeNode::ConstructorApply {
            arguments: applied_args,
            ..
        } => assert_eq!(applied_args, arguments),
        _ => panic!("expected ConstructorApply identity"),
    }
}

/// A union slot stamps a variant value to the application over the member it inhabits — the
/// boundary shape `:(Result {Ok = Number, Error = Str})` drives. A member the slot left bare, and
/// a value of a member the slot never declares, pass through unchanged.
#[test]
fn stamp_type_adopts_the_inhabited_members_application() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    container_door!(_storage, door);
    let window = RecursiveGroupWindow::for_binder(
        type_token("Result"),
        vec![type_token("Ok"), type_token("Error")],
    );
    window.fill_member(0, RelativeSchema::NewType(KType::ANY), types);
    let sealed = window
        .fill_member(1, RelativeSchema::NewType(KType::ANY), types)
        .expect("a two-member window seals on its second fill");
    let (ok, error) = (sealed.members[0], sealed.members[1]);
    let ok_applied = types.constructor_apply(
        ok,
        Record::from_pairs([(
            crate::builtins::test_support::binder_token("Ok"),
            KType::NUMBER,
        )]),
    );
    // `Error` rides bare, so a value of it keeps its own member handle.
    let declared = types.union_of(&[ok_applied, error]);

    let stamped =
        KObject::wrapped_hold(door, &KObject::Number(1.0), ok).stamp_type(declared, types);
    assert_eq!(stamped.ktype(), ok_applied);
    let bare =
        KObject::wrapped_hold(door, &KObject::Number(1.0), error).stamp_type(declared, types);
    assert_eq!(bare.ktype(), error);
    let foreign = newtype_singleton("Distance", KType::NUMBER, &registries);
    let untouched =
        KObject::wrapped_hold(door, &KObject::Number(1.0), foreign).stamp_type(declared, types);
    assert_eq!(untouched.ktype(), foreign);
}

#[test]
fn stamp_type_coarsens_list_carrier() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    container_door!(_storage, door);
    let value = KObject::list(door, vec![KObject::Number(1.0)], types);
    assert_eq!(value.ktype(), types.list(KType::NUMBER));
    let list_any = types.list(KType::ANY);
    let stamped = value.stamp_type(list_any, types);
    assert_eq!(stamped.ktype(), list_any);
}

#[test]
fn unstamped_empty_container_detection() {
    use std::collections::HashMap;
    let registries = RunRegistries::new();
    let types = &registries.types;
    container_door!(_storage, door);
    assert!(KObject::list(door, vec![], types).is_unstamped_empty_container());
    let stamped = KObject::list(door, vec![], types).stamp_type(types.list(KType::NUMBER), types);
    assert!(!stamped.is_unstamped_empty_container());
    let hetero = KObject::list(
        door,
        vec![KObject::Number(1.0), KObject::KString("x")],
        types,
    );
    assert!(!hetero.is_unstamped_empty_container());
    let map: HashMap<KKey, KObject<'_>> = HashMap::new();
    assert!(KObject::dict(door, map, types).is_unstamped_empty_container());
}

/// `Wrapped.ktype()` reports a copy of the member-handle identity the dispatcher reads for
/// per-declaration identity comparisons.
#[test]
fn wrapped_ktype_reports_clone_of_type_id() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    container_door!(_storage, door);
    let type_id = newtype_singleton("Distance", KType::NUMBER, &registries);
    let w = KObject::wrapped_peel(door, &KObject::Number(3.0), type_id);
    let handle = w.ktype();
    match types.node(handle) {
        TypeNode::SetMember { name, .. } => assert_eq!(name, type_token("Distance")),
        _ => panic!("expected NewType SetMember identity, got {handle:?}"),
    }
}

#[test]
fn wrapped_summarize_renders_surface_form() {
    let registries = RunRegistries::new();
    container_door!(_storage, door);
    let type_id = newtype_singleton("Distance", KType::NUMBER, &registries);
    let w = KObject::wrapped_peel(door, &KObject::Number(3.0), type_id);
    assert_eq!(w.summarize(&registries), "Distance(3)");
}

/// `deep_clone` is shallow: it pointer-copies the payload substrate borrow (sharing the same
/// region-resident substrate as the source `Wrapped`, not re-rebuilding the repr) and copies the
/// `type_id` handle.
#[test]
fn wrapped_deep_clone_shares_inner_substrate_and_type_id() {
    let registries = RunRegistries::new();
    container_door!(_storage, door);
    let type_id = newtype_singleton("Distance", KType::NUMBER, &registries);
    let original = KObject::wrapped_peel(door, &KObject::Number(3.0), type_id);
    // The source's payload rides its own region-resident substrate; `deep_clone` must share *that*
    // substrate borrow, never allocate a fresh one.
    let original_inner: *const PayloadSubstrate = match &original {
        KObject::Wrapped { inner, .. } => *inner as *const _,
        _ => unreachable!(),
    };
    let cloned = original.deep_clone();
    match cloned {
        KObject::Wrapped {
            inner: ci,
            type_id: ct,
        } => {
            assert_eq!(
                ci as *const PayloadSubstrate, original_inner,
                "deep_clone must pointer-copy the substrate borrow, sharing the source substrate",
            );
            assert_eq!(ct, type_id);
        }
        _ => panic!("expected Wrapped after deep_clone"),
    }
}
