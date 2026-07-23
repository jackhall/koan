use super::*;
use crate::machine::model::types::{RecursiveGroupWindow, RelativeSchema};
use crate::machine::model::values::KKey;
use crate::machine::model::TypeRegistry;
use std::collections::HashMap;

/// A singleton newtype member handle named `name` over `repr`.
fn newtype_singleton(name: &str, repr: KType, types: &TypeRegistry) -> KType {
    RecursiveGroupWindow::seal_singleton(name.into(), RelativeSchema::NewType(repr), None, types)
}

/// Mint the zero-dep fold door a container test needs, over a fresh root region, as two `let`
/// bindings in the caller's own scope: `forge_for_test` is the sanctioned test-only placement mint
/// (no enclosing fold engine required). A statement macro (not a function returning the pair) so
/// `door`'s borrow of `storage` lives in the same frame it was minted in, never crossing a return.
macro_rules! container_door {
    ($storage:ident, $door:ident) => {
        use crate::machine::core::{run_root_storage, FoldingBrand, FrameStorageExt};
        use crate::witnessed::FoldedPlacement;
        let $storage = run_root_storage();
        let $door = FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(
            $storage.brand().handle(),
        ));
    };
}

#[test]
fn ktype_of_homogeneous_number_list() {
    let types = TypeRegistry::new();
    container_door!(_storage, door);
    let l: KObject<'_> = KObject::list(
        door,
        vec![KObject::Number(1.0), KObject::Number(2.0)],
        &types,
    );
    assert_eq!(l.ktype(), types.list(KType::NUMBER));
}

#[test]
fn ktype_of_mixed_list_is_list_any() {
    let types = TypeRegistry::new();
    container_door!(_storage, door);
    let l: KObject<'_> = KObject::list(
        door,
        vec![KObject::Number(1.0), KObject::KString("x".into())],
        &types,
    );
    assert_eq!(l.ktype(), types.list(KType::ANY));
}

#[test]
fn ktype_of_empty_list_is_list_any() {
    let types = TypeRegistry::new();
    container_door!(_storage, door);
    let l: KObject<'_> = KObject::list(door, vec![], &types);
    assert_eq!(l.ktype(), types.list(KType::ANY));
}

#[test]
fn ktype_of_nested_list() {
    let types = TypeRegistry::new();
    container_door!(_storage, door);
    let inner: KObject<'_> = KObject::list(door, vec![KObject::Number(1.0)], &types);
    let outer: KObject<'_> = KObject::list(door, vec![inner], &types);
    assert_eq!(outer.ktype(), types.list(types.list(KType::NUMBER)));
}

#[test]
fn ktype_of_dict_string_number() {
    let types = TypeRegistry::new();
    let mut map: HashMap<KKey, KObject<'static>> = HashMap::new();
    map.insert(KKey::String("a".into()), KObject::Number(1.0));
    map.insert(KKey::String("b".into()), KObject::Number(2.0));
    let d: KObject<'_> = KObject::dict(map, &types);
    assert_eq!(d.ktype(), types.dict(KType::STR, KType::NUMBER));
}

#[test]
fn ktype_of_empty_dict_is_dict_any_any() {
    let types = TypeRegistry::new();
    let map: HashMap<KKey, KObject<'static>> = HashMap::new();
    let d: KObject<'_> = KObject::dict(map, &types);
    assert_eq!(d.ktype(), types.dict(KType::ANY, KType::ANY));
}

#[test]
fn matches_value_list_number_rejects_string_element() {
    let types = TypeRegistry::new();
    container_door!(_storage, door);
    let t = types.list(KType::NUMBER);
    let bad: KObject<'_> = KObject::list(
        door,
        vec![KObject::Number(1.0), KObject::KString("x".into())],
        &types,
    );
    assert!(!t.matches_value(&bad, &types));
}

#[test]
fn matches_value_list_number_accepts_all_numbers() {
    let types = TypeRegistry::new();
    container_door!(_storage, door);
    let t = types.list(KType::NUMBER);
    let good: KObject<'_> = KObject::list(
        door,
        vec![KObject::Number(1.0), KObject::Number(2.0)],
        &types,
    );
    assert!(t.matches_value(&good, &types));
}

#[test]
fn matches_value_list_any_accepts_any_list() {
    let types = TypeRegistry::new();
    container_door!(_storage, door);
    let t = types.list(KType::ANY);
    let mixed: KObject<'_> = KObject::list(
        door,
        vec![KObject::Number(1.0), KObject::KString("x".into())],
        &types,
    );
    assert!(t.matches_value(&mixed, &types));
}

