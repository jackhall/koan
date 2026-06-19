use super::*;
use crate::machine::core::ScopeId;
use crate::machine::model::types::{NominalSchema, RecursiveSet};
use crate::machine::model::values::KKey;
use std::collections::HashMap;

/// A singleton tagged-set `Rc` named `name` at `scope_id`.
fn tagged_singleton<'a>(name: &str, scope_id: ScopeId) -> std::rc::Rc<RecursiveSet<'a>> {
    RecursiveSet::singleton(name.into(), scope_id, NominalSchema::Tagged(HashMap::new()))
}

/// A singleton newtype-set `Rc` named `name` over `repr`.
fn newtype_singleton<'a>(
    name: &str,
    scope_id: ScopeId,
    repr: KType<'a>,
) -> std::rc::Rc<RecursiveSet<'a>> {
    RecursiveSet::singleton(
        name.into(),
        scope_id,
        NominalSchema::NewType(Box::new(repr)),
    )
}

#[test]
fn ktype_of_homogeneous_number_list() {
    let l: KObject<'_> = KObject::list(vec![KObject::Number(1.0), KObject::Number(2.0)]);
    assert_eq!(l.ktype(), KType::List(Box::new(KType::Number)));
}

#[test]
fn ktype_of_mixed_list_is_list_any() {
    let l: KObject<'_> = KObject::list(vec![KObject::Number(1.0), KObject::KString("x".into())]);
    assert_eq!(l.ktype(), KType::List(Box::new(KType::Any)));
}

#[test]
fn ktype_of_empty_list_is_list_any() {
    let l: KObject<'_> = KObject::list(vec![]);
    assert_eq!(l.ktype(), KType::List(Box::new(KType::Any)));
}

#[test]
fn ktype_of_nested_list() {
    let inner: KObject<'_> = KObject::list(vec![KObject::Number(1.0)]);
    let outer: KObject<'_> = KObject::list(vec![inner]);
    assert_eq!(
        outer.ktype(),
        KType::List(Box::new(KType::List(Box::new(KType::Number))))
    );
}

#[test]
fn ktype_of_dict_string_number() {
    let mut map: HashMap<Box<dyn Serializable<'static> + 'static>, KObject<'static>> =
        HashMap::new();
    map.insert(Box::new(KKey::String("a".into())), KObject::Number(1.0));
    map.insert(Box::new(KKey::String("b".into())), KObject::Number(2.0));
    let d: KObject<'_> = KObject::dict(map);
    assert_eq!(
        d.ktype(),
        KType::Dict(Box::new(KType::Str), Box::new(KType::Number))
    );
}

#[test]
fn ktype_of_empty_dict_is_dict_any_any() {
    let map: HashMap<Box<dyn Serializable<'static> + 'static>, KObject<'static>> = HashMap::new();
    let d: KObject<'_> = KObject::dict(map);
    assert_eq!(
        d.ktype(),
        KType::Dict(Box::new(KType::Any), Box::new(KType::Any))
    );
}

#[test]
fn matches_value_list_number_rejects_string_element() {
    let t = KType::List(Box::new(KType::Number));
    let bad: KObject<'_> = KObject::list(vec![KObject::Number(1.0), KObject::KString("x".into())]);
    assert!(!t.matches_value(&bad));
}

#[test]
fn matches_value_list_number_accepts_all_numbers() {
    let t = KType::List(Box::new(KType::Number));
    let good: KObject<'_> = KObject::list(vec![KObject::Number(1.0), KObject::Number(2.0)]);
    assert!(t.matches_value(&good));
}

#[test]
fn matches_value_list_any_accepts_any_list() {
    let t = KType::List(Box::new(KType::Any));
    let mixed: KObject<'_> =
        KObject::list(vec![KObject::Number(1.0), KObject::KString("x".into())]);
    assert!(t.matches_value(&mixed));
}

/// Carrier is authoritative for `ktype()`: a stamped `List<Any>` reports `Any`
/// even when contents would join to `Number`.
#[test]
fn list_with_type_carrier_is_authoritative_for_ktype() {
    use crate::machine::model::values::Held;
    use std::rc::Rc;
    let items = Rc::new(vec![
        Held::Object(KObject::Number(1.0)),
        Held::Object(KObject::Number(2.0)),
    ]);
    let stamped = KObject::list_with_type(items, KType::Any);
    assert_eq!(stamped.ktype(), KType::List(Box::new(KType::Any)));
}

/// A user-`UNION` (`Tagged` kind) value reports its *variant* refinement, keyed on the
/// inhabited tag, so a `:(Maybe Some)` slot can dispatch on it.
#[test]
fn tagged_value_ktype_reports_variant() {
    use std::rc::Rc;
    let sid = ScopeId::from_raw(0, 0x55);
    let set = tagged_singleton("Maybe", sid);
    let value = KObject::Tagged {
        tag: "Some".into(),
        value: Rc::new(KObject::Number(1.0)),
        set: Rc::clone(&set),
        index: 0,
        type_args: Rc::new(vec![]),
    };
    match value.ktype() {
        KType::Variant { set: s, index, tag } => {
            assert_eq!(s.member(index).name, "Maybe");
            assert_eq!(tag, "Some");
        }
        other => panic!("expected Variant, got {other:?}"),
    }
}

