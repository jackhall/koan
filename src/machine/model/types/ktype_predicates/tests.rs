use super::*;
use crate::builtins::test_support::spliced_part;
use crate::machine::core::ScopeId;
use crate::machine::model::ast::ExpressionPart;
use crate::machine::model::types::{NominalSchema, RecursiveSet};
use crate::machine::model::Carried;
use crate::machine::model::Record;
use std::rc::Rc;

/// A singleton-set `KType::SetRef` for a record-repr newtype (an ex-struct) named `name`
/// (empty record repr is fine — the predicates key on `(set ptr, index)` + `kind`, never the
/// schema).
fn record_newtype_setref<'a>(name: &str, scope_id: ScopeId) -> KType<'a> {
    let set = RecursiveSet::singleton(
        name.into(),
        scope_id,
        NominalSchema::NewType(Box::new(KType::Record(Box::new(Record::new())))),
    );
    KType::SetRef { set, index: 0 }
}

/// A singleton-set `KType::SetRef` for a newtype named `name` over `repr`.
fn newtype_setref<'a>(name: &str, scope_id: ScopeId, repr: KType<'a>) -> KType<'a> {
    let set = RecursiveSet::singleton(
        name.into(),
        scope_id,
        NominalSchema::NewType(Box::new(repr)),
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

/// `accepts_carried` is the same-lifetime core `accepts_part`'s `Spliced` arm delegates to: a
/// resolved value classifies identically whether reached as a spliced part or opened directly. Also
/// pins the value-shaped arms (object type-tag, type-channel `OfKind`) the delegation now owns.
#[test]
fn accepts_carried_matches_spliced_delegation() {
    use crate::machine::core::{run_root_storage, FrameStorageExt};
    let storage = run_root_storage();
    let region = storage.brand();
    let n: &KObject<'_> = region.alloc_object(KObject::Number(7.0));
    let s: &KObject<'_> = region.alloc_object(KObject::KString("hi".into()));

    for (ty, carried) in [
        (KType::Number, Carried::Object(n)),
        (KType::Str, Carried::Object(s)),
        (KType::Any, Carried::Object(n)),
    ] {
        // The delegation equivalence: classifying the spliced cell and opening the value directly agree.
        assert_eq!(
            ty.accepts_carried(carried),
            ty.accepts_part(&spliced_part(carried))
        );
    }
    // A numeric value is admitted by `:Number` / `:Any`, refused by `:Str`.
    assert!(KType::Number.accepts_carried(Carried::Object(n)));
    assert!(KType::Any.accepts_carried(Carried::Object(n)));
    assert!(!KType::Str.accepts_carried(Carried::Object(n)));
    // A type-channel value reaches the `OfKind` arm; a proper-type slot admits it.
    let kt_number = KType::Number;
    assert!(KType::OfKind(KKind::ProperType).accepts_carried(Carried::Type(&kt_number)));
    // An object value reports a non-type `kind_of` and is refused by a type-channel slot.
    assert!(!KType::OfKind(KKind::ProperType).accepts_carried(Carried::Object(n)));
}

/// A spliced **cell** classifies through `accepts_part` by opening at its own brand and re-anchoring
/// for the same-lifetime predicate: a `7.0` value is admitted by `:Number` / `:Any` and refused by
/// `:Str`, matching a direct `accepts_carried`. Built through the scope's own carrier surface
/// (`resident_value_carrier` + `Sealed::seal`) — the exact construction a real splice rests on the
/// working expression — so the open exercises the confined lifetime cast under Miri. Also pins the
/// cell's `is_splice_free` (a resolved value is not raw AST, so QUOTE's guard rejects it).
#[test]
fn spliced_cell_classifies_by_opening() {
    use crate::builtins::test_support::run_root_bare;
    use crate::machine::core::run_root_storage;
    use crate::machine::model::ast::KExpression;
    use crate::machine::model::values::KObject;
    use crate::witnessed::{Delivered, Sealed};

    let storage = run_root_storage();
    let scope = run_root_bare(&storage);
    let obj: &KObject = scope.brand().alloc_object(KObject::Number(7.0));
    let carrier = scope.resident_value_carrier(obj, None, false);
    let cell_part = ExpressionPart::Spliced {
        cell: Delivered::hosted(Sealed::seal(carrier), None),
    };

    for (ty, admits) in [
        (KType::Number, true),
        (KType::Any, true),
        (KType::Str, false),
    ] {
        assert_eq!(
            ty.accepts_part(&cell_part),
            admits,
            "cell classification for {ty:?}",
        );
        // Agrees with opening the value directly.
        assert_eq!(
            ty.accepts_part(&cell_part),
            ty.accepts_carried(Carried::Object(obj))
        );
    }

    // A cell is a resolved value, not raw AST — QUOTE's splice-free guard rejects it.
    let expr = KExpression::new(vec![crate::source::Spanned::bare(cell_part)]);
    assert!(!expr.is_splice_free());
}

/// A `{x = 1, y = "a"}` value (carried type `:{x :Number, y :Str}`) admits and matches a
/// narrower `:{x :Number}` slot (width drop); rejects a field-type mismatch (`:{x :Str}`)
/// and a slot demanding a field the value lacks (`:{x :Number, q :Bool}`). A bare record
/// literal admits any record slot shape-only.
#[test]
fn record_value_admission_and_matches() {
    use crate::machine::core::{run_root_storage, FrameStorageExt};
    let storage = run_root_storage();
    let region = storage.brand();
    let value: &KObject<'_> = region.alloc_object(KObject::record(Record::from_pairs(vec![
        ("x".to_string(), KObject::Number(1.0)),
        ("y".to_string(), KObject::KString("a".into())),
    ])));

    let narrow = record_ty(vec![("x", KType::Number)]);
    assert!(narrow.accepts_part(&spliced_part(Carried::Object(value))));
    assert!(narrow.matches_value(value));

    let mismatch = record_ty(vec![("x", KType::Str)]);
    assert!(!mismatch.accepts_part(&spliced_part(Carried::Object(value))));
    assert!(!mismatch.matches_value(value));

    let extra = record_ty(vec![("x", KType::Number), ("q", KType::Bool)]);
    assert!(!extra.accepts_part(&spliced_part(Carried::Object(value))));
    assert!(!extra.matches_value(value));

    // Unevaluated literal admits shape-only (defer-then-reevaluate on the typed value).
    assert!(mismatch.accepts_part(&ExpressionPart::RecordLiteral(vec![])));
}

