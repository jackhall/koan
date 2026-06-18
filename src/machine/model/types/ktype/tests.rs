use super::super::recursive_set::{NominalMember, NominalSchema};
use super::*;

/// A singleton `Rc<RecursiveSet>` over a record-repr newtype member named `name`, schema
/// filled.
fn record_newtype_set<'a>(name: &str, scope_id: ScopeId) -> Rc<RecursiveSet<'a>> {
    let member = NominalMember::pending(name.into(), scope_id, KKind::Newtype);
    member.fill(NominalSchema::Newtype(Box::new(KType::Record(Box::new(
        Record::new(),
    )))));
    Rc::new(RecursiveSet::new(vec![member]))
}

#[test]
fn name_renders_parameterized_list() {
    let t = KType::List(Box::new(KType::List(Box::new(KType::Number))));
    assert_eq!(t.name(), ":(LIST OF :(LIST OF Number))");
}

#[test]
fn name_renders_dict() {
    let t = KType::Dict(Box::new(KType::Str), Box::new(KType::Number));
    assert_eq!(t.name(), ":(MAP Str -> Number)");
}

#[test]
fn name_renders_function() {
    let t = KType::KFunction {
        params: Record::from_pairs(vec![("x".into(), KType::Number), ("y".into(), KType::Str)]),
        ret: Box::new(KType::Bool),
    };
    assert_eq!(t.name(), ":(FN (x :Number y :Str) -> Bool)");
}

/// A nested sigiled parameter type already opens with `:`, so the renderer must not
/// prefix a second colon (`xs :(LIST OF Number)`, not `xs ::(LIST OF Number)`).
#[test]
fn name_renders_function_with_sigiled_param() {
    let t = KType::KFunction {
        params: Record::from_pairs(vec![("xs".into(), KType::List(Box::new(KType::Number)))]),
        ret: Box::new(KType::Bool),
    };
    assert_eq!(t.name(), ":(FN (xs :(LIST OF Number)) -> Bool)");
}

#[test]
fn name_renders_functor() {
    let t = KType::KFunctor {
        params: Record::from_pairs(vec![("x".into(), KType::Number), ("y".into(), KType::Str)]),
        ret: Box::new(KType::Bool),
        body: None,
    };
    assert_eq!(t.name(), ":(FUNCTOR (x :Number y :Str) -> Bool)");
}

#[test]
fn functor_structural_eq_same_shape() {
    let a = KType::KFunctor {
        params: Record::from_pairs(vec![("x".into(), KType::Number), ("y".into(), KType::Str)]),
        ret: Box::new(KType::Bool),
        body: None,
    };
    let b = KType::KFunctor {
        params: Record::from_pairs(vec![("x".into(), KType::Number), ("y".into(), KType::Str)]),
        ret: Box::new(KType::Bool),
        body: None,
    };
    assert_eq!(a, b);
}

#[test]
fn functor_structural_neq_when_params_or_ret_differ() {
    let base = KType::KFunctor {
        params: Record::from_pairs(vec![("x".into(), KType::Number)]),
        ret: Box::new(KType::Bool),
        body: None,
    };
    let diff_params = KType::KFunctor {
        params: Record::from_pairs(vec![("x".into(), KType::Str)]),
        ret: Box::new(KType::Bool),
        body: None,
    };
    let diff_ret = KType::KFunctor {
        params: Record::from_pairs(vec![("x".into(), KType::Number)]),
        ret: Box::new(KType::Null),
        body: None,
    };
    assert_ne!(base, diff_params);
    assert_ne!(base, diff_ret);
}

#[test]
fn functor_and_function_are_disjoint_types() {
    let f = KType::KFunction {
        params: Record::from_pairs(vec![("x".into(), KType::Number)]),
        ret: Box::new(KType::Bool),
    };
    let g = KType::KFunctor {
        params: Record::from_pairs(vec![("x".into(), KType::Number)]),
        ret: Box::new(KType::Bool),
        body: None,
    };
    assert_ne!(f, g);
}

