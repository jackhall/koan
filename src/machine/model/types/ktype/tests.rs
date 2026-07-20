use super::super::recursive_set::{NominalMember, NominalSchema};
use super::*;
use crate::builtins::default_scope;
use crate::machine::core::{run_root_storage, FrameStorageExt};
use crate::machine::model::Module;

/// A singleton `Rc<RecursiveSet>` over a record-repr newtype member named `name`, schema
/// filled.
fn record_newtype_set(name: &str, scope_id: ScopeId) -> Rc<RecursiveSet> {
    RecursiveSet::singleton(
        name.into(),
        scope_id,
        NominalSchema::NewType(Box::new(KType::record(Box::new(Record::new())))),
    )
}

#[test]
fn name_renders_parameterized_list() {
    let t = KType::list(Box::new(KType::list(Box::new(KType::Number))));
    assert_eq!(t.name(), ":(LIST OF :(LIST OF Number))");
}

#[test]
fn name_renders_dict() {
    let t = KType::dict(Box::new(KType::Str), Box::new(KType::Number));
    assert_eq!(t.name(), ":(MAP Str -> Number)");
}

#[test]
fn name_renders_function() {
    let t = KType::function_type(
        Record::from_pairs(vec![("x".into(), KType::Number), ("y".into(), KType::Str)]),
        Box::new(KType::Bool),
    );
    assert_eq!(t.name(), ":(FN (x :Number y :Str) -> Bool)");
}

/// A nested sigiled parameter type already opens with `:`, so the renderer must not
/// prefix a second colon (`xs :(LIST OF Number)`, not `xs ::(LIST OF Number)`).
#[test]
fn name_renders_function_with_sigiled_param() {
    let t = KType::function_type(
        Record::from_pairs(vec![("xs".into(), KType::list(Box::new(KType::Number)))]),
        Box::new(KType::Bool),
    );
    assert_eq!(t.name(), ":(FN (xs :(LIST OF Number)) -> Bool)");
}

#[test]
fn name_renders_function_nullary() {
    let t = KType::function_type(Record::new(), Box::new(KType::Any));
    assert_eq!(t.name(), ":(FN () -> Any)");
}

/// Function-slot identity is the record substrate's order-blind equality: the same
/// parameters by `(name, type)` in a different declaration order compare equal and
/// hash equal.
#[test]
fn function_params_order_blind_equality() {
    let xy = KType::function_type(
        Record::from_pairs(vec![("x".into(), KType::Number), ("y".into(), KType::Str)]),
        Box::new(KType::Bool),
    );
    let yx = KType::function_type(
        Record::from_pairs(vec![("y".into(), KType::Str), ("x".into(), KType::Number)]),
        Box::new(KType::Bool),
    );
    assert_eq!(xy, yx);
    assert_eq!(hash_of(&xy), hash_of(&yx));
}