/// Admission table for `KType::accepts_part`: bare builtin type tokens
/// and newtype / union `Carried::Type(SetRef)` identities admit; module and signature
/// carriers reject so the `:Type` vs `:Module` / `:Signature` overload distinction
/// stays intact; non-type-denoting carriers reject.
#[test]
fn type_slot_admits_bare_builtin_tokens_and_user_type_carriers() {
    use crate::builtins::default_scope;
    use crate::machine::core::{run_root_storage, FrameStorageExt};
    use crate::machine::model::values::{Module, ModuleSignature};
    use std::collections::HashMap;
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let t = KType::OfKind(KKind::AnyType);
    let kt_number: &KType<'_> = region.brand().alloc_ktype(KType::Number);
    let kt_str: &KType<'_> = region.brand().alloc_ktype(KType::Str);
    let kt_bool: &KType<'_> = region.brand().alloc_ktype(KType::Bool);
    let kt_null: &KType<'_> = region.brand().alloc_ktype(KType::Null);
    assert!(t.accepts_part(&spliced_part(Carried::Type(kt_number))));
    assert!(t.accepts_part(&spliced_part(Carried::Type(kt_str))));
    assert!(t.accepts_part(&spliced_part(Carried::Type(kt_bool))));
    assert!(t.accepts_part(&spliced_part(Carried::Type(kt_null))));
    // NewType / union type tokens flow as `SetRef { .. }` in the type channel — a `:Type`
    // slot admits them when the spliced cell opens to a `Carried::Type`.
    let tagged_set = RecursiveSet::singleton(
        "Maybe".into(),
        ScopeId::SENTINEL,
        NominalSchema::Tagged(HashMap::new()),
    );
    let tagged_token: &KType<'_> = region.brand().alloc_ktype(KType::SetRef {
        set: tagged_set,
        index: 0,
    });
    let struct_token: &KType<'_> = region
        .brand()
        .alloc_ktype(record_newtype_setref("Point", ScopeId::SENTINEL));
    assert!(t.accepts_part(&spliced_part(Carried::Type(tagged_token))));
    assert!(t.accepts_part(&spliced_part(Carried::Type(struct_token))));
    let child = region
        .brand()
        .alloc_scope(crate::machine::Scope::child_under_module(
            scope,
            "IntMod".into(),
        ));
    let module = region
        .brand()
        .alloc_module(Module::new("IntMod".into(), child));
    let kt_module: &KType<'_> = region.brand().alloc_ktype(KType::Module { module });
    assert!(!t.accepts_part(&spliced_part(Carried::Type(kt_module))));
    let sig = region
        .brand()
        .alloc_signature(ModuleSignature::new("OrderedSig".into(), scope));
    let kt_sig: &KType<'_> = region.brand().alloc_ktype(KType::Signature {
        sig,
        pinned_slots: Vec::new(),
    });
    assert!(!t.accepts_part(&spliced_part(Carried::Type(kt_sig))));
    let n: &KObject<'_> = region.brand().alloc_object(KObject::Number(7.0));
    let s: &KObject<'_> = region.brand().alloc_object(KObject::KString("hi".into()));
    assert!(!t.accepts_part(&spliced_part(Carried::Object(n))));
    assert!(!t.accepts_part(&spliced_part(Carried::Object(s))));
}

