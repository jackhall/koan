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
    let v = KObject::list(vec![]);
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
    assert!(t.matches_value(&KObject::list(vec![])));
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
    let s: &KObject<'_> = arena.alloc(KObject::Struct {
        name: "Point".into(),
        scope_id: ScopeId::SENTINEL,
        fields: Rc::new(IndexMap::new()),
    });
    let tagged: &KObject<'_> = arena.alloc(KObject::Tagged {
        tag: "some".into(),
        value: Rc::new(KObject::Number(1.0)),
        scope_id: ScopeId::SENTINEL,
        name: "Maybe".into(),
        type_args: Rc::new(vec![]),
    });
    let n: &KObject<'_> = arena.alloc(KObject::Number(1.0));
    assert!(t.accepts_part(&ExpressionPart::Future(s)));
    assert!(!t.accepts_part(&ExpressionPart::Future(tagged)));
    assert!(!t.accepts_part(&ExpressionPart::Future(n)));
}

/// Direct admission table for `KType::Type::accepts_part` post bare-type-token
/// widening. Pins the cut-(a) admission set without going through the
/// scheduler: bare builtin type tokens (`Number` / `Str` / `Bool` / `Null`)
/// and `TaggedUnionType` / `StructType` carriers admit; the cut-(a) wall on
/// `KTypeValue(KType::Module { .. })` / `KTypeValue(KType::Signature(_))`
/// rejects (so the `:Type` vs `:Module` overload distinction stays intact);
/// non-type-denoting `Future(_)` carriers (a raw `Number(7)` literal, a
/// `KString`) reject. The `ExpressionPart::Type(_)` branch is exercised
/// implicitly by every FN/FUNCTOR call test that passes a `TypeExpr` in a
/// `:Type` slot — this test pins the carrier-bearing arm of the admission
/// table that has no end-to-end counterpart short of `[FUNCTOR ... :Type]`
/// drivers.
#[test]
fn type_slot_admits_bare_builtin_tokens_and_user_type_carriers() {
    use crate::builtins::default_scope;
    use crate::machine::core::RuntimeArena;
    use crate::machine::model::values::{Module, Signature};
    use std::collections::HashMap;
    use std::rc::Rc;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let t = KType::Type;
    // Admit: bare builtin type tokens carried as `KTypeValue(KType::<prim>)`.
    let kt_number: &KObject<'_> = arena.alloc(KObject::KTypeValue(KType::Number));
    let kt_str: &KObject<'_> = arena.alloc(KObject::KTypeValue(KType::Str));
    let kt_bool: &KObject<'_> = arena.alloc(KObject::KTypeValue(KType::Bool));
    let kt_null: &KObject<'_> = arena.alloc(KObject::KTypeValue(KType::Null));
    assert!(t.accepts_part(&ExpressionPart::Future(kt_number)));
    assert!(t.accepts_part(&ExpressionPart::Future(kt_str)));
    assert!(t.accepts_part(&ExpressionPart::Future(kt_bool)));
    assert!(t.accepts_part(&ExpressionPart::Future(kt_null)));
    // Admit: tagged-union and struct schema carriers (Type-denoting via the
    // `TaggedUnionType` / `StructType` arms — these carriers ride directly,
    // not wrapped in `KTypeValue`).
    let tagged_schema: &KObject<'_> = arena.alloc(KObject::TaggedUnionType {
        schema: Rc::new(HashMap::new()),
        name: "Maybe".into(),
        scope_id: ScopeId::SENTINEL,
    });
    let struct_schema: &KObject<'_> = arena.alloc(KObject::StructType {
        name: "Point".into(),
        scope_id: ScopeId::SENTINEL,
        fields: Rc::new(Vec::new()),
    });
    assert!(t.accepts_part(&ExpressionPart::Future(tagged_schema)));
    assert!(t.accepts_part(&ExpressionPart::Future(struct_schema)));
    // Cut-(a) wall: module carrier rejects. `:Module` / `:SatisfiesSignature`
    // catch module values; admitting them here would collapse the wall.
    let child = arena.alloc_scope(crate::machine::Scope::child_under_module(
        scope,
        "IntMod".into(),
    ));
    let module = arena.alloc_module(Module::new("IntMod".into(), child));
    let kt_module: &KObject<'_> =
        arena.alloc(KObject::KTypeValue(KType::Module { module, frame: None }));
    assert!(!t.accepts_part(&ExpressionPart::Future(kt_module)));
    // Cut-(a) wall: signature carrier rejects. `:Signature` (`AnySignature`)
    // catches signature values.
    let sig = arena.alloc_signature(Signature::new("OrderedSig".into(), scope));
    let kt_sig: &KObject<'_> = arena.alloc(KObject::KTypeValue(KType::Signature(sig)));
    assert!(!t.accepts_part(&ExpressionPart::Future(kt_sig)));
    // Non-type carriers reject: a `Number(7)` literal (a value, not a type
    // token) and a `KString` (a string literal) both fall through.
    let n: &KObject<'_> = arena.alloc(KObject::Number(7.0));
    let s: &KObject<'_> = arena.alloc(KObject::KString("hi".into()));
    assert!(!t.accepts_part(&ExpressionPart::Future(n)));
    assert!(!t.accepts_part(&ExpressionPart::Future(s)));
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
    let inner: &KObject<'_> = arena.alloc(KObject::Number(3.0));
    let type_id: &KType = arena.alloc(KType::UserType {
        kind: UserTypeKind::Newtype { repr: Box::new(KType::Number) },
        scope_id: ScopeId::from_raw(0, 0xAA),
        name: "Distance".into(),
    });
    let w: &KObject<'_> = arena.alloc(KObject::Wrapped {
        inner: crate::machine::model::values::NonWrappedRef::peel(inner),
        type_id,
    });
    let s: &KObject<'_> = arena.alloc(KObject::Struct {
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
    // SatisfiesSignature — module ascribed to a signature.
    let sb = KType::SatisfiesSignature {
        sig_id: ScopeId::from_raw(0, 1),
        sig_path: "OrderedSig".into(),
        pinned_slots: Vec::new(),
    };
    assert!(sb.is_type_denoting());
    // SatisfiesSignature with pins — still type-denoting.
    let sb_pinned = KType::SatisfiesSignature {
        sig_id: ScopeId::from_raw(0, 1),
        sig_path: "OrderedSig".into(),
        pinned_slots: vec![("Type".into(), KType::Number)],
    };
    assert!(sb_pinned.is_type_denoting());
    // `:Signature` slot wildcard — admits first-class signature values.
    assert!(KType::AnySignature.is_type_denoting());
    // Type — schema meta-type.
    assert!(KType::Type.is_type_denoting());
    // TypeExprRef — TypeExpr carrier.
    assert!(KType::TypeExprRef.is_type_denoting());
    // `:Module` slot wildcard — admits any first-class module value.
    assert!(KType::AnyModule.is_type_denoting());
    // Sibling AnyUserType kinds are NOT type-denoting at the parameter level —
    // a STRUCT-typed parameter doesn't make its name a type-language binder.
    assert!(!KType::AnyUserType { kind: UserTypeKind::Struct }.is_type_denoting());
    assert!(!KType::AnyUserType { kind: UserTypeKind::Tagged }.is_type_denoting());
    // Per-declaration UserType (Struct / Tagged / Newtype / TypeConstructor) is NOT
    // type-denoting — the nominal identity already lives in the declaring scope's
    // `bindings.types`; rebinding per-call would be a no-op (or worse, a shadow).
    let ut = KType::UserType {
        kind: UserTypeKind::Struct,
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

/// `SatisfiesSignature { pinned_slots }` specificity rules:
/// - A non-empty `pinned_slots` strictly refines an empty same-`sig_id` form when
///   every pin in the empty side appears (with equal `KType`) in the non-empty side.
/// - Different `sig_id`s are incomparable.
/// - Same `sig_id` with disjoint constraint keys is incomparable.
/// - Same-key-different-`KType` is incomparable.
/// - A `SatisfiesSignature` (pinned or not) strictly refines `AnyUserType { kind: Module }`.
#[test]
fn is_more_specific_for_pinned_signature_bound() {
    let bare = KType::SatisfiesSignature {
        sig_id: ScopeId::from_raw(0, 1),
        sig_path: "OrderedSig".into(),
        pinned_slots: Vec::new(),
    };
    let pinned_number = KType::SatisfiesSignature {
        sig_id: ScopeId::from_raw(0, 1),
        sig_path: "OrderedSig".into(),
        pinned_slots: vec![("Type".into(), KType::Number)],
    };
    let pinned_str = KType::SatisfiesSignature {
        sig_id: ScopeId::from_raw(0, 1),
        sig_path: "OrderedSig".into(),
        pinned_slots: vec![("Type".into(), KType::Str)],
    };
    let pinned_two = KType::SatisfiesSignature {
        sig_id: ScopeId::from_raw(0, 1),
        sig_path: "OrderedSig".into(),
        pinned_slots: vec![("Type".into(), KType::Number), ("Elt".into(), KType::Str)],
    };
    let other_sig = KType::SatisfiesSignature {
        sig_id: ScopeId::from_raw(0, 2),
        sig_path: "HashedSig".into(),
        pinned_slots: vec![("Type".into(), KType::Number)],
    };
    let pinned_elt = KType::SatisfiesSignature {
        sig_id: ScopeId::from_raw(0, 1),
        sig_path: "OrderedSig".into(),
        pinned_slots: vec![("Elt".into(), KType::Number)],
    };
    let any_module = KType::AnyModule;

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
    // Any SatisfiesSignature (pinned or not) refines `AnyUserType { kind: Module }`.
    assert!(bare.is_more_specific_than(&any_module));
    assert!(pinned_number.is_more_specific_than(&any_module));
    assert!(pinned_two.is_more_specific_than(&any_module));
}

/// Build a `Result`-named `Tagged` value occupying `tag` with `payload`. `result_sid` is
/// the declaring scope id; the inner `payload` is itself a `Tagged` carrier whose name is
/// the error type's nominal identity.
fn result_value<'a>(result_sid: ScopeId, tag: &str, payload: KObject<'a>) -> KObject<'a> {
    KObject::Tagged {
        tag: tag.into(),
        value: std::rc::Rc::new(payload),
        scope_id: result_sid,
        name: "Result".into(),
        type_args: std::rc::Rc::new(vec![]),
    }
}

