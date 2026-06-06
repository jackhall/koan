use super::*;
use crate::machine::core::ScopeId;
use crate::machine::model::types::{NominalSchema, RecursiveSet};
use crate::machine::model::Record;
use std::rc::Rc;

/// A singleton-set `KType::SetRef` for a struct named `name` (empty schema is fine — the
/// predicates key on `(set ptr, index)` + `kind`, never the schema).
fn struct_setref<'a>(name: &str, scope_id: ScopeId) -> KType<'a> {
    let set = RecursiveSet::singleton(name.into(), scope_id, NominalSchema::Struct(Record::new()));
    KType::SetRef { set, index: 0 }
}

/// A singleton-set `KType::SetRef` for a newtype named `name` over `repr`.
fn newtype_setref<'a>(name: &str, scope_id: ScopeId, repr: KType<'a>) -> KType<'a> {
    let set = RecursiveSet::singleton(
        name.into(),
        scope_id,
        NominalSchema::Newtype(Box::new(repr)),
    );
    KType::SetRef { set, index: 0 }
}

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

/// Width-subset specificity: a nullary function `{}` is strictly more specific than a
/// unary `{x}` (its param key set is a subset, so it fills the wider slot under
/// call-by-name width drop), and the unary is not more specific than the nullary
/// (the unary declares a param the nullary lacks → contravariant width violation).
#[test]
fn is_more_specific_function_width_subset() {
    let unary = KType::KFunction {
        params: Record::from_pairs(vec![("x".into(), KType::Number)]),
        ret: Box::new(KType::Number),
    };
    let nullary = KType::KFunction {
        params: Record::new(),
        ret: Box::new(KType::Number),
    };
    assert!(nullary.is_more_specific_than(&unary));
    assert!(!unary.is_more_specific_than(&nullary));
}

/// Depth-contravariant function specificity: `(x :Any) -> R ≺ (x :Number) -> R`. The
/// more-general param (`Any` ⊐ `Number`) makes the function more specific, because a
/// value accepting `Any` fills a slot that promised only `Number`.
#[test]
fn is_more_specific_function_param_contravariant() {
    let any_param = KType::KFunction {
        params: Record::from_pairs(vec![("x".into(), KType::Any)]),
        ret: Box::new(KType::Str),
    };
    let number_param = KType::KFunction {
        params: Record::from_pairs(vec![("x".into(), KType::Number)]),
        ret: Box::new(KType::Str),
    };
    assert!(any_param.is_more_specific_than(&number_param));
    assert!(!number_param.is_more_specific_than(&any_param));
}

/// Return-covariant function specificity: `(x) -> Number ≺ (x) -> Any`. The narrower
/// return makes the function more specific.
#[test]
fn is_more_specific_function_return_covariant() {
    let number_ret = KType::KFunction {
        params: Record::from_pairs(vec![("x".into(), KType::Number)]),
        ret: Box::new(KType::Number),
    };
    let any_ret = KType::KFunction {
        params: Record::from_pairs(vec![("x".into(), KType::Number)]),
        ret: Box::new(KType::Any),
    };
    assert!(number_ret.is_more_specific_than(&any_ret));
    assert!(!any_ret.is_more_specific_than(&number_ret));
}

fn record_ty<'a>(fields: Vec<(&str, KType<'a>)>) -> KType<'a> {
    KType::Record(Box::new(Record::from_pairs(
        fields.into_iter().map(|(n, t)| (n.to_string(), t)),
    )))
}

/// Record-value subtyping is the *dual* of function-param subtyping: a *wider* record is
/// strictly more specific (a `{x, y}` value fills an `{x}` slot, dropping `y`).
#[test]
fn record_width_superset_more_specific() {
    let wide = record_ty(vec![("x", KType::Number), ("y", KType::Str)]);
    let narrow = record_ty(vec![("x", KType::Number)]);
    assert!(wide.is_more_specific_than(&narrow));
    assert!(!narrow.is_more_specific_than(&wide));
}

/// Covariant depth: `:{x :Number} ≺ :{x :Any}`.
#[test]
fn record_depth_covariant() {
    let number = record_ty(vec![("x", KType::Number)]);
    let any = record_ty(vec![("x", KType::Any)]);
    assert!(number.is_more_specific_than(&any));
    assert!(!any.is_more_specific_than(&number));
}

/// Disjoint field sets are incomparable (`{x, y}` vs `{x, z}`) — dispatch ambiguity, not
/// an ordering.
#[test]
fn record_disjoint_fields_incomparable() {
    let xy = record_ty(vec![("x", KType::Number), ("y", KType::Str)]);
    let xz = record_ty(vec![("x", KType::Number), ("z", KType::Str)]);
    assert!(!xy.is_more_specific_than(&xz));
    assert!(!xz.is_more_specific_than(&xy));
}