#[test]
fn name_renders_function_nullary() {
    let t = KType::KFunction {
        params: Record::new(),
        ret: Box::new(KType::Any),
    };
    assert_eq!(t.name(), ":(FN () -> Any)");
}

/// Function-slot identity is the record substrate's order-blind equality: the same
/// parameters by `(name, type)` in a different declaration order compare equal and
/// hash equal.
#[test]
fn function_params_order_blind_equality() {
    let xy = KType::KFunction {
        params: Record::from_pairs(vec![("x".into(), KType::Number), ("y".into(), KType::Str)]),
        ret: Box::new(KType::Bool),
    };
    let yx = KType::KFunction {
        params: Record::from_pairs(vec![("y".into(), KType::Str), ("x".into(), KType::Number)]),
        ret: Box::new(KType::Bool),
    };
    assert_eq!(xy, yx);
    assert_eq!(hash_of(&xy), hash_of(&yx));
}

/// Identity is name-sensitive: same type, different parameter name is a different
/// function type.
#[test]
fn function_params_name_sensitive_inequality() {
    let x = KType::KFunction {
        params: Record::from_pairs(vec![("x".into(), KType::Number)]),
        ret: Box::new(KType::Bool),
    };
    let a = KType::KFunction {
        params: Record::from_pairs(vec![("a".into(), KType::Number)]),
        ret: Box::new(KType::Bool),
    };
    assert_ne!(x, a);
}

#[test]
fn name_renders_recursive_ref_as_name() {
    let t = KType::RecursiveRef("Tree".into());
    assert_eq!(t.name(), "Tree");
}

#[test]
fn nominal_kind_surface_keywords() {
    assert_eq!(KKind::Tagged.surface_keyword(), "Tagged");
    assert_eq!(KKind::Newtype.surface_keyword(), "Newtype");
    assert_eq!(KKind::TypeConstructor.surface_keyword(), "TypeConstructor",);
}

#[test]
fn nominal_of_kind_name_renders_family_keyword() {
    assert_eq!(KType::OfKind(KKind::Newtype).name(), "Newtype");
    assert_eq!(KType::OfKind(KKind::Tagged).name(), "Tagged");
    assert_eq!(
        KType::OfKind(KKind::TypeConstructor).name(),
        "TypeConstructor"
    );
}

#[test]
fn any_module_and_any_signature_render_surface_keywords() {
    let am: KType<'_> = KType::OfKind(KKind::Module);
    let asg: KType<'_> = KType::OfKind(KKind::Signature);
    assert_eq!(am.name(), "Module");
    assert_eq!(asg.name(), "Signature");
}

fn hash_of(t: &KType<'_>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    t.hash(&mut h);
    h.finish()
}

