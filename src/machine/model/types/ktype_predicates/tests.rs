use super::*;
use crate::machine::core::ScopeId;

#[test]
fn is_more_specific_concrete_beats_any() {
    assert!(KType::Number.is_more_specific_than(&KType::Any));
    assert!(!KType::Any.is_more_specific_than(&KType::Number));
}

#[test]
fn is_more_specific_list_number_beats_list_any() {
    let n = KType::List(Box::new(KType::Number));
    let a = KType::List(Box::new(KType::Any));
    assert!(n.is_more_specific_than(&a));
    assert!(!a.is_more_specific_than(&n));
}

#[test]
fn is_more_specific_disjoint_lists_incomparable() {
    let n = KType::List(Box::new(KType::Number));
    let s = KType::List(Box::new(KType::Str));
    assert!(!n.is_more_specific_than(&s));
    assert!(!s.is_more_specific_than(&n));
}

#[test]
fn is_more_specific_dict_refines_value() {
    let strict = KType::Dict(Box::new(KType::Str), Box::new(KType::Number));
    let loose = KType::Dict(Box::new(KType::Str), Box::new(KType::Any));
    assert!(strict.is_more_specific_than(&loose));
    assert!(!loose.is_more_specific_than(&strict));
}

#[test]
fn is_more_specific_function_arity_mismatch_incomparable() {
    let unary = KType::KFunction {
        args: vec![KType::Number],
        ret: Box::new(KType::Number),
    };
    let nullary = KType::KFunction {
        args: vec![],
        ret: Box::new(KType::Number),
    };
    assert!(!unary.is_more_specific_than(&nullary));
    assert!(!nullary.is_more_specific_than(&unary));
}

#[test]
fn mu_matches_value_via_one_unfold() {
    // Phase 1 cycle-gate: `Mu` matches via one unfold of its body. A `RecursiveRef`
    // inside that body accepts anything for now (phase 3 tightens).
    let t = KType::Mu {
        binder: "Tree".into(),
        body: Box::new(KType::List(Box::new(KType::RecursiveRef("Tree".into())))),
    };
    // Empty list — element type is unconstrained anyway.
    let v = KObject::List(vec![].into());
    assert!(t.matches_value(&v));
    // Non-list shouldn't pass through.
    assert!(!t.matches_value(&KObject::Number(1.0)));
}

#[test]
fn recursive_ref_accepts_anything_phase_one() {
    // Phase 1: `RecursiveRef` is a cycle gate that accepts every value. Phase 3
    // tightens this by threading the enclosing `Mu`'s body through the predicate.
    let t = KType::RecursiveRef("Tree".into());
    assert!(t.matches_value(&KObject::Number(1.0)));
    assert!(t.matches_value(&KObject::List(vec![].into())));
}

/// `AnyUserType { kind: Struct }` accepts `Future(KObject::Struct{..})` and rejects
/// carriers of other kinds (`Tagged`) or wholly different families (`Number`).
/// Anchors the wildcard predicate's family-filtering behavior — stage 3.0b will
/// flip `from_name("Struct")` to produce this variant, and dispatch tests using
/// `(PICK x: Struct)` must continue to accept any struct carrier.
#[test]
fn any_user_type_struct_accepts_struct_future_only() {
    use crate::machine::core::RuntimeArena;
    use indexmap::IndexMap;
    use std::rc::Rc;
    // Arena-allocate the carriers: `KObject` is invariant in its `'a` lifetime, so
    // stack locals trip dropck. Arena allocation hands out `&'a KObject<'a>` whose
    // lifetime is tied to the arena's, dodging the false-positive.
    let arena = RuntimeArena::new();
    let t = KType::AnyUserType { kind: UserTypeKind::Struct };
    let s: &KObject<'_> = arena.alloc_object(KObject::Struct {
        name: "Point".into(),
        scope_id: ScopeId::SENTINEL,
        fields: Rc::new(IndexMap::new()),
    });
    let tagged: &KObject<'_> = arena.alloc_object(KObject::Tagged {
        tag: "some".into(),
        value: Rc::new(KObject::Number(1.0)),
        scope_id: ScopeId::SENTINEL,
        name: "Maybe".into(),
    });
    let n: &KObject<'_> = arena.alloc_object(KObject::Number(1.0));
    assert!(t.accepts_part(&ExpressionPart::Future(s)));
    assert!(!t.accepts_part(&ExpressionPart::Future(tagged)));
    assert!(!t.accepts_part(&ExpressionPart::Future(n)));
}