/// Carrier is authoritative for `ktype()`: a stamped `List<Any>` reports `Any`
/// even when contents would join to `Number`.
#[test]
fn list_with_type_carrier_is_authoritative_for_ktype() {
    let types = TypeRegistry::new();
    container_door!(_storage, door);
    let list_any = types.list(KType::ANY);
    // Contents join to `Number`; the stamp re-tags the shared substrate to `List<Any>`.
    let stamped = KObject::list(
        door,
        vec![KObject::Number(1.0), KObject::Number(2.0)],
        &types,
    )
    .stamp_type(list_any, &types);
    assert_eq!(stamped.ktype(), list_any);
}

/// A `TypeConstructor` (`Result`) value carries its identity handle directly: an erased carrier
/// holds the bare member reference, a stamped carrier the applied `ConstructorApply`.
#[test]
fn type_constructor_ktype_erased_vs_applied() {
    use std::rc::Rc;
    let types = TypeRegistry::new();
    let ctor = RecursiveGroupWindow::seal_singleton(
        "Result".into(),
        RelativeSchema::TypeConstructor {
            schema: HashMap::new(),
            param_names: vec!["Ok".into(), "Error".into()],
        },
        None,
        &types,
    );
    let erased = KObject::Tagged {
        tag: "Ok".into(),
        value: Rc::new(KObject::Number(1.0)),
        identity: ctor,
    };
    let erased_handle = erased.ktype();
    match types.node(erased_handle) {
        TypeNode::SetMember { name, .. } => assert_eq!(name, "Result"),
        _ => panic!("expected SetMember, got {erased_handle:?}"),
    }
    let arguments = Record::from_pairs([
        ("Ok".to_string(), KType::NUMBER),
        ("Error".to_string(), KType::STR),
    ]);
    let applied = KObject::Tagged {
        tag: "Ok".into(),
        value: Rc::new(KObject::Number(1.0)),
        identity: types.constructor_apply(ctor, arguments.clone()),
    };
    let applied_handle = applied.ktype();
    match types.node(applied_handle) {
        TypeNode::ConstructorApply {
            arguments: applied_args,
            ..
        } => {
            assert_eq!(applied_args, arguments);
        }
        _ => panic!("expected ConstructorApply, got {applied_handle:?}"),
    }
}

#[test]
fn stamp_type_coarsens_list_carrier() {
    let types = TypeRegistry::new();
    container_door!(_storage, door);
    let value = KObject::list(door, vec![KObject::Number(1.0)], &types);
    assert_eq!(value.ktype(), types.list(KType::NUMBER));
    let list_any = types.list(KType::ANY);
    let stamped = value.stamp_type(list_any, &types);
    assert_eq!(stamped.ktype(), list_any);
}

#[test]
fn unstamped_empty_container_detection() {
    use std::collections::HashMap;
    let types = TypeRegistry::new();
    container_door!(_storage, door);
    assert!(KObject::list(door, vec![], &types).is_unstamped_empty_container());
    let stamped = KObject::list(door, vec![], &types).stamp_type(types.list(KType::NUMBER), &types);
    assert!(!stamped.is_unstamped_empty_container());
    let hetero = KObject::list(
        door,
        vec![KObject::Number(1.0), KObject::KString("x".into())],
        &types,
    );
    assert!(!hetero.is_unstamped_empty_container());
    let map: HashMap<KKey, KObject<'static>> = HashMap::new();
    assert!(KObject::dict(map, &types).is_unstamped_empty_container());
}

/// `Wrapped.ktype()` reports a copy of the member-handle identity the dispatcher reads for
/// per-declaration identity comparisons.
#[test]
fn wrapped_ktype_reports_clone_of_type_id() {
    use crate::machine::core::{run_root_storage, FrameStorageExt};
    let types = TypeRegistry::new();
    let storage = run_root_storage();
    let region = storage.brand();
    let inner = region.alloc_object(KObject::Number(3.0));
    let type_id = newtype_singleton("Distance", KType::NUMBER, &types);
    let w = KObject::Wrapped {
        inner: WrappedPayload::peel(inner),
        type_id,
    };
    let handle = w.ktype();
    match types.node(handle) {
        TypeNode::SetMember { name, .. } => assert_eq!(name, "Distance"),
        _ => panic!("expected NewType SetMember identity, got {handle:?}"),
    }
}

#[test]
fn wrapped_summarize_renders_surface_form() {
    use crate::machine::core::{run_root_storage, FrameStorageExt};
    let types = TypeRegistry::new();
    let storage = run_root_storage();
    let region = storage.brand();
    let inner = region.alloc_object(KObject::Number(3.0));
    let type_id = newtype_singleton("Distance", KType::NUMBER, &types);
    let w = KObject::Wrapped {
        inner: WrappedPayload::peel(inner),
        type_id,
    };
    assert_eq!(w.summarize(&types), "Distance(3)");
}