/// A bare error carrier (`Tagged` named `error_name`) standing in for a caught error
/// value. `ktype()` reports `UserType { kind: Tagged, scope_id, name }`.
fn error_carrier<'a>(error_sid: ScopeId, error_name: &str) -> KObject<'a> {
    KObject::Tagged {
        tag: "_".into(),
        value: std::rc::Rc::new(KObject::Number(0.0)),
        scope_id: error_sid,
        name: error_name.into(),
        type_args: std::rc::Rc::new(vec![]),
    }
}

/// `:(Result T E)` slot admission: a `ConstructorApply` slot whose ctor identity matches
/// the `Result` carrier admits an `error(...)` value iff the inhabited `error` payload
/// (param index 1) satisfies the slot's `E`. A caught `error(KError)` is rejected where
/// `E = MyErr` and accepted where `E = KError` / `Any`.
#[test]
fn constructor_apply_result_checks_inhabited_error_param() {
    let result_sid = ScopeId::from_raw(0, 0x9001);
    let kerror_sid = ScopeId::from_raw(0, 0x9002);
    let myerr_sid = ScopeId::from_raw(0, 0x9003);

    let ctor = Box::new(KType::UserType {
        kind: UserTypeKind::TypeConstructor { param_names: vec!["T".into(), "E".into()] },
        scope_id: result_sid,
        name: "Result".into(),
    });
    let myerr_ty = KType::UserType {
        kind: UserTypeKind::Tagged,
        scope_id: myerr_sid,
        name: "MyErr".into(),
    };
    let kerror_ty = KType::UserType {
        kind: UserTypeKind::Tagged,
        scope_id: kerror_sid,
        name: "KError".into(),
    };

    // Slot `:(Result Any MyErr)`.
    let slot_myerr = KType::ConstructorApply {
        ctor: ctor.clone(),
        args: vec![KType::Any, myerr_ty.clone()],
    };
    // A caught `error(KError)` value.
    let caught = result_value(result_sid, "error", error_carrier(kerror_sid, "KError"));
    // KError error is NOT a MyErr — rejected.
    assert!(!slot_myerr.matches_value(&caught));

    // Same value admitted where `E = KError`.
    let slot_kerror = KType::ConstructorApply {
        ctor: ctor.clone(),
        args: vec![KType::Any, kerror_ty.clone()],
    };
    assert!(slot_kerror.matches_value(&caught));

    // A `MyErr` error value satisfies `:(Result Any MyErr)`.
    let my_error = result_value(result_sid, "error", error_carrier(myerr_sid, "MyErr"));
    assert!(slot_myerr.matches_value(&my_error));
}