/// `OfKind` is type-channel-only: a nominal-kind slot classifies a *type value* by its
/// `kind_of`, and never matches a runtime instance (a value is matched by a type, not a kind).
/// `OfKind(NewType)` admits a NewType *type* value, declines a Tagged type value, and declines
/// the runtime `Wrapped` *instance* entirely; `OfKind(Proper)` subsumes the NewType type.
#[test]
fn of_kind_nominal_is_type_channel_only() {
    use crate::machine::core::{run_root_storage, FrameStorageExt};
    let storage = run_root_storage();
    let region = storage.brand();
    let newtype_ty = KType::OfKind(KKind::NewType);

    // The NewType *type value* — admitted in the type channel.
    let newtype_tv = newtype_setref("Distance", ScopeId::from_raw(0, 0xAA), KType::Number);
    assert!(newtype_ty.accepts_part(&spliced_part(Carried::Type(&newtype_tv))));
    assert!(
        KType::OfKind(KKind::ProperType).accepts_part(&spliced_part(Carried::Type(&newtype_tv)))
    );

    // A Tagged type value is the wrong family — declined.
    let tagged_tv = KType::SetRef {
        set: RecursiveSet::singleton(
            "Maybe".into(),
            ScopeId::SENTINEL,
            NominalSchema::Tagged(std::collections::HashMap::new()),
        ),
        index: 0,
    };
    assert!(!newtype_ty.accepts_part(&spliced_part(Carried::Type(&tagged_tv))));

    // The runtime `Wrapped` *instance* is never matched by a kind slot.
    let inner: &KObject<'_> = region.alloc_object(KObject::Number(3.0));
    let type_id: &KType = region.alloc_ktype(newtype_tv.clone());
    let w: &KObject<'_> = region.alloc_object(KObject::Wrapped {
        inner: crate::machine::model::values::NonWrappedRef::peel(inner),
        type_id,
    });
    assert!(!newtype_ty.accepts_part(&spliced_part(Carried::Object(w))));
    assert!(!newtype_ty.matches_value(w));
}