/// `a == b` ⟹ `hash(a) == hash(b)` across every arena-free variant. Each pair is
/// built independently so a stray identity-from-pointer bug would surface.
#[test]
fn hash_agrees_with_eq_for_arena_free_variants() {
    let sid = ScopeId::from_raw(0, 0xBEEF);
    let pairs: Vec<(KType<'_>, KType<'_>)> = vec![
        (KType::Number, KType::Number),
        (KType::Str, KType::Str),
        (KType::Bool, KType::Bool),
        (KType::Null, KType::Null),
        (KType::Identifier, KType::Identifier),
        (KType::KExpression, KType::KExpression),
        (KType::OfKind(KKind::Proper), KType::OfKind(KKind::Proper)),
        (KType::OfKind(KKind::Any), KType::OfKind(KKind::Any)),
        (KType::Any, KType::Any),
        (KType::OfKind(KKind::Module), KType::OfKind(KKind::Module)),
        (
            KType::OfKind(KKind::Signature),
            KType::OfKind(KKind::Signature),
        ),
        (
            KType::List(Box::new(KType::Number)),
            KType::List(Box::new(KType::Number)),
        ),
        (
            KType::Dict(Box::new(KType::Str), Box::new(KType::Number)),
            KType::Dict(Box::new(KType::Str), Box::new(KType::Number)),
        ),
        (
            KType::KFunction {
                params: Record::from_pairs(vec![("x".into(), KType::Number)]),
                ret: Box::new(KType::Bool),
            },
            KType::KFunction {
                params: Record::from_pairs(vec![("x".into(), KType::Number)]),
                ret: Box::new(KType::Bool),
            },
        ),
        (
            KType::KFunctor {
                params: Record::from_pairs(vec![("x".into(), KType::Number)]),
                ret: Box::new(KType::Bool),
                body: None,
            },
            KType::KFunctor {
                params: Record::from_pairs(vec![("x".into(), KType::Number)]),
                ret: Box::new(KType::Bool),
                body: None,
            },
        ),
        (KType::OfKind(KKind::Tagged), KType::OfKind(KKind::Tagged)),
        (
            KType::RecursiveRef("Tree".into()),
            KType::RecursiveRef("Tree".into()),
        ),
        (KType::SetLocal(2), KType::SetLocal(2)),
    ];
    // A `SetRef` pair sharing one `Rc` — identity is `(set ptr, index)`, so the same
    // allocation must hash and compare equal.
    let shared = record_newtype_set("Point", sid);
    let set_ref_a = KType::SetRef {
        set: Rc::clone(&shared),
        index: 0,
    };
    let set_ref_b = KType::SetRef {
        set: Rc::clone(&shared),
        index: 0,
    };
    let pairs: Vec<(KType<'_>, KType<'_>)> = pairs
        .into_iter()
        .chain(std::iter::once((set_ref_a, set_ref_b)))
        .collect();
    for (a, b) in &pairs {
        assert_eq!(a, b, "values must be equal: {:?}", a);
        assert_eq!(
            hash_of(a),
            hash_of(b),
            "equal values must hash equal: {:?}",
            a
        );
    }
}

/// `SetRef` identity is `(set ptr, index)` and never descends the (cyclic) schema. Two
/// `SetRef`s over the same `Rc` allocation and index compare equal — so `Hash` must
/// agree. Two over *distinct* allocations of the same name compare unequal.
#[test]
fn hash_keys_set_ref_on_pointer_and_index() {
    let sid = ScopeId::from_raw(0, 0x1234);
    let set = record_newtype_set("Point", sid);
    let a = KType::SetRef {
        set: Rc::clone(&set),
        index: 0,
    };
    let b = KType::SetRef {
        set: Rc::clone(&set),
        index: 0,
    };
    assert_eq!(a, b);
    assert_eq!(hash_of(&a), hash_of(&b));

    // A separate allocation with the same name is a distinct identity.
    let other = record_newtype_set("Point", sid);
    let c = KType::SetRef {
        set: other,
        index: 0,
    };
    assert_ne!(a, c);
}

/// Distinct variants must not collide structurally — the leading discriminant
/// keeps e.g. `KFunction` and `KFunctor` of the same shape apart in both `Eq`
/// and `Hash`.
#[test]
fn hash_distinguishes_function_from_functor() {
    let f = KType::KFunction {
        params: Record::from_pairs(vec![("x".into(), KType::Number)]),
        ret: Box::new(KType::Bool),
    };
    let g = KType::KFunctor {
        params: Record::from_pairs(vec![("x".into(), KType::Number)]),
        ret: Box::new(KType::Bool),
        body: None,
    };
    assert_ne!(f, g);
    assert_ne!(hash_of(&f), hash_of(&g));
}

#[test]
fn set_ref_name_renders_member_name() {
    // Renders the member's declared `name`, not the kind keyword: a `Point` struct
    // slot shows `Point`, not `Struct`.
    let set = record_newtype_set("Point", ScopeId::from_raw(0, 0x1234));
    let t = KType::SetRef { set, index: 0 };
    assert_eq!(t.name(), "Point");
}
