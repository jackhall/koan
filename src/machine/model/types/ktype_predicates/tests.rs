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
    let t = KType::Mu {
        binder: "Tree".into(),
        body: Box::new(KType::List(Box::new(KType::RecursiveRef("Tree".into())))),
    };
    let v = KObject::list(vec![]);
    assert!(t.matches_value(&v));
    assert!(!t.matches_value(&KObject::Number(1.0)));
}

#[test]
fn recursive_ref_accepts_anything() {
    let t = KType::RecursiveRef("Tree".into());
    assert!(t.matches_value(&KObject::Number(1.0)));
    assert!(t.matches_value(&KObject::list(vec![])));
}

#[test]
fn any_user_type_struct_accepts_struct_future_only() {
    use crate::machine::core::RuntimeArena;
    use indexmap::IndexMap;
    use std::rc::Rc;
    // `KObject` is invariant in `'a`, so stack locals trip dropck; arena
    // allocation hands out `&'a KObject<'a>` tied to the arena's lifetime.
    let arena = RuntimeArena::new();
    let t = KType::AnyUserType {
        kind: UserTypeKind::struct_sentinel(),
    };
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

/// Admission table for `KType::Type::accepts_part`: bare builtin type tokens
/// and struct / union `KTypeValue(UserType)` identities admit; module and signature
/// carriers reject so the `:Type` vs `:Module` / `:Signature` overload distinction
/// stays intact; non-type-denoting carriers reject.
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
    let kt_number: &KObject<'_> = arena.alloc(KObject::KTypeValue(KType::Number));
    let kt_str: &KObject<'_> = arena.alloc(KObject::KTypeValue(KType::Str));
    let kt_bool: &KObject<'_> = arena.alloc(KObject::KTypeValue(KType::Bool));
    let kt_null: &KObject<'_> = arena.alloc(KObject::KTypeValue(KType::Null));
    assert!(t.accepts_part(&ExpressionPart::Future(kt_number)));
    assert!(t.accepts_part(&ExpressionPart::Future(kt_str)));
    assert!(t.accepts_part(&ExpressionPart::Future(kt_bool)));
    assert!(t.accepts_part(&ExpressionPart::Future(kt_null)));
    // Struct / union type tokens flow as `KTypeValue(UserType { .. })` now — a `:Type`
    // slot admits them via the generic `Future(KTypeValue(_))` arm.
    let tagged_token: &KObject<'_> = arena.alloc(KObject::KTypeValue(KType::UserType {
        kind: UserTypeKind::Tagged {
            schema: Rc::new(HashMap::new()),
        },
        name: "Maybe".into(),
        scope_id: ScopeId::SENTINEL,
    }));
    let struct_token: &KObject<'_> = arena.alloc(KObject::KTypeValue(KType::UserType {
        kind: UserTypeKind::struct_sentinel(),
        name: "Point".into(),
        scope_id: ScopeId::SENTINEL,
    }));
    assert!(t.accepts_part(&ExpressionPart::Future(tagged_token)));
    assert!(t.accepts_part(&ExpressionPart::Future(struct_token)));
    let child = arena.alloc_scope(crate::machine::Scope::child_under_module(
        scope,
        "IntMod".into(),
    ));
    let module = arena.alloc_module(Module::new("IntMod".into(), child));
    let kt_module: &KObject<'_> = arena.alloc(KObject::KTypeValue(KType::Module {
        module,
        frame: None,
    }));
    assert!(!t.accepts_part(&ExpressionPart::Future(kt_module)));
    let sig = arena.alloc_signature(Signature::new("OrderedSig".into(), scope));
    let kt_sig: &KObject<'_> = arena.alloc(KObject::KTypeValue(KType::Signature {
        sig,
        pinned_slots: Vec::new(),
    }));
    assert!(!t.accepts_part(&ExpressionPart::Future(kt_sig)));
    let n: &KObject<'_> = arena.alloc(KObject::Number(7.0));
    let s: &KObject<'_> = arena.alloc(KObject::KString("hi".into()));
    assert!(!t.accepts_part(&ExpressionPart::Future(n)));
    assert!(!t.accepts_part(&ExpressionPart::Future(s)));
}