/// A `{x = 1, y = "a"}` value (carried type `:{x :Number, y :Str}`) admits and matches a
/// narrower `:{x :Number}` slot (width drop); rejects a field-type mismatch (`:{x :Str}`)
/// and a slot demanding a field the value lacks (`:{x :Number, q :Bool}`). A bare record
/// literal admits any record slot shape-only.
#[test]
fn record_value_admission_and_matches() {
    use crate::machine::core::RuntimeArena;
    let arena = RuntimeArena::new();
    let value: &KObject<'_> = arena.alloc_object(KObject::record(Record::from_pairs(vec![
        ("x".to_string(), KObject::Number(1.0)),
        ("y".to_string(), KObject::KString("a".into())),
    ])));

    let narrow = record_ty(vec![("x", KType::Number)]);
    assert!(narrow.accepts_part(&ExpressionPart::Future(value)));
    assert!(narrow.matches_value(value));

    let mismatch = record_ty(vec![("x", KType::Str)]);
    assert!(!mismatch.accepts_part(&ExpressionPart::Future(value)));
    assert!(!mismatch.matches_value(value));

    let extra = record_ty(vec![("x", KType::Number), ("q", KType::Bool)]);
    assert!(!extra.accepts_part(&ExpressionPart::Future(value)));
    assert!(!extra.matches_value(value));

    // Unevaluated literal admits shape-only (defer-then-reevaluate on the typed value).
    assert!(mismatch.accepts_part(&ExpressionPart::RecordLiteral(vec![])));
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
        kind: NominalKind::Struct,
    };
    let struct_set = RecursiveSet::singleton(
        "Point".into(),
        ScopeId::SENTINEL,
        NominalSchema::Struct(Record::new()),
    );
    let s: &KObject<'_> = arena.alloc_object(KObject::Struct {
        set: struct_set,
        index: 0,
        fields: Rc::new(IndexMap::new()),
    });
    let tagged_set = RecursiveSet::singleton(
        "Maybe".into(),
        ScopeId::SENTINEL,
        NominalSchema::Tagged(std::collections::HashMap::new()),
    );
    let tagged: &KObject<'_> = arena.alloc_object(KObject::Tagged {
        tag: "some".into(),
        value: Rc::new(KObject::Number(1.0)),
        set: tagged_set,
        index: 0,
        type_args: Rc::new(vec![]),
    });
    let n: &KObject<'_> = arena.alloc_object(KObject::Number(1.0));
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
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let t = KType::Type;
    let kt_number: &KObject<'_> = arena.alloc_object(KObject::KTypeValue(KType::Number));
    let kt_str: &KObject<'_> = arena.alloc_object(KObject::KTypeValue(KType::Str));
    let kt_bool: &KObject<'_> = arena.alloc_object(KObject::KTypeValue(KType::Bool));
    let kt_null: &KObject<'_> = arena.alloc_object(KObject::KTypeValue(KType::Null));
    assert!(t.accepts_part(&ExpressionPart::Future(kt_number)));
    assert!(t.accepts_part(&ExpressionPart::Future(kt_str)));
    assert!(t.accepts_part(&ExpressionPart::Future(kt_bool)));
    assert!(t.accepts_part(&ExpressionPart::Future(kt_null)));
    // Struct / union type tokens flow as `KTypeValue(SetRef { .. })` now — a `:Type`
    // slot admits them via the generic `Future(KTypeValue(_))` arm.
    let tagged_set = RecursiveSet::singleton(
        "Maybe".into(),
        ScopeId::SENTINEL,
        NominalSchema::Tagged(HashMap::new()),
    );
    let tagged_token: &KObject<'_> = arena.alloc_object(KObject::KTypeValue(KType::SetRef {
        set: tagged_set,
        index: 0,
    }));
    let struct_token: &KObject<'_> = arena.alloc_object(KObject::KTypeValue(struct_setref(
        "Point",
        ScopeId::SENTINEL,
    )));
    assert!(t.accepts_part(&ExpressionPart::Future(tagged_token)));
    assert!(t.accepts_part(&ExpressionPart::Future(struct_token)));
    let child = arena.alloc_scope(crate::machine::Scope::child_under_module(
        scope,
        "IntMod".into(),
    ));
    let module = arena.alloc_module(Module::new("IntMod".into(), child));
    let kt_module: &KObject<'_> = arena.alloc_object(KObject::KTypeValue(KType::Module {
        module,
        frame: None,
    }));
    assert!(!t.accepts_part(&ExpressionPart::Future(kt_module)));
    let sig = arena.alloc_signature(Signature::new("OrderedSig".into(), scope));
    let kt_sig: &KObject<'_> = arena.alloc_object(KObject::KTypeValue(KType::Signature {
        sig,
        pinned_slots: Vec::new(),
    }));
    assert!(!t.accepts_part(&ExpressionPart::Future(kt_sig)));
    let n: &KObject<'_> = arena.alloc_object(KObject::Number(7.0));
    let s: &KObject<'_> = arena.alloc_object(KObject::KString("hi".into()));
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
        kind: NominalKind::Newtype,
    };
    let inner: &KObject<'_> = arena.alloc_object(KObject::Number(3.0));
    let type_id: &KType = arena.alloc_ktype(newtype_setref(
        "Distance",
        ScopeId::from_raw(0, 0xAA),
        KType::Number,
    ));
    let w: &KObject<'_> = arena.alloc_object(KObject::Wrapped {
        inner: crate::machine::model::values::NonWrappedRef::peel(inner),
        type_id,
    });
    let struct_set = RecursiveSet::singleton(
        "Point".into(),
        ScopeId::SENTINEL,
        NominalSchema::Struct(Record::new()),
    );
    let s: &KObject<'_> = arena.alloc_object(KObject::Struct {
        set: struct_set,
        index: 0,
        fields: std::rc::Rc::new(indexmap::IndexMap::new()),
    });
    assert!(t.accepts_part(&ExpressionPart::Future(w)));
    assert!(!t.accepts_part(&ExpressionPart::Future(s)));
    assert!(t.matches_value(w));
    assert!(!t.matches_value(s));
}