/// Stage 4: a `Wrapped` value with a NEWTYPE identity fills both the wildcard
/// `AnyUserType { kind: Newtype { repr: <sentinel> } }` (the manual `PartialEq`
/// ignores `repr`) and the per-declaration `UserType { kind: Newtype, .. }` slot
/// of matching `(scope_id, name)`.
#[test]
fn any_user_type_newtype_accepts_wrapped_only() {
    use crate::machine::core::RuntimeArena;
    let arena = RuntimeArena::new();
    let t = KType::AnyUserType {
        kind: UserTypeKind::Newtype { repr: Box::new(KType::Any) },
    };
    let inner: &KObject<'_> = arena.alloc_object(KObject::Number(3.0));
    let type_id: &KType = arena.alloc_ktype(KType::UserType {
        kind: UserTypeKind::Newtype { repr: Box::new(KType::Number) },
        scope_id: ScopeId::from_raw(0, 0xAA),
        name: "Distance".into(),
    });
    let w: &KObject<'_> = arena.alloc_object(KObject::Wrapped {
        inner: crate::machine::model::values::NonWrappedRef::peel(inner),
        type_id,
    });
    let s: &KObject<'_> = arena.alloc_object(KObject::Struct {
        name: "Point".into(),
        scope_id: ScopeId::SENTINEL,
        fields: std::rc::Rc::new(indexmap::IndexMap::new()),
    });
    assert!(t.accepts_part(&ExpressionPart::Future(w)));
    assert!(!t.accepts_part(&ExpressionPart::Future(s)));
    assert!(t.matches_value(w));
    assert!(!t.matches_value(s));
}

/// Pins the wildcard refinement: `UserType { kind: Newtype { repr: <real> }, .. }`
/// is strictly more specific than `AnyUserType { kind: Newtype { repr: <sentinel> } }`,
/// and incomparable with `AnyUserType { kind: Struct }`.
#[test]
fn user_type_newtype_specificity_lattice() {
    let any_newtype = KType::AnyUserType {
        kind: UserTypeKind::Newtype { repr: Box::new(KType::Any) },
    };
    let any_struct = KType::AnyUserType { kind: UserTypeKind::Struct };
    let dist = KType::UserType {
        kind: UserTypeKind::Newtype { repr: Box::new(KType::Number) },
        scope_id: ScopeId::from_raw(0, 0xAA),
        name: "Distance".into(),
    };
    assert!(dist.is_more_specific_than(&any_newtype));
    assert!(!any_newtype.is_more_specific_than(&dist));
    assert!(!dist.is_more_specific_than(&any_struct));
    assert!(!any_struct.is_more_specific_than(&dist));
}

/// Specificity ordering for the new `UserType` / `AnyUserType` variants:
/// - `AnyUserType` is strictly under `Any` (handled by the top-level `Any` short-circuit).
/// - `UserType { kind: K, .. }` is strictly under `AnyUserType { kind: K }` (same kind).
/// - `UserType` of one kind and `AnyUserType` of a different kind are incomparable
///   (sibling families).
#[test]
fn user_type_specificity_lattice() {
    let any_struct = KType::AnyUserType { kind: UserTypeKind::Struct };
    let any_tagged = KType::AnyUserType { kind: UserTypeKind::Tagged };
    let point = KType::UserType {
        kind: UserTypeKind::Struct,
        scope_id: ScopeId::from_raw(0, 0xAA),
        name: "Point".into(),
    };
    // `AnyUserType` strictly under `Any`.
    assert!(any_struct.is_more_specific_than(&KType::Any));
    assert!(!KType::Any.is_more_specific_than(&any_struct));
    // `UserType { kind: Struct, .. }` strictly under `AnyUserType { kind: Struct }`.
    assert!(point.is_more_specific_than(&any_struct));
    assert!(!any_struct.is_more_specific_than(&point));
    // Different-kind pairs incomparable.
    assert!(!point.is_more_specific_than(&any_tagged));
    assert!(!any_tagged.is_more_specific_than(&point));
}