/// Identity is name-sensitive: same type, different parameter name is a different
/// function type.
#[test]
fn function_params_name_sensitive_inequality() {
    let x = KType::function_type(
        Record::from_pairs(vec![("x".into(), KType::Number)]),
        Box::new(KType::Bool),
    );
    let a = KType::function_type(
        Record::from_pairs(vec![("a".into(), KType::Number)]),
        Box::new(KType::Bool),
    );
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

/// `:Module` lowers to the empty signature, which renders back as the `Module` surface keyword;
/// `:Signature` is the `OfKind` wildcard and renders as its own keyword.
#[test]
fn any_module_and_any_signature_render_surface_keywords() {
    let am: KType = KType::empty_signature();
    let asg: KType = KType::OfKind(KKind::Signature);
    assert_eq!(am.name(), "Module");
    assert_eq!(asg.name(), "Signature");
}

// --- KType::Union ------------------------------------------------------------------

/// `:(A | B)` renders members joined by ` | ` and wrapped in the type sigil.
#[test]
fn name_renders_union() {
    let u = KType::union_of(vec![KType::Number, KType::Str]);
    assert_eq!(u.name(), ":(Number | Str)");
}

/// A compound member already opens its own sigil, which nests without a doubled colon.
#[test]
fn name_renders_union_with_compound_member() {
    let u = KType::union_of(vec![KType::list(Box::new(KType::Number)), KType::Str]);
    assert_eq!(u.name(), ":(:(LIST OF Number) | Str)");
}

/// Union equality is order-blind: the same members in a different order compare equal.
#[test]
fn union_equality_order_blind() {
    let ab = KType::union_of(vec![KType::Number, KType::Str]);
    let ba = KType::union_of(vec![KType::Str, KType::Number]);
    assert_eq!(ab, ba);
}

/// Two unions of different member sets are unequal.
#[test]
fn union_inequality_different_members() {
    let ns = KType::union_of(vec![KType::Number, KType::Str]);
    let nb = KType::union_of(vec![KType::Number, KType::Bool]);
    assert_ne!(ns, nb);
}

/// Hash agrees with the order-blind equality: reordered-but-equal unions hash equal.
#[test]
fn union_hash_order_blind() {
    let ab = KType::union_of(vec![KType::Number, KType::Str, KType::Bool]);
    let ba = KType::union_of(vec![KType::Bool, KType::Number, KType::Str]);
    assert_eq!(ab, ba);
    assert_eq!(hash_of(&ab), hash_of(&ba));
}

fn hash_of(t: &KType) -> u64 {
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
    let pairs: Vec<(KType, KType)> = vec![
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
        (
            KType::OfKind(KKind::Signature),
            KType::OfKind(KKind::Signature),
        ),
        (
            KType::list(Box::new(KType::Number)),
            KType::list(Box::new(KType::Number)),
        ),
        (
            KType::dict(Box::new(KType::Str), Box::new(KType::Number)),
            KType::dict(Box::new(KType::Str), Box::new(KType::Number)),
        ),
        (
            KType::function_type(
                Record::from_pairs(vec![("x".into(), KType::Number)]),
                Box::new(KType::Bool),
            ),
            KType::function_type(
                Record::from_pairs(vec![("x".into(), KType::Number)]),
                Box::new(KType::Bool),
            ),
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
    let pairs: Vec<(KType, KType)> = pairs
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
fn set_ref_identity_unifies_by_content_digest() {
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

    // A separate allocation with the same content unifies: identity is the content digest,
    // not the allocation (content-addressed identity — structurally identical
    // declarations denote one type).
    let other = record_newtype_set("Point", sid);
    let c = KType::SetRef {
        set: other,
        index: 0,
    };
    assert_eq!(a, c);
    assert_eq!(hash_of(&a), hash_of(&c));

    // A different member name is different content, so it stays a distinct type.
    let line = record_newtype_set("Line", sid);
    let d = KType::SetRef {
        set: line,
        index: 0,
    };
    assert_ne!(a, d);
}

/// The two-phase window: before a set seals it has no digest, so `SetRef` identity falls to
/// the set pointer (the only path that answers "equal" pre-seal); once `fill_member` seals it,
/// the content-digest rule takes over and same-content sets in different allocations unify.
/// Koan source never compares a pre-seal `SetRef` from a *different* allocation (a pre-installed
/// identity stays confined to its declaring elaboration), so the pre-seal cross-allocation case
/// is pinned here at the Rust level.
#[test]
fn set_ref_pre_seal_window_pointer_then_digest() {
    let pending_pair = |session| {
        Rc::new(RecursiveSet::new(vec![
            NominalMember::pending("Aa".into(), ScopeId::from_raw(session, 1), KKind::NewType),
            NominalMember::pending("Bb".into(), ScopeId::from_raw(session, 2), KKind::NewType),
        ]))
    };
    let seal = |set: &Rc<RecursiveSet>| {
        set.fill_member(0, NominalSchema::NewType(Box::new(KType::Number)));
        set.fill_member(1, NominalSchema::NewType(Box::new(KType::Str)));
    };

    // Unsealed: pointer rule. Same set + index equal; same set + different index distinct.
    let set = pending_pair(1);
    assert!(set.digest().is_none());
    let a0 = KType::SetRef {
        set: Rc::clone(&set),
        index: 0,
    };
    let a0_again = KType::SetRef {
        set: Rc::clone(&set),
        index: 0,
    };
    let a1 = KType::SetRef {
        set: Rc::clone(&set),
        index: 1,
    };
    assert_eq!(a0, a0_again);
    assert_ne!(a0, a1);
    assert_eq!(hash_of(&a0), hash_of(&a0_again));

    // A SetRef into a *different* unsealed set has no digest to compare, so it is not equal.
    let other = pending_pair(1);
    let other0 = KType::SetRef {
        set: other,
        index: 0,
    };
    assert_ne!(a0, other0);

    // Seal both this set and an independently built same-content set: the digest rule now
    // governs and the two unify across allocations (the `session` half of each `scope_id`
    // differs, proving `scope_id` is excluded from identity).
    seal(&set);
    let twin = pending_pair(42);
    seal(&twin);
    assert!(set.digest().is_some() && twin.digest().is_some());
    let sealed_a0 = KType::SetRef {
        set: Rc::clone(&set),
        index: 0,
    };
    let twin0 = KType::SetRef {
        set: twin,
        index: 0,
    };
    assert_eq!(
        sealed_a0, twin0,
        "sealed same-content sets unify across allocations"
    );
    assert_eq!(hash_of(&sealed_a0), hash_of(&twin0));
}

#[test]
fn set_ref_name_renders_member_name() {
    // Renders the member's declared `name`, not the kind keyword: a `Point` struct
    // slot shows `Point`, not `Struct`.
    let set = record_newtype_set("Point", ScopeId::from_raw(0, 0x1234));
    let t = KType::SetRef { set, index: 0 };
    assert_eq!(t.name(), "Point");
}

/// `AbstractType` identity is `(source, name)`: two mints naming the same module and the same
/// abstract member compare (and hash) equal, while a mint against another module — what a second
/// `:|` application of the same SIG produces, since each ascription allocates a fresh child scope —
/// stays distinct. Renaming the member also separates them.
#[test]
fn abstract_type_identity_keys_on_source_and_name() {
    let storage = run_root_storage();
    // Each `:|` allocates its own child scope, so the two views carry distinct `ScopeId`s.
    let first = storage.brand().alloc_module(Module::new(
        "View".into(),
        default_scope(&storage, Box::new(std::io::sink())),
    ));
    let second = storage.brand().alloc_module(Module::new(
        "View".into(),
        default_scope(&storage, Box::new(std::io::sink())),
    ));
    assert_ne!(first.scope_id(), second.scope_id());

    let mint = |m: &Module<'_>, name: &str| KType::AbstractType {
        source: m.scope_id(),
        name: name.into(),
        param_names: Vec::new(),
    };

    assert_eq!(mint(first, "Carrier"), mint(first, "Carrier"));
    assert_eq!(
        hash_of(&mint(first, "Carrier")),
        hash_of(&mint(first, "Carrier"))
    );
    assert_ne!(mint(first, "Carrier"), mint(second, "Carrier"));
    assert_ne!(mint(first, "Carrier"), mint(first, "Elem"));
}