/// Pins the kind refinement: a `NewType`-kind `SetRef` is strictly more specific than
/// `OfKind(NewType)`, and incomparable with `OfKind(Tagged)` (a sibling family).
#[test]
fn user_type_newtype_specificity_lattice() {
    let newtype_kind = KType::OfKind(KKind::NewType);
    let tagged_kind = KType::OfKind(KKind::Tagged);
    let dist = newtype_setref("Distance", ScopeId::from_raw(0, 0xAA), KType::Number);
    assert!(dist.is_more_specific_than(&newtype_kind));
    assert!(!newtype_kind.is_more_specific_than(&dist));
    assert!(!dist.is_more_specific_than(&tagged_kind));
    assert!(!tagged_kind.is_more_specific_than(&dist));
}

/// Specificity ordering for `SetRef` against the `OfKind` kind lattice:
/// - a nominal kind is strictly under `Any` and strictly under `OfKind(Proper)`;
/// - a `SetRef` member of kind `K` is strictly under `OfKind(K)`;
/// - a `SetRef` of one kind and `OfKind` of a different kind are incomparable.
#[test]
fn user_type_specificity_lattice() {
    let newtype_kind = KType::OfKind(KKind::NewType);
    let tagged_kind = KType::OfKind(KKind::Tagged);
    let point = record_newtype_setref("Point", ScopeId::from_raw(0, 0xAA));
    // A nominal kind strictly under `Any` and under `OfKind(Proper)`.
    assert!(newtype_kind.is_more_specific_than(&KType::Any));
    assert!(!KType::Any.is_more_specific_than(&newtype_kind));
    assert!(newtype_kind.is_more_specific_than(&KType::OfKind(KKind::ProperType)));
    assert!(!KType::OfKind(KKind::ProperType).is_more_specific_than(&newtype_kind));
    // A `NewType`-kind `SetRef` strictly under `OfKind(NewType)`.
    assert!(point.is_more_specific_than(&newtype_kind));
    assert!(!newtype_kind.is_more_specific_than(&point));
    // Different-kind pairs incomparable.
    assert!(!point.is_more_specific_than(&tagged_kind));
    assert!(!tagged_kind.is_more_specific_than(&point));
}

