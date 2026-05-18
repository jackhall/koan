use super::*;
use crate::machine::model::values::KKey;
use std::collections::HashMap;

#[test]
fn ktype_of_homogeneous_number_list() {
    let l: KObject<'_> =
        KObject::List(Rc::new(vec![KObject::Number(1.0), KObject::Number(2.0)]));
    assert_eq!(l.ktype(), KType::List(Box::new(KType::Number)));
}

#[test]
fn ktype_of_mixed_list_is_list_any() {
    let l: KObject<'_> = KObject::List(Rc::new(vec![
        KObject::Number(1.0),
        KObject::KString("x".into()),
    ]));
    assert_eq!(l.ktype(), KType::List(Box::new(KType::Any)));
}

#[test]
fn ktype_of_empty_list_is_list_any() {
    let l: KObject<'_> = KObject::List(Rc::new(vec![]));
    assert_eq!(l.ktype(), KType::List(Box::new(KType::Any)));
}

#[test]
fn ktype_of_nested_list() {
    let inner: KObject<'_> = KObject::List(Rc::new(vec![KObject::Number(1.0)]));
    let outer: KObject<'_> = KObject::List(Rc::new(vec![inner]));
    assert_eq!(
        outer.ktype(),
        KType::List(Box::new(KType::List(Box::new(KType::Number))))
    );
}

#[test]
fn ktype_of_dict_string_number() {
    let mut map: HashMap<Box<dyn Serializable + 'static>, KObject<'static>> = HashMap::new();
    map.insert(Box::new(KKey::String("a".into())), KObject::Number(1.0));
    map.insert(Box::new(KKey::String("b".into())), KObject::Number(2.0));
    let d: KObject<'_> = KObject::Dict(Rc::new(map));
    assert_eq!(
        d.ktype(),
        KType::Dict(Box::new(KType::Str), Box::new(KType::Number))
    );
}

#[test]
fn ktype_of_empty_dict_is_dict_any_any() {
    let map: HashMap<Box<dyn Serializable + 'static>, KObject<'static>> = HashMap::new();
    let d: KObject<'_> = KObject::Dict(Rc::new(map));
    assert_eq!(
        d.ktype(),
        KType::Dict(Box::new(KType::Any), Box::new(KType::Any))
    );
}

#[test]
fn matches_value_list_number_rejects_string_element() {
    let t = KType::List(Box::new(KType::Number));
    let bad: KObject<'_> = KObject::List(Rc::new(vec![
        KObject::Number(1.0),
        KObject::KString("x".into()),
    ]));
    assert!(!t.matches_value(&bad));
}

#[test]
fn matches_value_list_number_accepts_all_numbers() {
    let t = KType::List(Box::new(KType::Number));
    let good: KObject<'_> = KObject::List(Rc::new(vec![
        KObject::Number(1.0),
        KObject::Number(2.0),
    ]));
    assert!(t.matches_value(&good));
}

#[test]
fn matches_value_list_any_accepts_any_list() {
    let t = KType::List(Box::new(KType::Any));
    let mixed: KObject<'_> = KObject::List(Rc::new(vec![
        KObject::Number(1.0),
        KObject::KString("x".into()),
    ]));
    assert!(t.matches_value(&mixed));
}

/// `TypeNameRef` summarizes through `TypeExpr::render`, preserving the surface form
/// (`MyT`, `Point<Foo>`) for diagnostics. The surface form must survive bind
/// regardless of whether downstream scope-aware consumers have resolved the
/// carrier.
#[test]
fn type_name_ref_summarize_renders_surface_form() {
    use crate::machine::model::ast::TypeExpr;
    let v = KObject::TypeNameRef(TypeExpr::leaf("MyT".into()));
    use crate::machine::model::types::Parseable;
    assert_eq!(v.summarize(), "MyT");
}

/// `TypeNameRef::ktype()` reports `TypeExprRef` so it fills the same dispatch slot as
/// the fully-elaborated `KTypeValue` carrier. Pins the slot-routing invariant.
#[test]
fn type_name_ref_ktype_is_type_expr_ref() {
    use crate::machine::model::ast::TypeExpr;
    let v = KObject::TypeNameRef(TypeExpr::leaf("MyT".into()));
    assert_eq!(v.ktype(), KType::TypeExprRef);
}

#[test]
fn ktype_value_round_trips_through_summarize() {
    // `KObject::KTypeValue` summarizes through `KType::render`, mirroring the surface
    // form a user would write. Pins the post-refactor diagnostic shape.
    let v = KObject::KTypeValue(KType::List(Box::new(KType::Number)));
    use crate::machine::model::types::Parseable;
    assert_eq!(v.summarize(), ":(List Number)");
}

/// Stage 4: `Wrapped::ktype()` reports a clone of `*type_id`, preserving the full
/// `(kind, scope_id, name)` triple the dispatcher reads for per-declaration identity
/// comparisons.
#[test]
fn wrapped_ktype_reports_clone_of_type_id() {
    use crate::machine::RuntimeArena;
    let arena = RuntimeArena::new();
    let inner = arena.alloc_object(KObject::Number(3.0));
    let type_id: &KType = arena.alloc_ktype(KType::UserType {
        kind: UserTypeKind::Newtype { repr: Box::new(KType::Number) },
        scope_id: ScopeId::from_raw(0, 0xAA),
        name: "Distance".into(),
    });
    let w = KObject::Wrapped { inner: NonWrappedRef::peel(inner), type_id };
    match w.ktype() {
        KType::UserType { kind: UserTypeKind::Newtype { .. }, name, scope_id } => {
            assert_eq!(name, "Distance");
            assert_eq!(scope_id, ScopeId::from_raw(0, 0xAA));
        }
        other => panic!("expected Newtype identity, got {other:?}"),
    }
}

/// Stage 4: `Wrapped::summarize()` renders `Distance(<inner>)`, mirroring the
/// surface-form invariant Struct / Tagged carriers honor.
#[test]
fn wrapped_summarize_renders_surface_form() {
    use crate::machine::RuntimeArena;
    use crate::machine::model::types::Parseable;
    let arena = RuntimeArena::new();
    let inner = arena.alloc_object(KObject::Number(3.0));
    let type_id = arena.alloc_ktype(KType::UserType {
        kind: UserTypeKind::Newtype { repr: Box::new(KType::Number) },
        scope_id: ScopeId::from_raw(0, 0xAA),
        name: "Distance".into(),
    });
    let w = KObject::Wrapped { inner: NonWrappedRef::peel(inner), type_id };
    assert_eq!(w.summarize(), "Distance(3)");
}

/// Stage 4: `Wrapped::deep_clone()` copies both arena references without
/// re-allocating. The cloned `inner` and `type_id` point at the same arena slots.
#[test]
fn wrapped_deep_clone_preserves_arena_references() {
    use crate::machine::RuntimeArena;
    let arena = RuntimeArena::new();
    let inner = arena.alloc_object(KObject::Number(3.0));
    let type_id = arena.alloc_ktype(KType::UserType {
        kind: UserTypeKind::Newtype { repr: Box::new(KType::Number) },
        scope_id: ScopeId::from_raw(0, 0xAA),
        name: "Distance".into(),
    });
    let original = KObject::Wrapped { inner: NonWrappedRef::peel(inner), type_id };
    let cloned = original.deep_clone();
    match cloned {
        KObject::Wrapped { inner: ci, type_id: ct } => {
            assert!(std::ptr::eq(ci.get(), inner));
            assert!(std::ptr::eq(ct, type_id));
        }
        _ => panic!("expected Wrapped after deep_clone"),
    }
}