/// `deep_clone` is shallow: it `Rc::clone`s the inner (sharing the same allocation as the
/// source `Wrapped`, not re-deep-cloning the repr) and copies the `type_id` handle.
#[test]
fn wrapped_deep_clone_shares_inner_rc_and_type_id() {
    use crate::machine::core::{run_root_storage, FrameStorageExt};
    let types = TypeRegistry::new();
    let storage = run_root_storage();
    let region = storage.brand();
    let inner = region.alloc_object(KObject::Number(3.0));
    let type_id = newtype_singleton("Distance", KType::NUMBER, &types);
    let original = KObject::Wrapped {
        inner: WrappedPayload::peel(inner),
        type_id,
    };
    // `peel` `Rc`-boxes a fresh deep_clone, so the source's inner is its own allocation;
    // `deep_clone` must then share *that* allocation, never re-allocate.
    let original_inner: *const KObject = match &original {
        KObject::Wrapped { inner, .. } => inner.get(),
        _ => unreachable!(),
    };
    let cloned = original.deep_clone();
    match cloned {
        KObject::Wrapped {
            inner: ci,
            type_id: ct,
        } => {
            assert_eq!(
                ci.get() as *const KObject,
                original_inner,
                "deep_clone must Rc::clone the inner, sharing the source allocation",
            );
            assert_eq!(ct, type_id);
        }
        _ => panic!("expected Wrapped after deep_clone"),
    }
}

// --- KObject::resident_in / resident_in_delivered ---------------------------------

/// A `KFunction` allocated into `dest`'s own region is dest-resident.
#[test]
fn resident_in_true_for_same_region_kfunction() {
    use crate::builtins::test_support::TestRun;
    use crate::machine::core::Body;
    use crate::machine::core::{run_root_storage, FrameStorageExt};
    use crate::machine::model::ast::KExpression;
    use crate::machine::model::types::{ExpressionSignature, ReturnType};
    use crate::machine::KFunction;

    let storage = run_root_storage();
    let test_run = TestRun::silent(&storage);
    let scope = test_run.scope;
    let sig = ExpressionSignature {
        return_type: ReturnType::Resolved(KType::NUMBER),
        elements: Vec::new(),
    };
    let f = storage.brand().alloc_function(KFunction::new(
        sig,
        Body::UserDefined(KExpression::new(Vec::new())),
        scope,
        false,
        &test_run.types,
    ));
    let o = KObject::KFunction(f);
    assert!(o.resident_in(storage.region()));
}

/// A `KFunction` allocated into a foreign region is not resident in an unrelated `dest`, and
/// [`KObject::resident_in_delivered`] widens the check to cover it once evidence names that
/// region.
#[test]
fn resident_in_delivered_true_when_evidence_covers_foreign_kfunction() {
    use crate::builtins::test_support::TestRun;
    use crate::machine::core::Body;
    use crate::machine::core::{run_root_storage, FrameSet, FrameStorageExt};
    use crate::machine::model::ast::KExpression;
    use crate::machine::model::types::{ExpressionSignature, ReturnType};
    use crate::machine::KFunction;
    use std::rc::Rc;

    let foreign = run_root_storage();
    let foreign_test_run = TestRun::silent(&foreign);
    let foreign_scope = foreign_test_run.scope;
    let sig = ExpressionSignature {
        return_type: ReturnType::Resolved(KType::NUMBER),
        elements: Vec::new(),
    };
    let f = foreign.brand().alloc_function(KFunction::new(
        sig,
        Body::UserDefined(KExpression::new(Vec::new())),
        foreign_scope,
        false,
        &foreign_test_run.types,
    ));
    let o = KObject::KFunction(f);

    let dest = run_root_storage();
    assert!(
        !o.resident_in(dest.region()),
        "sanity: not resident without evidence"
    );

    let foreign_reach = FrameSet::singleton(Rc::clone(&foreign));
    assert!(o.resident_in_delivered(dest.region(), &[&foreign_reach]));
}

/// A `List` born in `dest`'s region is resident there: its element substrate lives in `dest`, so
/// the `owns_substrate` membership check passes (a list is now a region-resident substrate, like a
/// record — residence is home-region membership, not an element walk).
#[test]
fn resident_in_true_for_owned_list() {
    use crate::machine::core::{run_root_storage, FoldingBrand, FrameStorageExt};
    use crate::witnessed::FoldedPlacement;
    let types = TypeRegistry::new();
    let dest = run_root_storage();
    let door =
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(dest.brand().handle()));
    let o = KObject::list(
        door,
        vec![KObject::Number(1.0), KObject::Number(2.0)],
        &types,
    );
    assert!(o.resident_in(dest.region()));
}