/// A `TypeConstructor` (`Result`) value keeps the union identity: erased `type_args`
/// reports the bare `SetRef`, a populated carrier the applied `ConstructorApply`.
#[test]
fn type_constructor_ktype_erased_vs_applied() {
    use std::rc::Rc;
    let sid = ScopeId::from_raw(0, 0x55);
    let member = crate::machine::model::types::NominalMember::pending(
        "Result".into(),
        sid,
        crate::machine::model::types::KKind::TypeConstructor,
    );
    member.fill(NominalSchema::TypeConstructor {
        schema: HashMap::new(),
        param_names: vec!["T".into(), "E".into()],
    });
    let set = std::rc::Rc::new(RecursiveSet::new(vec![member]));
    let erased = KObject::Tagged {
        tag: "Ok".into(),
        value: Rc::new(KObject::Number(1.0)),
        set: Rc::clone(&set),
        index: 0,
        type_args: Rc::new(vec![]),
    };
    match erased.ktype() {
        KType::SetRef { set: s, index } => assert_eq!(s.member(index).name, "Result"),
        other => panic!("expected SetRef, got {other:?}"),
    }
    let applied = KObject::Tagged {
        tag: "Ok".into(),
        value: Rc::new(KObject::Number(1.0)),
        set: Rc::clone(&set),
        index: 0,
        type_args: Rc::new(vec![KType::Number, KType::Str]),
    };
    match applied.ktype() {
        KType::ConstructorApply { args, .. } => {
            assert_eq!(args, vec![KType::Number, KType::Str]);
        }
        other => panic!("expected ConstructorApply, got {other:?}"),
    }
}

#[test]
fn stamp_type_coarsens_list_carrier() {
    let value = KObject::list(vec![KObject::Number(1.0)]);
    assert_eq!(value.ktype(), KType::List(Box::new(KType::Number)));
    let stamped = value.stamp_type(&KType::List(Box::new(KType::Any)));
    assert_eq!(stamped.ktype(), KType::List(Box::new(KType::Any)));
}

#[test]
fn unstamped_empty_container_detection() {
    use std::collections::HashMap;
    use std::rc::Rc;
    assert!(KObject::list(vec![]).is_unstamped_empty_container());
    let stamped = KObject::list_with_type(Rc::new(vec![]), KType::Number);
    assert!(!stamped.is_unstamped_empty_container());
    let hetero = KObject::list(vec![KObject::Number(1.0), KObject::KString("x".into())]);
    assert!(!hetero.is_unstamped_empty_container());
    let map: HashMap<Box<dyn Serializable<'static> + 'static>, KObject<'static>> = HashMap::new();
    assert!(KObject::dict(map).is_unstamped_empty_container());
}

/// `Wrapped.ktype()` reports a clone of the `SetRef` identity the dispatcher reads for
/// per-declaration identity comparisons.
#[test]
fn wrapped_ktype_reports_clone_of_type_id() {
    use crate::machine::KoanRegion;
    let arena = KoanRegion::new();
    let inner = arena.alloc_object(KObject::Number(3.0));
    let set = newtype_singleton("Distance", ScopeId::from_raw(0, 0xAA), KType::Number);
    let type_id: &KType = arena.alloc_ktype(KType::SetRef { set, index: 0 });
    let w = KObject::Wrapped {
        inner: NonWrappedRef::peel(inner),
        type_id,
    };
    match w.ktype() {
        KType::SetRef { set, index } => {
            assert_eq!(set.member(index).name, "Distance");
            assert_eq!(set.member(index).scope_id, ScopeId::from_raw(0, 0xAA));
        }
        other => panic!("expected NewType SetRef identity, got {other:?}"),
    }
}

#[test]
fn wrapped_summarize_renders_surface_form() {
    use crate::machine::model::types::Parseable;
    use crate::machine::KoanRegion;
    let arena = KoanRegion::new();
    let inner = arena.alloc_object(KObject::Number(3.0));
    let set = newtype_singleton("Distance", ScopeId::from_raw(0, 0xAA), KType::Number);
    let type_id = arena.alloc_ktype(KType::SetRef { set, index: 0 });
    let w = KObject::Wrapped {
        inner: NonWrappedRef::peel(inner),
        type_id,
    };
    assert_eq!(w.summarize(), "Distance(3)");
}

/// `deep_clone` is shallow: it `Rc::clone`s the inner (sharing the same allocation as the
/// source `Wrapped`, not re-deep-cloning the repr) and copies the `&'a` `type_id` slot.
#[test]
fn wrapped_deep_clone_shares_inner_rc_and_type_id() {
    use crate::machine::KoanRegion;
    let arena = KoanRegion::new();
    let inner = arena.alloc_object(KObject::Number(3.0));
    let set = newtype_singleton("Distance", ScopeId::from_raw(0, 0xAA), KType::Number);
    let type_id = arena.alloc_ktype(KType::SetRef { set, index: 0 });
    let original = KObject::Wrapped {
        inner: NonWrappedRef::peel(inner),
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
            assert!(std::ptr::eq(ct, type_id));
        }
        _ => panic!("expected Wrapped after deep_clone"),
    }
}