/// Pins the wildcard refinement: a `Newtype`-kind `SetRef` is strictly more specific than
/// `AnyUserType { kind: Newtype }`, and incomparable with `AnyUserType { kind: Struct }`.
#[test]
fn user_type_newtype_specificity_lattice() {
    let any_newtype = KType::AnyUserType {
        kind: NominalKind::Newtype,
    };
    let any_struct = KType::AnyUserType {
        kind: NominalKind::Struct,
    };
    let dist = newtype_setref("Distance", ScopeId::from_raw(0, 0xAA), KType::Number);
    assert!(dist.is_more_specific_than(&any_newtype));
    assert!(!any_newtype.is_more_specific_than(&dist));
    assert!(!dist.is_more_specific_than(&any_struct));
    assert!(!any_struct.is_more_specific_than(&dist));
}

/// Specificity ordering for the `SetRef` / `AnyUserType` variants:
/// - `AnyUserType` is strictly under `Any` (handled by the top-level `Any` short-circuit).
/// - A `SetRef` member of kind `K` is strictly under `AnyUserType { kind: K }`.
/// - A `SetRef` of one kind and `AnyUserType` of a different kind are incomparable
///   (sibling families).
#[test]
fn user_type_specificity_lattice() {
    let any_struct = KType::AnyUserType {
        kind: NominalKind::Struct,
    };
    let any_tagged = KType::AnyUserType {
        kind: NominalKind::Tagged,
    };
    let point = struct_setref("Point", ScopeId::from_raw(0, 0xAA));
    // `AnyUserType` strictly under `Any`.
    assert!(any_struct.is_more_specific_than(&KType::Any));
    assert!(!KType::Any.is_more_specific_than(&any_struct));
    // A `Struct`-kind `SetRef` strictly under `AnyUserType { kind: Struct }`.
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
        kind: NominalKind::Struct
    }
    .is_type_denoting());
    assert!(!KType::AnyUserType {
        kind: NominalKind::Tagged
    }
    .is_type_denoting());
    // Per-declaration `SetRef`: nominal identity already lives in the declaring
    // scope's `bindings.types`; rebinding per-call would be a no-op or shadow.
    let ut = struct_setref("Foo", ScopeId::from_raw(0, 1));
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
        params: Record::from_pairs(vec![("x".into(), KType::Number)]),
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

/// A shared `Result` `TypeConstructor` set. Identity is now `(set ptr, index)`, so a
/// `ConstructorApply` ctor and a `Tagged` carrier only match when they reference the *same*
/// `Rc` — every test below threads this one set through both the slot ctor and the value.
fn result_set<'a>(result_sid: ScopeId) -> Rc<RecursiveSet<'a>> {
    RecursiveSet::singleton(
        "Result".into(),
        result_sid,
        NominalSchema::TypeConstructor {
            schema: std::collections::HashMap::new(),
            param_names: vec!["T".into(), "E".into()],
        },
    )
}

