use super::super::recursive_set::{NominalMember, NominalSchema};
use super::*;
use crate::builtins::default_scope;
use crate::machine::core::kfunction::Body;
use crate::machine::core::{run_root_storage, FrameStorageExt};
use crate::machine::model::ast::KExpression;
use crate::machine::model::types::{ExpressionSignature, ReturnType};

/// A singleton `Rc<RecursiveSet>` over a record-repr newtype member named `name`, schema
/// filled.
fn record_newtype_set<'a>(name: &str, scope_id: ScopeId) -> Rc<RecursiveSet<'a>> {
    let member = NominalMember::pending(name.into(), scope_id, KKind::NewType);
    member.fill(NominalSchema::NewType(Box::new(KType::Record(Box::new(
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
    assert_eq!(KKind::NewType.surface_keyword(), "NewType");
    assert_eq!(KKind::TypeConstructor.surface_keyword(), "TypeConstructor",);
}

#[test]
fn nominal_of_kind_name_renders_family_keyword() {
    assert_eq!(KType::OfKind(KKind::NewType).name(), "NewType");
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

// --- KType::Union ------------------------------------------------------------------

/// `:(A | B)` renders members joined by ` | ` and wrapped in the type sigil.
#[test]
fn name_renders_union() {
    let u = KType::Union(vec![KType::Number, KType::Str]);
    assert_eq!(u.name(), ":(Number | Str)");
}

/// A compound member already opens its own sigil, which nests without a doubled colon.
#[test]
fn name_renders_union_with_compound_member() {
    let u = KType::Union(vec![KType::List(Box::new(KType::Number)), KType::Str]);
    assert_eq!(u.name(), ":(:(LIST OF Number) | Str)");
}

/// Union equality is order-blind: the same members in a different order compare equal.
#[test]
fn union_equality_order_blind() {
    let ab = KType::Union(vec![KType::Number, KType::Str]);
    let ba = KType::Union(vec![KType::Str, KType::Number]);
    assert_eq!(ab, ba);
}

/// Two unions of different member sets are unequal.
#[test]
fn union_inequality_different_members() {
    let ns = KType::Union(vec![KType::Number, KType::Str]);
    let nb = KType::Union(vec![KType::Number, KType::Bool]);
    assert_ne!(ns, nb);
}

/// Hash agrees with the order-blind equality: reordered-but-equal unions hash equal.
#[test]
fn union_hash_order_blind() {
    let ab = KType::Union(vec![KType::Number, KType::Str, KType::Bool]);
    let ba = KType::Union(vec![KType::Bool, KType::Number, KType::Str]);
    assert_eq!(ab, ba);
    assert_eq!(hash_of(&ab), hash_of(&ba));
}

/// A region-free union rebuilds at `'static` member-wise.
#[test]
fn to_static_rebuilds_union() {
    let u = KType::Union(vec![KType::Number, KType::Str]);
    assert_eq!(
        u.to_static().expect("union of owned members rebuilds"),
        KType::Union(vec![KType::Number, KType::Str])
    );
}

fn hash_of(t: &KType<'_>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    t.hash(&mut h);
    h.finish()
}

/// `a == b` ⟹ `hash(a) == hash(b)` across every region-free variant. Each pair is
/// built independently so a stray identity-from-pointer bug would surface.
#[test]
fn hash_agrees_with_eq_for_region_free_variants() {
    let sid = ScopeId::from_raw(0, 0xBEEF);
    let pairs: Vec<(KType<'_>, KType<'_>)> = vec![
        (KType::Number, KType::Number),
        (KType::Str, KType::Str),
        (KType::Bool, KType::Bool),
        (KType::Null, KType::Null),
        (KType::Identifier, KType::Identifier),
        (KType::KExpression, KType::KExpression),
        (
            KType::OfKind(KKind::ProperType),
            KType::OfKind(KKind::ProperType),
        ),
        (KType::OfKind(KKind::AnyType), KType::OfKind(KKind::AnyType)),
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
        (KType::OfKind(KKind::NewType), KType::OfKind(KKind::NewType)),
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

// --- KType::to_static -------------------------------------------------------------

/// Every owned leaf (no boxed/nested payload) rebuilds at `'static` by clone/copy,
/// round-tripping through the same surface rendering.
#[test]
fn to_static_rebuilds_owned_leaves() {
    let leaves: Vec<KType<'_>> = vec![
        KType::Number,
        KType::Str,
        KType::Bool,
        KType::Null,
        KType::Identifier,
        KType::KExpression,
        KType::SigiledTypeExpr,
        KType::RecordType,
        KType::Any,
        KType::SetLocal(3),
        KType::RecursiveRef("Tree".into()),
        KType::Unresolved(TypeIdentifier::leaf("Foo".into())),
        KType::OfKind(KKind::NewType),
        KType::DeferredReturn(DeferredReturnSurface::Expression("expr".into())),
    ];
    for leaf in &leaves {
        let rebuilt = leaf.to_static().expect("owned leaf rebuilds at 'static");
        assert_eq!(rebuilt.name(), leaf.name());
    }
}

/// `AbstractType { source: Sig(_), .. }` is owned (no `&Module`), so it rebuilds too.
#[test]
fn to_static_rebuilds_abstract_type_sig_source() {
    let t = KType::AbstractType {
        source: AbstractSource::Sig(ScopeId::from_raw(0, 42)),
        name: "Carrier".into(),
    };
    let rebuilt = t
        .to_static()
        .expect("Sig-sourced AbstractType holds no region pointer");
    assert_eq!(
        rebuilt,
        KType::AbstractType {
            source: AbstractSource::Sig(ScopeId::from_raw(0, 42)),
            name: "Carrier".into(),
        }
    );
}

/// Nested container variants (`List`, `Dict`, `Record`) recurse into their owned
/// children and propagate the rebuild.
#[test]
fn to_static_rebuilds_nested_containers() {
    let list = KType::List(Box::new(KType::Dict(
        Box::new(KType::Str),
        Box::new(KType::Number),
    )));
    assert_eq!(
        list.to_static().expect("nested owned containers rebuild"),
        KType::List(Box::new(KType::Dict(
            Box::new(KType::Str),
            Box::new(KType::Number)
        )))
    );

    let record = KType::Record(Box::new(Record::from_pairs(vec![(
        "x".into(),
        KType::Number,
    )])));
    assert_eq!(
        record.to_static().expect("record-type fields rebuild"),
        KType::Record(Box::new(Record::from_pairs(vec![(
            "x".into(),
            KType::Number
        )])))
    );
}

/// `KFunction` (always owned) and a bodyless `KFunctor` both recurse `params`/`ret`.
#[test]
fn to_static_rebuilds_function_and_bodyless_functor() {
    let f = KType::KFunction {
        params: Record::from_pairs(vec![("x".into(), KType::Number)]),
        ret: Box::new(KType::Bool),
    };
    assert_eq!(
        f.to_static().expect("KFunction is owned"),
        KType::KFunction {
            params: Record::from_pairs(vec![("x".into(), KType::Number)]),
            ret: Box::new(KType::Bool),
        }
    );

    let g = KType::KFunctor {
        params: Record::from_pairs(vec![("x".into(), KType::Number)]),
        ret: Box::new(KType::Bool),
        body: None,
    };
    assert_eq!(
        g.to_static().expect("bodyless KFunctor is owned"),
        KType::KFunctor {
            params: Record::from_pairs(vec![("x".into(), KType::Number)]),
            ret: Box::new(KType::Bool),
            body: None,
        }
    );
}

/// `ConstructorApply` recurses `ctor` and every element of `args`.
#[test]
fn to_static_rebuilds_constructor_apply() {
    let t = KType::ConstructorApply {
        ctor: Box::new(KType::Any),
        args: vec![KType::Number, KType::Str],
    };
    assert_eq!(
        t.to_static()
            .expect("ConstructorApply over owned args rebuilds"),
        KType::ConstructorApply {
            ctor: Box::new(KType::Any),
            args: vec![KType::Number, KType::Str],
        }
    );
}

/// `Module { module }` holds a live `&'a Module` region pointer -> `None`. `Module` /
/// `ModuleSignature` / `KFunction` are region-pinned (`Scope<'a>`'s fields make them
/// self-referential), so — matching every other fixture in this crate that needs one
/// (e.g. `ktype_predicates/tests.rs`, `kfunction/tests.rs`) — they are built through the
/// region brand rather than as bare stack locals.
#[test]
fn to_static_none_for_module_borrow() {
    let storage = run_root_storage();
    let scope = default_scope(&storage, Box::new(std::io::sink()));
    let module = storage
        .brand()
        .alloc_module(Module::new("Test".into(), scope));
    let t = KType::Module { module };
    assert!(t.to_static().is_none());
}

/// `Signature { sig, .. }` holds a live `&'a ModuleSignature` region pointer -> `None`,
/// even with an otherwise-owned (empty) `pinned_slots`.
#[test]
fn to_static_none_for_signature_borrow() {
    let storage = run_root_storage();
    let scope = default_scope(&storage, Box::new(std::io::sink()));
    let sig = storage
        .brand()
        .alloc_signature(ModuleSignature::new("Sig".into(), scope));
    let t = KType::Signature {
        sig: SigSource::Declared(sig),
        pinned_slots: Vec::new(),
    };
    assert!(t.to_static().is_none());
}

/// `AbstractType { source: Module(_), .. }` holds a live `&'a Module` -> `None`, unlike
/// the `Sig(_)`-sourced case above.
#[test]
fn to_static_none_for_abstract_type_module_source() {
    let storage = run_root_storage();
    let scope = default_scope(&storage, Box::new(std::io::sink()));
    let module = storage
        .brand()
        .alloc_module(Module::new("Test".into(), scope));
    let t = KType::AbstractType {
        source: AbstractSource::Module(module),
        name: "Carrier".into(),
    };
    assert!(t.to_static().is_none());
}

/// A bound functor value's `body: Some(&'a KFunction)` is a live region pointer -> `None`,
/// even though `body` is identity-inert for `Eq`/`Hash`.
#[test]
fn to_static_none_for_functor_with_body() {
    let storage = run_root_storage();
    let scope = default_scope(&storage, Box::new(std::io::sink()));
    let sig = ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Number),
        elements: Vec::new(),
    };
    let func = storage.brand().alloc_function(KFunction::new(
        sig,
        Body::UserDefined(KExpression::new(Vec::new())),
        scope,
        None,
        None,
        true,
    ));
    let t = KType::KFunctor {
        params: Record::new(),
        ret: Box::new(KType::Number),
        body: Some(func),
    };
    assert!(t.to_static().is_none());
}

/// `SetRef` shares its schema by `Rc`, compared by `Rc::ptr_eq` — rebuilding it would
/// mint a distinct allocation and silently break nominal identity, so `to_static`
/// declines unconditionally (even though every member here is otherwise pure).
#[test]
fn to_static_none_for_set_ref_rc_shared() {
    let set = record_newtype_set("Point", ScopeId::from_raw(0, 0x9999));
    let t = KType::SetRef { set, index: 0 };
    assert!(t.to_static().is_none());
}

// --- KType::resident_in / resident_in_reach --------------------------------------

/// A `Module` allocated into `dest`'s own region is dest-resident.
#[test]
fn resident_in_true_for_same_region_module() {
    let storage = run_root_storage();
    let scope = default_scope(&storage, Box::new(std::io::sink()));
    let module = storage
        .brand()
        .alloc_module(Module::new("Test".into(), scope));
    let t = KType::Module { module };
    assert!(t.resident_in(storage.region()));
}

/// A `Module` allocated into a foreign region is not resident in an unrelated `dest`.
#[test]
fn resident_in_false_for_foreign_region_module() {
    let foreign = run_root_storage();
    let foreign_scope = default_scope(&foreign, Box::new(std::io::sink()));
    let module = foreign
        .brand()
        .alloc_module(Module::new("Test".into(), foreign_scope));
    let t = KType::Module { module };

    let dest = run_root_storage();
    assert!(!t.resident_in(dest.region()));
}

/// An `Rc`-shared `SetRef` whose every member schema is owned data is resident in any `dest` —
/// the checked path's whole reason to exist for the identity-preserving set family.
#[test]
fn resident_in_true_for_pure_set_ref() {
    let set = record_newtype_set("Point", ScopeId::from_raw(0, 0xA1));
    let t = KType::SetRef { set, index: 0 };
    let dest = run_root_storage();
    assert!(t.resident_in(dest.region()));
}

/// A `SetRef` whose member schema embeds a `KType::Signature` pointing into a foreign region is
/// not resident in an unrelated `dest` — the walk descends into member schemas, not just the
/// set's own (foreign-agnostic) `Rc` identity.
#[test]
fn resident_in_false_for_set_with_foreign_signature_member() {
    let sig_storage = run_root_storage();
    let sig_scope = default_scope(&sig_storage, Box::new(std::io::sink()));
    let sig = sig_storage
        .brand()
        .alloc_signature(ModuleSignature::new("Sig".into(), sig_scope));

    let member = NominalMember::pending("Wrap".into(), ScopeId::from_raw(0, 0xA2), KKind::NewType);
    member.fill(NominalSchema::NewType(Box::new(KType::Signature {
        sig: SigSource::Declared(sig),
        pinned_slots: Vec::new(),
    })));
    let set = Rc::new(RecursiveSet::new(vec![member]));
    let t = KType::SetRef { set, index: 0 };

    let dest = run_root_storage();
    assert!(!t.resident_in(dest.region()));
}

/// [`KType::resident_in_reach`] widens the dest-only check: a `Module` foreign to `dest` but
/// named by `reach`'s evidence is resident.
#[test]
fn resident_in_reach_true_when_evidence_covers_foreign_module() {
    use crate::machine::core::FrameSet;

    let foreign = run_root_storage();
    let foreign_scope = default_scope(&foreign, Box::new(std::io::sink()));
    let module = foreign
        .brand()
        .alloc_module(Module::new("Test".into(), foreign_scope));
    let t = KType::Module { module };

    let dest = run_root_storage();
    assert!(
        !t.resident_in(dest.region()),
        "sanity: not resident without evidence"
    );

    let foreign_reach = FrameSet::singleton(Rc::clone(&foreign));
    assert!(t.resident_in_reach(dest.region(), &[&foreign_reach]));
}