/// The `ok` field maps to param 0, so `:(Result Number E)` checks the `ok` payload
/// against `Number` regardless of `E`: an `ok(42)` value admits any `E` (the absent
/// `error` parameter is unconstrained at the value).
#[test]
fn constructor_apply_result_ok_admits_any_error_param() {
    let result_sid = ScopeId::from_raw(0, 0x9001);
    let myerr_sid = ScopeId::from_raw(0, 0x9003);
    let ctor = Box::new(KType::UserType {
        kind: UserTypeKind::TypeConstructor { param_names: vec!["T".into(), "E".into()] },
        scope_id: result_sid,
        name: "Result".into(),
    });
    let myerr_ty = KType::UserType {
        kind: UserTypeKind::Tagged,
        scope_id: myerr_sid,
        name: "MyErr".into(),
    };
    // `ok(42)` value.
    let ok_value = result_value(result_sid, "ok", KObject::Number(42.0));
    // `:(Result Number MyErr)` — ok payload is Number, error side unoccupied.
    let slot = KType::ConstructorApply {
        ctor: ctor.clone(),
        args: vec![KType::Number, myerr_ty],
    };
    assert!(slot.matches_value(&ok_value));
    // `:(Result Str MyErr)` rejects — ok payload Number is not Str.
    let slot_str = KType::ConstructorApply {
        ctor,
        args: vec![KType::Str, KType::Any],
    };
    assert!(!slot_str.matches_value(&ok_value));
}