/// Build a `Result`-carrier `Tagged` value occupying `tag` with `payload`, referencing
/// `set` (the shared `Result` allocation). The inner `payload` is itself a `Tagged` carrier
/// whose set is the error type's nominal identity.
fn result_value<'a>(set: &Rc<RecursiveSet<'a>>, tag: &str, payload: KObject<'a>) -> KObject<'a> {
    KObject::Tagged {
        tag: tag.into(),
        value: std::rc::Rc::new(payload),
        set: Rc::clone(set),
        index: 0,
        type_args: std::rc::Rc::new(vec![]),
    }
}

/// A bare error carrier (`Tagged` over `set`) standing in for a caught error value.
fn error_carrier<'a>(set: &Rc<RecursiveSet<'a>>) -> KObject<'a> {
    KObject::Tagged {
        tag: "_".into(),
        value: std::rc::Rc::new(KObject::Number(0.0)),
        set: Rc::clone(set),
        index: 0,
        type_args: std::rc::Rc::new(vec![]),
    }
}

/// A singleton tagged set named `name`, for an error-type identity.
fn tagged_set<'a>(name: &str, scope_id: ScopeId) -> Rc<RecursiveSet<'a>> {
    RecursiveSet::singleton(
        name.into(),
        scope_id,
        NominalSchema::Tagged(std::collections::HashMap::new()),
    )
}

/// `:(Result T E)` slot admission: a `ConstructorApply` slot whose ctor identity matches
/// the `Result` carrier admits an `error(...)` value iff the inhabited `error` payload
/// (param index 1) satisfies the slot's `E`. A caught `error(KError)` is rejected where
/// `E = MyErr` and accepted where `E = KError` / `Any`. Identity is now allocation-based, so
/// the slot's `E` and the value's payload carrier share one set per error type.
#[test]
fn constructor_apply_result_checks_inhabited_error_param() {
    let result_sid = ScopeId::from_raw(0, 0x9001);
    let kerror_sid = ScopeId::from_raw(0, 0x9002);
    let myerr_sid = ScopeId::from_raw(0, 0x9003);

    let r_set = result_set(result_sid);
    let ctor = Box::new(KType::SetRef {
        set: Rc::clone(&r_set),
        index: 0,
    });
    let kerror_set = tagged_set("KError", kerror_sid);
    let myerr_set = tagged_set("MyErr", myerr_sid);
    let myerr_ty = KType::SetRef {
        set: Rc::clone(&myerr_set),
        index: 0,
    };
    let kerror_ty = KType::SetRef {
        set: Rc::clone(&kerror_set),
        index: 0,
    };

    let slot_myerr = KType::ConstructorApply {
        ctor: ctor.clone(),
        args: vec![KType::Any, myerr_ty.clone()],
    };
    let caught = result_value(&r_set, "error", error_carrier(&kerror_set));
    assert!(!slot_myerr.matches_value(&caught));

    let slot_kerror = KType::ConstructorApply {
        ctor: ctor.clone(),
        args: vec![KType::Any, kerror_ty.clone()],
    };
    assert!(slot_kerror.matches_value(&caught));

    let my_error = result_value(&r_set, "error", error_carrier(&myerr_set));
    assert!(slot_myerr.matches_value(&my_error));
}