/// `is_type_denoting` admission table: variants whose declared `KType` makes
/// the bound value's nominal identity meaningful at the type level.
#[test]
fn is_type_denoting_table() {
    use crate::builtins::default_scope;
    use crate::machine::core::{run_root_storage, FrameStorageExt};
    use crate::machine::model::values::ModuleSignature;
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let sig = region
        .brand()
        .alloc_signature(ModuleSignature::new("OrderedSig".into(), scope));
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
    assert!(KType::OfKind(KKind::Signature).is_type_denoting());
    assert!(KType::OfKind(KKind::AnyType).is_type_denoting());
    assert!(KType::OfKind(KKind::ProperType).is_type_denoting());
    assert!(KType::OfKind(KKind::Module).is_type_denoting());
    // Nominal-family `OfKind` slots are type-channel-only but never name a type binder —
    // the value carries no nominal identity the caller hasn't already named.
    assert!(!KType::OfKind(KKind::NewType).is_type_denoting());
    assert!(!KType::OfKind(KKind::Tagged).is_type_denoting());
    assert!(!KType::OfKind(KKind::TypeConstructor).is_type_denoting());
    // Per-declaration `SetRef`: nominal identity already lives in the declaring
    // scope's `bindings.types`; rebinding per-call would be a no-op or shadow.
    let ut = record_newtype_setref("Foo", ScopeId::from_raw(0, 1));
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
/// - A `Signature` (pinned or not) strictly refines `OfKind(Module)`.
#[test]
fn is_more_specific_for_pinned_signature_bound() {
    use crate::builtins::default_scope;
    use crate::machine::core::{run_root_storage, FrameStorageExt};
    use crate::machine::model::values::ModuleSignature;
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    // Two distinct decl_scopes → two distinct `sig_id`s.
    let ordered_scope = region
        .brand()
        .alloc_scope(crate::machine::Scope::child_under_sig(
            scope,
            "OrderedSig".into(),
        ));
    let hashed_scope = region
        .brand()
        .alloc_scope(crate::machine::Scope::child_under_sig(
            scope,
            "HashedSig".into(),
        ));
    let ordered = region
        .brand()
        .alloc_signature(ModuleSignature::new("OrderedSig".into(), ordered_scope));
    let hashed = region
        .brand()
        .alloc_signature(ModuleSignature::new("HashedSig".into(), hashed_scope));

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
    let any_module = KType::OfKind(KKind::Module);

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
    let caught = result_value(&r_set, "Error", error_carrier(&kerror_set));
    assert!(!slot_myerr.matches_value(&caught));

    let slot_kerror = KType::ConstructorApply {
        ctor: ctor.clone(),
        args: vec![KType::Any, kerror_ty.clone()],
    };
    assert!(slot_kerror.matches_value(&caught));

    let my_error = result_value(&r_set, "Error", error_carrier(&myerr_set));
    assert!(slot_myerr.matches_value(&my_error));
}

/// The `Ok` field maps to param 0, so `:(Result Number E)` checks the `Ok` payload
/// against `Number` regardless of `E`: an `Ok(42)` value admits any `E` (the absent
/// `Error` parameter is unconstrained at the value).
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
    let ok_value = result_value(&r_set, "Ok", KObject::Number(42.0));
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

/// `result_field_param_index` is the field→param linkage source of truth: `Ok`→0,
/// `Error`→1, `None` for any other carrier or tag.
#[test]
fn result_field_param_index_table() {
    assert_eq!(super::result_field_param_index("Result", "Ok"), Some(0));
    assert_eq!(super::result_field_param_index("Result", "Error"), Some(1));
    assert_eq!(super::result_field_param_index("Result", "Other"), None);
    assert_eq!(super::result_field_param_index("Maybe", "Ok"), None);
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
        tag: "Ok".into(),
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
        tag: "Ok".into(),
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

use crate::machine::model::ast::TypeIdentifier;
use crate::machine::model::types::{DeferredReturn, DeferredReturnSurface, ReturnType};

/// A function whose `ret` slot is a `DeferredReturn` carrier is strictly more specific
/// than the same shape with an `Any` return (covariant short-circuit), and the reverse
/// does not hold — `Any` never refines a precise placeholder.
#[test]
fn deferred_return_more_specific_than_any() {
    let deferred = KType::KFunction {
        params: Record::new(),
        ret: Box::new(KType::DeferredReturn(DeferredReturnSurface::Type(
            TypeIdentifier::leaf("Er".into()),
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
        ret: Box::new(KType::DeferredReturn(DeferredReturnSurface::Type(
            TypeIdentifier::leaf("Er".into()),
        ))),
        body: None,
    };
    let ar = KType::KFunctor {
        params: Record::new(),
        ret: Box::new(KType::DeferredReturn(DeferredReturnSurface::Type(
            TypeIdentifier::leaf("Ar".into()),
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
        return_type: ReturnType::Deferred(DeferredReturn::Type(TypeIdentifier::leaf("Er".into()))),
        elements: vec![],
    };
    let no_params = Record::new();

    // Matching shadow → admit.
    let slot_er = KType::DeferredReturn(DeferredReturnSurface::Type(TypeIdentifier::leaf(
        "Er".into(),
    )));
    assert!(function_compat(&candidate, &no_params, &slot_er, false));

    // Differing shadow → reject.
    let slot_ar = KType::DeferredReturn(DeferredReturnSurface::Type(TypeIdentifier::leaf(
        "Ar".into(),
    )));
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