/// `result_field_param_index` is the field→param linkage source of truth: `ok`→0,
/// `error`→1, `None` for any other carrier or tag.
#[test]
fn result_field_param_index_table() {
    assert_eq!(super::result_field_param_index("Result", "ok"), Some(0));
    assert_eq!(super::result_field_param_index("Result", "error"), Some(1));
    assert_eq!(super::result_field_param_index("Result", "other"), None);
    assert_eq!(super::result_field_param_index("Maybe", "ok"), None);
}

/// Phase 6 covariance for `ConstructorApply` carriers: a `Result<Number, MyErr>` value is
/// admitted by the coarser `:(Result Any Any)` slot (covariant in every arg), and the
/// refined `:(Result Number MyErr)` slot is strictly more specific than the coarse one, so
/// dispatch tie-breaks toward the refined overload.
#[test]
fn constructor_apply_covariant_admission_and_specificity() {
    let result_sid = ScopeId::from_raw(0, 0x9001);
    let myerr_sid = ScopeId::from_raw(0, 0x9003);
    let ctor = Box::new(KType::UserType {
        kind: UserTypeKind::TypeConstructor { param_names: vec!["T".into(), "E".into()] },
        scope_id: result_sid,
        name: "Result".into(),
    });
    let myerr = KType::UserType {
        kind: UserTypeKind::Tagged,
        scope_id: myerr_sid,
        name: "MyErr".into(),
    };
    // Value stamped `Result<Number, MyErr>`.
    let stamped = KObject::Tagged {
        tag: "ok".into(),
        value: std::rc::Rc::new(KObject::Number(1.0)),
        scope_id: result_sid,
        name: "Result".into(),
        type_args: std::rc::Rc::new(vec![KType::Number, myerr.clone()]),
    };
    let coarse = KType::ConstructorApply {
        ctor: ctor.clone(),
        args: vec![KType::Any, KType::Any],
    };
    let refined = KType::ConstructorApply {
        ctor,
        args: vec![KType::Number, myerr],
    };
    // Covariant admission: the coarse slot admits the precise value.
    assert!(coarse.matches_value(&stamped));
    assert!(refined.matches_value(&stamped));
    // Refined strictly more specific than coarse.
    assert!(refined.is_more_specific_than(&coarse));
    assert!(!coarse.is_more_specific_than(&refined));
}

/// A populated `type_args` carrier (stamped by ascription) is checked structurally against
/// the slot args, taking precedence over the inhabited-tag path.
#[test]
fn constructor_apply_stamped_type_args_checked_structurally() {
    let result_sid = ScopeId::from_raw(0, 0x9001);
    let ctor = Box::new(KType::UserType {
        kind: UserTypeKind::TypeConstructor { param_names: vec!["T".into(), "E".into()] },
        scope_id: result_sid,
        name: "Result".into(),
    });
    // A value stamped `Result<Number, Str>`.
    let stamped = KObject::Tagged {
        tag: "ok".into(),
        value: std::rc::Rc::new(KObject::Number(1.0)),
        scope_id: result_sid,
        name: "Result".into(),
        type_args: std::rc::Rc::new(vec![KType::Number, KType::Str]),
    };
    // Matches an identical slot.
    let slot_ok = KType::ConstructorApply {
        ctor: ctor.clone(),
        args: vec![KType::Number, KType::Str],
    };
    assert!(slot_ok.matches_value(&stamped));
    // `Any` args admit (covariant coarsening at the slot).
    let slot_any = KType::ConstructorApply {
        ctor: ctor.clone(),
        args: vec![KType::Any, KType::Any],
    };
    assert!(slot_any.matches_value(&stamped));
    // Mismatched arg rejects.
    let slot_bad = KType::ConstructorApply {
        ctor,
        args: vec![KType::Bool, KType::Str],
    };
    assert!(!slot_bad.matches_value(&stamped));
}