/// `is_type_denoting` returns `true` exactly for the variants enumerated in the
/// predicate's docstring — the parameters whose declared `KType` makes the bound
/// value's nominal identity meaningful at the type level. Anchors the dual-write
/// gate in [`crate::machine::core::kfunction::KFunction::invoke`].
#[test]
fn is_type_denoting_table() {
    // SignatureBound — module ascribed to a signature.
    let sb = KType::SignatureBound {
        sig_id: ScopeId::from_raw(0, 1),
        sig_path: "OrderedSig".into(),
        pinned_slots: Vec::new(),
    };
    assert!(sb.is_type_denoting());
    // SignatureBound with pins — still type-denoting.
    let sb_pinned = KType::SignatureBound {
        sig_id: ScopeId::from_raw(0, 1),
        sig_path: "OrderedSig".into(),
        pinned_slots: vec![("Type".into(), KType::Number)],
    };
    assert!(sb_pinned.is_type_denoting());
    // Signature — first-class signature value.
    assert!(KType::Signature.is_type_denoting());
    // Type — schema meta-type.
    assert!(KType::Type.is_type_denoting());
    // TypeExprRef — TypeExpr carrier.
    assert!(KType::TypeExprRef.is_type_denoting());
    // AnyUserType { kind: Module } — unascribed module wildcard.
    assert!(KType::AnyUserType { kind: UserTypeKind::Module }.is_type_denoting());
    // Sibling AnyUserType kinds are NOT type-denoting at the parameter level —
    // a STRUCT-typed parameter doesn't make its name a type-language binder.
    assert!(!KType::AnyUserType { kind: UserTypeKind::Struct }.is_type_denoting());
    assert!(!KType::AnyUserType { kind: UserTypeKind::Tagged }.is_type_denoting());
    // Per-declaration UserType is NOT type-denoting — the nominal identity already
    // lives in the declaring scope's `bindings.types`; rebinding per-call would
    // be a no-op (or worse, a shadow).
    let ut = KType::UserType {
        kind: UserTypeKind::Module,
        scope_id: ScopeId::from_raw(0, 1),
        name: "Foo".into(),
    };
    assert!(!ut.is_type_denoting());
    // Primitives, containers, function shapes — none denote types.
    assert!(!KType::Number.is_type_denoting());
    assert!(!KType::Str.is_type_denoting());
    assert!(!KType::Bool.is_type_denoting());
    assert!(!KType::Null.is_type_denoting());
    assert!(!KType::Any.is_type_denoting());
    assert!(!KType::Identifier.is_type_denoting());
    assert!(!KType::KExpression.is_type_denoting());
    assert!(!KType::List(Box::new(KType::Number)).is_type_denoting());
    assert!(!KType::Dict(
        Box::new(KType::Str),
        Box::new(KType::Number),
    )
    .is_type_denoting());
    assert!(!KType::KFunction {
        args: vec![KType::Number],
        ret: Box::new(KType::Number),
    }
    .is_type_denoting());
}

/// `SignatureBound { pinned_slots }` specificity rules:
/// - A non-empty `pinned_slots` strictly refines an empty same-`sig_id` form when
///   every pin in the empty side appears (with equal `KType`) in the non-empty side.
/// - Different `sig_id`s are incomparable.
/// - Same `sig_id` with disjoint constraint keys is incomparable.
/// - Same-key-different-`KType` is incomparable.
/// - A `SignatureBound` (pinned or not) strictly refines `AnyUserType { kind: Module }`.
#[test]
fn is_more_specific_for_pinned_signature_bound() {
    let bare = KType::SignatureBound {
        sig_id: ScopeId::from_raw(0, 1),
        sig_path: "OrderedSig".into(),
        pinned_slots: Vec::new(),
    };
    let pinned_number = KType::SignatureBound {
        sig_id: ScopeId::from_raw(0, 1),
        sig_path: "OrderedSig".into(),
        pinned_slots: vec![("Type".into(), KType::Number)],
    };
    let pinned_str = KType::SignatureBound {
        sig_id: ScopeId::from_raw(0, 1),
        sig_path: "OrderedSig".into(),
        pinned_slots: vec![("Type".into(), KType::Str)],
    };
    let pinned_two = KType::SignatureBound {
        sig_id: ScopeId::from_raw(0, 1),
        sig_path: "OrderedSig".into(),
        pinned_slots: vec![("Type".into(), KType::Number), ("Elt".into(), KType::Str)],
    };
    let other_sig = KType::SignatureBound {
        sig_id: ScopeId::from_raw(0, 2),
        sig_path: "HashedSig".into(),
        pinned_slots: vec![("Type".into(), KType::Number)],
    };
    let pinned_elt = KType::SignatureBound {
        sig_id: ScopeId::from_raw(0, 1),
        sig_path: "OrderedSig".into(),
        pinned_slots: vec![("Elt".into(), KType::Number)],
    };
    let any_module = KType::AnyUserType { kind: UserTypeKind::Module };

    // Pinned strictly more specific than bare same-sig.
    assert!(pinned_number.is_more_specific_than(&bare));
    assert!(!bare.is_more_specific_than(&pinned_number));
    // Two-pin extension of one-pin (covering pin) is strictly more specific.
    assert!(pinned_two.is_more_specific_than(&pinned_number));
    assert!(!pinned_number.is_more_specific_than(&pinned_two));
    // Same key, different KType → incomparable.
    assert!(!pinned_number.is_more_specific_than(&pinned_str));
    assert!(!pinned_str.is_more_specific_than(&pinned_number));
    // Disjoint constraint keys at same length → incomparable.
    assert!(!pinned_number.is_more_specific_than(&pinned_elt));
    assert!(!pinned_elt.is_more_specific_than(&pinned_number));
    // Different sig_ids → incomparable on the pinned path; both still refine
    // the AnyUserType { kind: Module } wildcard.
    assert!(!pinned_number.is_more_specific_than(&other_sig));
    assert!(!other_sig.is_more_specific_than(&pinned_number));
    // Any SignatureBound (pinned or not) refines `AnyUserType { kind: Module }`.
    assert!(bare.is_more_specific_than(&any_module));
    assert!(pinned_number.is_more_specific_than(&any_module));
    assert!(pinned_two.is_more_specific_than(&any_module));
}