/// The `ok` field maps to param 0, so `:(Result Number E)` checks the `ok` payload
/// against `Number` regardless of `E`: an `ok(42)` value admits any `E` (the absent
/// `error` parameter is unconstrained at the value).
#[test]
fn constructor_apply_result_ok_admits_any_error_param() {
    let result_sid = ScopeId::from_raw(0, 0x9001);
    let myerr_sid = ScopeId::from_raw(0, 0x9003);
    let r_set = result_set(result_sid);
    let ctor = Box::new(KType::SetRef {
        set: Rc::clone(&r_set),
        index: 0,
    });
    let myerr_ty = KType::SetRef {
        set: tagged_set("MyErr", myerr_sid),
        index: 0,
    };
    let ok_value = result_value(&r_set, "ok", KObject::Number(42.0));
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
    let r_set = result_set(result_sid);
    let ctor = Box::new(KType::SetRef {
        set: Rc::clone(&r_set),
        index: 0,
    });
    let myerr = KType::SetRef {
        set: tagged_set("MyErr", myerr_sid),
        index: 0,
    };
    let stamped = KObject::Tagged {
        tag: "ok".into(),
        value: std::rc::Rc::new(KObject::Number(1.0)),
        set: Rc::clone(&r_set),
        index: 0,
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
    let r_set = result_set(result_sid);
    let ctor = Box::new(KType::SetRef {
        set: Rc::clone(&r_set),
        index: 0,
    });
    let stamped = KObject::Tagged {
        tag: "ok".into(),
        value: std::rc::Rc::new(KObject::Number(1.0)),
        set: Rc::clone(&r_set),
        index: 0,
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

use crate::machine::model::ast::TypeName;
use crate::machine::model::types::{DeferredReturn, DeferredReturnSurface, ReturnType};

/// A function whose `ret` slot is a `DeferredReturn` carrier is strictly more specific
/// than the same shape with an `Any` return (covariant short-circuit), and the reverse
/// does not hold — `Any` never refines a precise placeholder.
#[test]
fn deferred_return_more_specific_than_any() {
    let deferred = KType::KFunction {
        params: Record::new(),
        ret: Box::new(KType::DeferredReturn(DeferredReturnSurface::TypeExpr(
            TypeName::leaf("Er".into()),
        ))),
    };
    let any = KType::KFunction {
        params: Record::new(),
        ret: Box::new(KType::Any),
    };
    assert!(deferred.is_more_specific_than(&any));
    assert!(!any.is_more_specific_than(&deferred));
}

/// Two functors differing only in their deferred-return shadow are distinct: not equal,
/// neither more specific than the other, and they hash apart.
#[test]
fn two_functors_differ_only_in_deferred_return_are_distinct() {
    use std::hash::{Hash, Hasher};
    let er = KType::KFunctor {
        params: Record::new(),
        ret: Box::new(KType::DeferredReturn(DeferredReturnSurface::TypeExpr(
            TypeName::leaf("Er".into()),
        ))),
        body: None,
    };
    let ar = KType::KFunctor {
        params: Record::new(),
        ret: Box::new(KType::DeferredReturn(DeferredReturnSurface::TypeExpr(
            TypeName::leaf("Ar".into()),
        ))),
        body: None,
    };
    assert_ne!(er, ar);
    assert!(!er.is_more_specific_than(&ar));
    assert!(!ar.is_more_specific_than(&er));
    // `KType` carries interior mutability, so it can't key a `HashSet` (clippy
    // `mutable_key_type`). Hash each directly: the deferred-return shadow participates
    // in `KType`'s hash, so the two functors hash apart.
    let hash = |k: &KType<'_>| {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        k.hash(&mut h);
        h.finish()
    };
    assert_ne!(hash(&er), hash(&ar));
}

/// `function_compat` admits a deferred-return candidate against a `DeferredReturn` slot
/// iff the surface shadows match, admits any deferred return against an `Any` slot, and
/// rejects against a resolved (`Number`) slot — a deferred return refines nothing more
/// precise than its own shadow.
#[test]
fn deferred_return_admission_via_function_compat() {
    let candidate = ExpressionSignature {
        return_type: ReturnType::Deferred(DeferredReturn::TypeExpr(TypeName::leaf("Er".into()))),
        elements: vec![],
    };
    let no_params = Record::new();

    // Matching shadow → admit.
    let slot_er =
        KType::DeferredReturn(DeferredReturnSurface::TypeExpr(TypeName::leaf("Er".into())));
    assert!(function_compat(&candidate, &no_params, &slot_er, false));

    // Differing shadow → reject.
    let slot_ar =
        KType::DeferredReturn(DeferredReturnSurface::TypeExpr(TypeName::leaf("Ar".into())));
    assert!(!function_compat(&candidate, &no_params, &slot_ar, false));

    // Resolved slot → reject (opaque until elaboration).
    assert!(!function_compat(
        &candidate,
        &no_params,
        &KType::Number,
        false
    ));

    // `Any` slot → admit.
    assert!(function_compat(&candidate, &no_params, &KType::Any, false));
}

/// `DeferredReturnSurface` identity is syntactic: two `Expression` shadows built from the
/// same render are equal and hash-equal; a differing render is unequal.
#[test]
fn deferred_return_surface_eq_and_hash() {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    fn h(s: &DeferredReturnSurface) -> u64 {
        let mut hasher = DefaultHasher::new();
        s.hash(&mut hasher);
        hasher.finish()
    }
    let a = DeferredReturnSurface::Expression("ATTR Er Type".into());
    let b = DeferredReturnSurface::Expression("ATTR Er Type".into());
    let c = DeferredReturnSurface::Expression("ATTR Ar Type".into());
    assert_eq!(a, b);
    assert_eq!(h(&a), h(&b));
    assert_ne!(a, c);
}