/// A `Wrapped` value with a NEWTYPE identity fills the wildcard
/// `AnyUserType { kind: Newtype { repr: <sentinel> } }` slot — the manual
/// `PartialEq` ignores `repr`.
#[test]
fn any_user_type_newtype_accepts_wrapped_only() {
    use crate::machine::core::RuntimeArena;
    let arena = RuntimeArena::new();
    let t = KType::AnyUserType {
        kind: UserTypeKind::Newtype {
            repr: Box::new(KType::Any),
        },
    };
    let inner: &KObject<'_> = arena.alloc(KObject::Number(3.0));
    let type_id: &KType = arena.alloc(KType::UserType {
        kind: UserTypeKind::Newtype {
            repr: Box::new(KType::Number),
        },
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
        kind: UserTypeKind::Newtype {
            repr: Box::new(KType::Any),
        },
    };
    let any_struct = KType::AnyUserType {
        kind: UserTypeKind::struct_sentinel(),
    };
    let dist = KType::UserType {
        kind: UserTypeKind::Newtype {
            repr: Box::new(KType::Number),
        },
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
    let any_struct = KType::AnyUserType {
        kind: UserTypeKind::struct_sentinel(),
    };
    let any_tagged = KType::AnyUserType {
        kind: UserTypeKind::tagged_sentinel(),
    };
    let point = KType::UserType {
        kind: UserTypeKind::struct_sentinel(),
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

/// `is_type_denoting` admission table: variants whose declared `KType` makes
/// the bound value's nominal identity meaningful at the type level.
#[test]
fn is_type_denoting_table() {
    use crate::builtins::default_scope;
    use crate::machine::core::RuntimeArena;
    use crate::machine::model::values::Signature;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let sig = arena.alloc_signature(Signature::new("OrderedSig".into(), scope));
    let sb = KType::Signature {
        sig,
        pinned_slots: Vec::new(),
    };
    assert!(sb.is_type_denoting());
    let sb_pinned = KType::Signature {
        sig,
        pinned_slots: vec![("Type".into(), KType::Number)],
    };
    assert!(sb_pinned.is_type_denoting());
    assert!(KType::AnySignature.is_type_denoting());
    assert!(KType::Type.is_type_denoting());
    assert!(KType::TypeExprRef.is_type_denoting());
    assert!(KType::AnyModule.is_type_denoting());
    // Wildcard struct/tagged slots don't make their parameter a type binder —
    // the value carries no nominal identity the caller hasn't already named.
    assert!(!KType::AnyUserType {
        kind: UserTypeKind::struct_sentinel()
    }
    .is_type_denoting());
    assert!(!KType::AnyUserType {
        kind: UserTypeKind::tagged_sentinel()
    }
    .is_type_denoting());
    // Per-declaration UserType: nominal identity already lives in the declaring
    // scope's `bindings.types`; rebinding per-call would be a no-op or shadow.
    let ut = KType::UserType {
        kind: UserTypeKind::struct_sentinel(),
        scope_id: ScopeId::from_raw(0, 1),
        name: "Foo".into(),
    };
    assert!(!ut.is_type_denoting());
    assert!(!KType::Number.is_type_denoting());
    assert!(!KType::Str.is_type_denoting());
    assert!(!KType::Bool.is_type_denoting());
    assert!(!KType::Null.is_type_denoting());
    assert!(!KType::Any.is_type_denoting());
    assert!(!KType::Identifier.is_type_denoting());
    assert!(!KType::KExpression.is_type_denoting());
    assert!(!KType::List(Box::new(KType::Number)).is_type_denoting());
    assert!(!KType::Dict(Box::new(KType::Str), Box::new(KType::Number),).is_type_denoting());
    assert!(!KType::KFunction {
        args: vec![KType::Number],
        ret: Box::new(KType::Number),
    }
    .is_type_denoting());
}

/// `KType::Signature { pinned_slots }` specificity rules (constraint role):
/// - A non-empty `pinned_slots` strictly refines an empty same-`sig_id` form when
///   every pin in the empty side appears (with equal `KType`) in the non-empty side.
/// - Different `sig_id`s are incomparable.
/// - Same `sig_id` with disjoint constraint keys is incomparable.
/// - Same-key-different-`KType` is incomparable.
/// - A `Signature` (pinned or not) strictly refines `AnyModule`.
#[test]
fn is_more_specific_for_pinned_signature_bound() {
    use crate::builtins::default_scope;
    use crate::machine::core::RuntimeArena;
    use crate::machine::model::values::Signature;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    // Two distinct decl_scopes → two distinct `sig_id`s.
    let ordered_scope = arena.alloc_scope(crate::machine::Scope::child_under_sig(
        scope,
        "OrderedSig".into(),
    ));
    let hashed_scope = arena.alloc_scope(crate::machine::Scope::child_under_sig(
        scope,
        "HashedSig".into(),
    ));
    let ordered = arena.alloc_signature(Signature::new("OrderedSig".into(), ordered_scope));
    let hashed = arena.alloc_signature(Signature::new("HashedSig".into(), hashed_scope));

    let bare = KType::Signature {
        sig: ordered,
        pinned_slots: Vec::new(),
    };
    let pinned_number = KType::Signature {
        sig: ordered,
        pinned_slots: vec![("Type".into(), KType::Number)],
    };
    let pinned_str = KType::Signature {
        sig: ordered,
        pinned_slots: vec![("Type".into(), KType::Str)],
    };
    let pinned_two = KType::Signature {
        sig: ordered,
        pinned_slots: vec![("Type".into(), KType::Number), ("Elt".into(), KType::Str)],
    };
    let other_sig = KType::Signature {
        sig: hashed,
        pinned_slots: vec![("Type".into(), KType::Number)],
    };
    let pinned_elt = KType::Signature {
        sig: ordered,
        pinned_slots: vec![("Elt".into(), KType::Number)],
    };
    let any_module = KType::AnyModule;

    assert!(pinned_number.is_more_specific_than(&bare));
    assert!(!bare.is_more_specific_than(&pinned_number));
    assert!(pinned_two.is_more_specific_than(&pinned_number));
    assert!(!pinned_number.is_more_specific_than(&pinned_two));
    assert!(!pinned_number.is_more_specific_than(&pinned_str));
    assert!(!pinned_str.is_more_specific_than(&pinned_number));
    assert!(!pinned_number.is_more_specific_than(&pinned_elt));
    assert!(!pinned_elt.is_more_specific_than(&pinned_number));
    assert!(!pinned_number.is_more_specific_than(&other_sig));
    assert!(!other_sig.is_more_specific_than(&pinned_number));
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
        kind: UserTypeKind::TypeConstructor {
            schema: std::rc::Rc::new(std::collections::HashMap::new()),
            param_names: vec!["T".into(), "E".into()],
        },
        scope_id: result_sid,
        name: "Result".into(),
    });
    let myerr_ty = KType::UserType {
        kind: UserTypeKind::tagged_sentinel(),
        scope_id: myerr_sid,
        name: "MyErr".into(),
    };
    let kerror_ty = KType::UserType {
        kind: UserTypeKind::tagged_sentinel(),
        scope_id: kerror_sid,
        name: "KError".into(),
    };

    let slot_myerr = KType::ConstructorApply {
        ctor: ctor.clone(),
        args: vec![KType::Any, myerr_ty.clone()],
    };
    let caught = result_value(result_sid, "error", error_carrier(kerror_sid, "KError"));
    assert!(!slot_myerr.matches_value(&caught));

    let slot_kerror = KType::ConstructorApply {
        ctor: ctor.clone(),
        args: vec![KType::Any, kerror_ty.clone()],
    };
    assert!(slot_kerror.matches_value(&caught));

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
        kind: UserTypeKind::TypeConstructor {
            schema: std::rc::Rc::new(std::collections::HashMap::new()),
            param_names: vec!["T".into(), "E".into()],
        },
        scope_id: result_sid,
        name: "Result".into(),
    });
    let myerr_ty = KType::UserType {
        kind: UserTypeKind::tagged_sentinel(),
        scope_id: myerr_sid,
        name: "MyErr".into(),
    };
    let ok_value = result_value(result_sid, "ok", KObject::Number(42.0));
    let slot = KType::ConstructorApply {
        ctor: ctor.clone(),
        args: vec![KType::Number, myerr_ty],
    };
    assert!(slot.matches_value(&ok_value));
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

/// Covariance for `ConstructorApply` carriers: a `Result<Number, MyErr>` value is
/// admitted by the coarser `:(Result Any Any)` slot, and the refined
/// `:(Result Number MyErr)` slot is strictly more specific, so dispatch tie-breaks
/// toward the refined overload.
#[test]
fn constructor_apply_covariant_admission_and_specificity() {
    let result_sid = ScopeId::from_raw(0, 0x9001);
    let myerr_sid = ScopeId::from_raw(0, 0x9003);
    let ctor = Box::new(KType::UserType {
        kind: UserTypeKind::TypeConstructor {
            schema: std::rc::Rc::new(std::collections::HashMap::new()),
            param_names: vec!["T".into(), "E".into()],
        },
        scope_id: result_sid,
        name: "Result".into(),
    });
    let myerr = KType::UserType {
        kind: UserTypeKind::tagged_sentinel(),
        scope_id: myerr_sid,
        name: "MyErr".into(),
    };
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
    assert!(coarse.matches_value(&stamped));
    assert!(refined.matches_value(&stamped));
    assert!(refined.is_more_specific_than(&coarse));
    assert!(!coarse.is_more_specific_than(&refined));
}

/// A populated `type_args` carrier (stamped by ascription) is checked structurally against
/// the slot args, taking precedence over the inhabited-tag path.
#[test]
fn constructor_apply_stamped_type_args_checked_structurally() {
    let result_sid = ScopeId::from_raw(0, 0x9001);
    let ctor = Box::new(KType::UserType {
        kind: UserTypeKind::TypeConstructor {
            schema: std::rc::Rc::new(std::collections::HashMap::new()),
            param_names: vec!["T".into(), "E".into()],
        },
        scope_id: result_sid,
        name: "Result".into(),
    });
    let stamped = KObject::Tagged {
        tag: "ok".into(),
        value: std::rc::Rc::new(KObject::Number(1.0)),
        scope_id: result_sid,
        name: "Result".into(),
        type_args: std::rc::Rc::new(vec![KType::Number, KType::Str]),
    };
    let slot_ok = KType::ConstructorApply {
        ctor: ctor.clone(),
        args: vec![KType::Number, KType::Str],
    };
    assert!(slot_ok.matches_value(&stamped));
    let slot_any = KType::ConstructorApply {
        ctor: ctor.clone(),
        args: vec![KType::Any, KType::Any],
    };
    assert!(slot_any.matches_value(&stamped));
    let slot_bad = KType::ConstructorApply {
        ctor,
        args: vec![KType::Bool, KType::Str],
    };
    assert!(!slot_bad.matches_value(&stamped));
}
