use super::*;
use crate::builtins::test_support::spliced_part;
use crate::machine::model::ast::ExpressionPart;
use crate::machine::model::types::{NominalSchema, RecursiveSet};
use crate::machine::model::Carried;
use crate::machine::model::Record;
use std::rc::Rc;

/// A singleton-set `KType::SetRef` for a record-repr newtype (an ex-struct) named `name`
/// (empty record repr is fine — the predicates key on the nominal `(set digest, index)` +
/// `kind`, never a schema descent).
fn record_newtype_setref(name: &str) -> KType {
    let set = RecursiveSet::singleton(
        name.into(),
        NominalSchema::NewType(Box::new(KType::record(Box::new(Record::new())))),
    );
    KType::SetRef { set, index: 0 }
}

/// A singleton-set `KType::SetRef` for a newtype named `name` over `repr`.
fn newtype_setref(name: &str, repr: KType) -> KType {
    let set = RecursiveSet::singleton(name.into(), NominalSchema::NewType(Box::new(repr)));
    KType::SetRef { set, index: 0 }
}

#[test]
fn is_more_specific_concrete_beats_any() {
    let types = TypeRegistry::new();
    assert!(KType::Number.is_more_specific_than(&KType::Any, &types));
    assert!(!KType::Any.is_more_specific_than(&KType::Number, &types));
}

/// Dispatch treats two structurally identical nominal declarations interchangeably — the
/// content-digest identity a `NEWTYPE` elaborated twice (an FN body called twice) yields. Two
/// independently built same-content newtype sets (distinct allocations) satisfy each other's
/// slot, so a value of one is admitted where the other is declared.
#[test]
fn dispatch_unifies_structurally_identical_nominals() {
    let types = TypeRegistry::new();
    let slot = newtype_setref("Wrapper", KType::Number);
    let carried = newtype_setref("Wrapper", KType::Number);
    assert_eq!(
        slot, carried,
        "same content unifies regardless of allocation"
    );
    assert!(slot.satisfied_by(&carried, &types));
    assert!(carried.satisfied_by(&slot, &types));

    // A different declared name is genuinely different content, so it is not admitted.
    let other = newtype_setref("Boxer", KType::Number);
    assert!(!slot.satisfied_by(&other, &types));
}

#[test]
fn is_more_specific_list_number_beats_list_any() {
    let types = TypeRegistry::new();
    let n = KType::list(Box::new(KType::Number));
    let a = KType::list(Box::new(KType::Any));
    assert!(n.is_more_specific_than(&a, &types));
    assert!(!a.is_more_specific_than(&n, &types));
}

#[test]
fn is_more_specific_disjoint_lists_incomparable() {
    let types = TypeRegistry::new();
    let n = KType::list(Box::new(KType::Number));
    let s = KType::list(Box::new(KType::Str));
    assert!(!n.is_more_specific_than(&s, &types));
    assert!(!s.is_more_specific_than(&n, &types));
}

#[test]
fn is_more_specific_dict_refines_value() {
    let types = TypeRegistry::new();
    let strict = KType::dict(Box::new(KType::Str), Box::new(KType::Number));
    let loose = KType::dict(Box::new(KType::Str), Box::new(KType::Any));
    assert!(strict.is_more_specific_than(&loose, &types));
    assert!(!loose.is_more_specific_than(&strict, &types));
}

/// Width-subset specificity: a nullary function `{}` is strictly more specific than a
/// unary `{x}` (its param key set is a subset, so it fills the wider slot under
/// call-by-name width drop), and the unary is not more specific than the nullary
/// (the unary declares a param the nullary lacks → contravariant width violation).
#[test]
fn is_more_specific_function_width_subset() {
    let types = TypeRegistry::new();
    let unary = KType::function_type(
        Record::from_pairs(vec![("x".into(), KType::Number)]),
        Box::new(KType::Number),
    );
    let nullary = KType::function_type(Record::new(), Box::new(KType::Number));
    assert!(nullary.is_more_specific_than(&unary, &types));
    assert!(!unary.is_more_specific_than(&nullary, &types));
}

/// Depth-contravariant function specificity: `(x :Any) -> R ≺ (x :Number) -> R`. The
/// more-general param (`Any` ⊐ `Number`) makes the function more specific, because a
/// value accepting `Any` fills a slot that promised only `Number`.
#[test]
fn is_more_specific_function_param_contravariant() {
    let types = TypeRegistry::new();
    let any_param = KType::function_type(
        Record::from_pairs(vec![("x".into(), KType::Any)]),
        Box::new(KType::Str),
    );
    let number_param = KType::function_type(
        Record::from_pairs(vec![("x".into(), KType::Number)]),
        Box::new(KType::Str),
    );
    assert!(any_param.is_more_specific_than(&number_param, &types));
    assert!(!number_param.is_more_specific_than(&any_param, &types));
}

/// Return-covariant function specificity: `(x) -> Number ≺ (x) -> Any`. The narrower
/// return makes the function more specific.
#[test]
fn is_more_specific_function_return_covariant() {
    let types = TypeRegistry::new();
    let number_ret = KType::function_type(
        Record::from_pairs(vec![("x".into(), KType::Number)]),
        Box::new(KType::Number),
    );
    let any_ret = KType::function_type(
        Record::from_pairs(vec![("x".into(), KType::Number)]),
        Box::new(KType::Any),
    );
    assert!(number_ret.is_more_specific_than(&any_ret, &types));
    assert!(!any_ret.is_more_specific_than(&number_ret, &types));
}

fn record_ty(fields: Vec<(&str, KType)>) -> KType {
    KType::record(Box::new(Record::from_pairs(
        fields.into_iter().map(|(n, t)| (n.to_string(), t)),
    )))
}

/// Record-value subtyping is the *dual* of function-param subtyping: a *wider* record is
/// strictly more specific (a `{x, y}` value fills an `{x}` slot, dropping `y`).
#[test]
fn record_width_superset_more_specific() {
    let types = TypeRegistry::new();
    let wide = record_ty(vec![("x", KType::Number), ("y", KType::Str)]);
    let narrow = record_ty(vec![("x", KType::Number)]);
    assert!(wide.is_more_specific_than(&narrow, &types));
    assert!(!narrow.is_more_specific_than(&wide, &types));
}

/// Covariant depth: `:{x :Number} ≺ :{x :Any}`.
#[test]
fn record_depth_covariant() {
    let types = TypeRegistry::new();
    let number = record_ty(vec![("x", KType::Number)]);
    let any = record_ty(vec![("x", KType::Any)]);
    assert!(number.is_more_specific_than(&any, &types));
    assert!(!any.is_more_specific_than(&number, &types));
}

/// Disjoint field sets are incomparable (`{x, y}` vs `{x, z}`) — dispatch ambiguity, not
/// an ordering.
#[test]
fn record_disjoint_fields_incomparable() {
    let types = TypeRegistry::new();
    let xy = record_ty(vec![("x", KType::Number), ("y", KType::Str)]);
    let xz = record_ty(vec![("x", KType::Number), ("z", KType::Str)]);
    assert!(!xy.is_more_specific_than(&xz, &types));
    assert!(!xz.is_more_specific_than(&xy, &types));
}

/// `accepts_carried` is the classifier `accepts_part`'s `Spliced` arm
/// delegates to: a resolved value classifies identically whether reached as a spliced part or opened
/// directly. Also pins the value-shaped arms (object type-tag, type-channel `OfKind`) it owns.
#[test]
fn accepts_carried_matches_spliced_delegation() {
    let types = TypeRegistry::new();
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
            ty.accepts_carried(carried, &types),
            ty.accepts_part(&spliced_part(carried), &types)
        );
    }
    // A numeric value is admitted by `:Number` / `:Any`, refused by `:Str`.
    assert!(KType::Number.accepts_carried(Carried::Object(n), &types));
    assert!(KType::Any.accepts_carried(Carried::Object(n), &types));
    assert!(!KType::Str.accepts_carried(Carried::Object(n), &types));
    // A type-channel value reaches the `OfKind` arm; a proper-type slot admits it.
    let kt_number = KType::Number;
    assert!(KType::OfKind(KKind::ProperType).accepts_carried(Carried::Type(&kt_number), &types));
    // An object value reports a non-type `kind_of` and is refused by a type-channel slot.
    assert!(!KType::OfKind(KKind::ProperType).accepts_carried(Carried::Object(n), &types));
}

/// A spliced **cell** classifies through `accepts_part` by opening at its own brand and handing the
/// value to `accepts_carried` (no re-anchoring): a `7.0` value is admitted
/// by `:Number` / `:Any` and refused by `:Str`, matching a direct `accepts_carried`. Built through
/// the scope's own carrier surface (`resident_value_carrier` + `Sealed::seal`) — the exact
/// construction a real splice rests on the working expression. Also pins the cell's `is_splice_free`
/// (a resolved value is not raw AST, so the checked seal's guard rejects it).
#[test]
fn spliced_cell_classifies_by_opening() {
    let types = TypeRegistry::new();
    use crate::builtins::test_support::run_root_bare;
    use crate::machine::core::run_root_storage;
    use crate::machine::model::ast::KExpression;
    use crate::machine::model::values::KObject;
    use crate::witnessed::{Delivered, Sealed};

    let storage = run_root_storage();
    let scope = run_root_bare(&storage);
    let obj: &KObject = scope.brand().alloc_object(KObject::Number(7.0));
    let carrier = scope.resident_value_carrier(
        obj,
        crate::machine::core::StoredReach::for_test(None, false),
    );
    let cell_part = ExpressionPart::Spliced {
        cell: Delivered::hosted(Sealed::seal(carrier), std::rc::Rc::clone(&storage)),
    };

    for (ty, admits) in [
        (KType::Number, true),
        (KType::Any, true),
        (KType::Str, false),
    ] {
        assert_eq!(
            ty.accepts_part(&cell_part, &types),
            admits,
            "cell classification for {ty:?}",
        );
        // Agrees with opening the value directly.
        assert_eq!(
            ty.accepts_part(&cell_part, &types),
            ty.accepts_carried(Carried::Object(obj), &types)
        );
    }

    // A cell is a resolved value, not raw AST — the checked seal's splice-free guard rejects it.
    let expr = KExpression::new(vec![crate::source::Spanned::bare(cell_part)]);
    assert!(!expr.is_splice_free());
}

/// A `{x = 1, y = "a"}` value (carried type `:{x :Number, y :Str}`) admits and matches a
/// narrower `:{x :Number}` slot (width drop); rejects a field-type mismatch (`:{x :Str}`)
/// and a slot demanding a field the value lacks (`:{x :Number, q :Bool}`). A bare record
/// literal admits any record slot shape-only.
#[test]
fn record_value_admission_and_matches() {
    let types = TypeRegistry::new();
    use crate::machine::core::{run_root_storage, FrameStorageExt};
    let storage = run_root_storage();
    let region = storage.brand();
    let value: &KObject<'_> = region.alloc_object(KObject::record(
        Record::from_pairs(vec![
            ("x".to_string(), KObject::Number(1.0)),
            ("y".to_string(), KObject::KString("a".into())),
        ]),
        &types,
    ));

    let narrow = record_ty(vec![("x", KType::Number)]);
    assert!(narrow.accepts_part(&spliced_part(Carried::Object(value)), &types));
    assert!(narrow.matches_value(value, &types));

    let mismatch = record_ty(vec![("x", KType::Str)]);
    assert!(!mismatch.accepts_part(&spliced_part(Carried::Object(value)), &types));
    assert!(!mismatch.matches_value(value, &types));

    let extra = record_ty(vec![("x", KType::Number), ("q", KType::Bool)]);
    assert!(!extra.accepts_part(&spliced_part(Carried::Object(value)), &types));
    assert!(!extra.matches_value(value, &types));

    // Unevaluated literal admits shape-only (defer-then-reevaluate on the typed value).
    assert!(mismatch.accepts_part(&ExpressionPart::RecordLiteral(vec![]), &types));
}

/// Admission table for `KType::accepts_part`: bare builtin type tokens, newtype / union
/// `Carried::Type(SetRef)` identities, and a signature carrier all admit — a signature is a
/// type value, and `:Type` is the lattice top. The module value rejects (a module is a value,
/// reaching slots on the Object channel), a `:(OfKind Proper)` slot rejects the signature
/// (the proper tier is the non-signature tier), and non-type-denoting carriers reject.
#[test]
fn type_slot_admits_bare_builtin_tokens_and_user_type_carriers() {
    use crate::builtins::test_support::TestRun;
    use crate::machine::core::{run_root_storage, FrameStorageExt};
    use crate::machine::model::values::Module;
    use crate::machine::model::{SigContent, SigSchema};
    let region = run_root_storage();
    let test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let types = test_run.types.clone();
    let t = KType::OfKind(KKind::AnyType);
    let kt_number: &KType = region.brand().alloc_ktype(KType::Number);
    let kt_str: &KType = region.brand().alloc_ktype(KType::Str);
    let kt_bool: &KType = region.brand().alloc_ktype(KType::Bool);
    let kt_null: &KType = region.brand().alloc_ktype(KType::Null);
    assert!(t.accepts_part(&spliced_part(Carried::Type(kt_number)), &types));
    assert!(t.accepts_part(&spliced_part(Carried::Type(kt_str)), &types));
    assert!(t.accepts_part(&spliced_part(Carried::Type(kt_bool)), &types));
    assert!(t.accepts_part(&spliced_part(Carried::Type(kt_null)), &types));
    // NewType / union-variant type tokens flow as `SetRef { .. }` in the type channel — a
    // `:Type` slot admits them when the spliced cell opens to a `Carried::Type`.
    let newtype_set = RecursiveSet::singleton(
        "Some".into(),
        NominalSchema::NewType(Box::new(KType::Number)),
    );
    let newtype_token: &KType = region.brand().alloc_ktype(KType::SetRef {
        set: newtype_set,
        index: 0,
    });
    let struct_token: &KType = region.brand().alloc_ktype(record_newtype_setref("Point"));
    assert!(t.accepts_part(&spliced_part(Carried::Type(newtype_token)), &types));
    assert!(t.accepts_part(&spliced_part(Carried::Type(struct_token)), &types));
    let child = region
        .brand()
        .alloc_scope(crate::machine::Scope::child_under_module(
            scope,
            "IntMod".into(),
        ));
    let module = region
        .brand()
        .alloc_module(Module::new("IntMod".into(), child));
    // A module is a value: it reaches a slot on the Object channel, and a `:Type` slot refuses it.
    let module_value = region
        .brand()
        .alloc_object_checked(KObject::Module(module), &types)
        .expect("module was just allocated into region's own region");
    assert!(!t.accepts_part(&spliced_part(Carried::Object(module_value)), &types));
    let sig_scope = region
        .brand()
        .alloc_scope(crate::machine::Scope::child_under_sig(
            scope,
            "Ordered".into(),
        ));
    let schema = SigSchema::project_decl(sig_scope);
    let content = Rc::new(SigContent::new("Ordered".into(), sig_scope.id, schema));
    let kt_sig: &KType = region
        .brand()
        .alloc_ktype(KType::signature(content, Vec::new()));
    // A signature is a type value: the `:Type` lattice top admits it; the proper tier does not.
    assert!(t.accepts_part(&spliced_part(Carried::Type(kt_sig)), &types));
    assert!(!KType::OfKind(KKind::ProperType)
        .accepts_part(&spliced_part(Carried::Type(kt_sig)), &types));
    let n: &KObject<'_> = region.brand().alloc_object(KObject::Number(7.0));
    let s: &KObject<'_> = region.brand().alloc_object(KObject::KString("hi".into()));
    assert!(!t.accepts_part(&spliced_part(Carried::Object(n)), &types));
    assert!(!t.accepts_part(&spliced_part(Carried::Object(s)), &types));
}

/// `:Signature` sits strictly below the `:Type` lattice top: a signature-slotted overload
/// out-specifies a `:Type` sibling when both admit a signature value, and the reverse fails.
#[test]
fn of_kind_signature_more_specific_than_any_type() {
    let types = TypeRegistry::new();
    assert!(KType::OfKind(KKind::Signature)
        .is_more_specific_than(&KType::OfKind(KKind::AnyType), &types));
    assert!(!KType::OfKind(KKind::AnyType)
        .is_more_specific_than(&KType::OfKind(KKind::Signature), &types));
}

/// `OfKind` is type-channel-only: a nominal-kind slot classifies a *type value* by its
/// `kind_of`, and never matches a runtime instance (a value is matched by a type, not a kind).
/// `OfKind(NewType)` admits a NewType *type* value, declines a `TypeConstructor` type value, and
/// declines the runtime `Wrapped` *instance* entirely; `OfKind(Proper)` subsumes the NewType type.
#[test]
fn of_kind_nominal_is_type_channel_only() {
    let types = TypeRegistry::new();
    use crate::machine::core::{run_root_storage, FrameStorageExt};
    let storage = run_root_storage();
    let region = storage.brand();
    let newtype_ty = KType::OfKind(KKind::NewType);

    // The NewType *type value* — admitted in the type channel.
    let newtype_tv = newtype_setref("Distance", KType::Number);
    assert!(newtype_ty.accepts_part(&spliced_part(Carried::Type(&newtype_tv)), &types));
    assert!(KType::OfKind(KKind::ProperType)
        .accepts_part(&spliced_part(Carried::Type(&newtype_tv)), &types));

    // A `TypeConstructor` type value is the wrong family — declined.
    let ctor_tv = KType::SetRef {
        set: RecursiveSet::singleton(
            "Result".into(),
            NominalSchema::TypeConstructor {
                schema: std::collections::HashMap::new(),
                param_names: Vec::new(),
            },
        ),
        index: 0,
    };
    assert!(!newtype_ty.accepts_part(&spliced_part(Carried::Type(&ctor_tv)), &types));

    // The runtime `Wrapped` *instance* is never matched by a kind slot.
    let inner: &KObject<'_> = region.alloc_object(KObject::Number(3.0));
    let type_id: &KType = region.alloc_ktype(newtype_tv.clone());
    let w: &KObject<'_> = region
        .alloc_object_checked(
            KObject::Wrapped {
                inner: crate::machine::model::values::WrappedPayload::peel(inner),
                type_id,
            },
            &types,
        )
        .expect("type_id was just allocated into region's own region");
    assert!(!newtype_ty.accepts_part(&spliced_part(Carried::Object(w)), &types));
    assert!(!newtype_ty.matches_value(w, &types));
}

/// Pins the kind refinement: a `NewType`-kind `SetRef` is strictly more specific than
/// `OfKind(NewType)`, and incomparable with `OfKind(TypeConstructor)` (a sibling family).
#[test]
fn user_type_newtype_specificity_lattice() {
    let types = TypeRegistry::new();
    let newtype_kind = KType::OfKind(KKind::NewType);
    let ctor_kind = KType::OfKind(KKind::TypeConstructor);
    let dist = newtype_setref("Distance", KType::Number);
    assert!(dist.is_more_specific_than(&newtype_kind, &types));
    assert!(!newtype_kind.is_more_specific_than(&dist, &types));
    assert!(!dist.is_more_specific_than(&ctor_kind, &types));
    assert!(!ctor_kind.is_more_specific_than(&dist, &types));
}

/// Specificity ordering for `SetRef` against the `OfKind` kind lattice:
/// - a nominal kind is strictly under `Any` and strictly under `OfKind(Proper)`;
/// - a `SetRef` member of kind `K` is strictly under `OfKind(K)`;
/// - a `SetRef` of one kind and `OfKind` of a different kind are incomparable.
#[test]
fn user_type_specificity_lattice() {
    let types = TypeRegistry::new();
    let newtype_kind = KType::OfKind(KKind::NewType);
    let ctor_kind = KType::OfKind(KKind::TypeConstructor);
    let point = record_newtype_setref("Point");
    // A nominal kind strictly under `Any` and under `OfKind(Proper)`.
    assert!(newtype_kind.is_more_specific_than(&KType::Any, &types));
    assert!(!KType::Any.is_more_specific_than(&newtype_kind, &types));
    assert!(newtype_kind.is_more_specific_than(&KType::OfKind(KKind::ProperType), &types));
    assert!(!KType::OfKind(KKind::ProperType).is_more_specific_than(&newtype_kind, &types));
    // A `NewType`-kind `SetRef` strictly under `OfKind(NewType)`.
    assert!(point.is_more_specific_than(&newtype_kind, &types));
    assert!(!newtype_kind.is_more_specific_than(&point, &types));
    // Different-kind pairs incomparable.
    assert!(!point.is_more_specific_than(&ctor_kind, &types));
    assert!(!ctor_kind.is_more_specific_than(&point, &types));
}

/// `KType::Signature { pinned_slots }` specificity rules (constraint role):
/// - A non-empty `pinned_slots` strictly refines an empty same-`sig_id` form when
///   every pin in the empty side appears (with equal `KType`) in the non-empty side.
/// - Different `sig_id`s compare by structural `sig_subtype`: two structurally-identical
///   distinct SIGs are mutually-satisfying, hence incomparable (neither strictly refines).
/// - Same `sig_id` with disjoint constraint keys is incomparable.
/// - Same-key-different-`KType` is incomparable.
/// - A `Signature` (pinned or not) strictly refines `OfKind(Module)`.
#[test]
fn is_more_specific_for_pinned_signature_bound() {
    use crate::builtins::test_support::TestRun;
    use crate::machine::core::{run_root_storage, FrameStorageExt};
    use crate::machine::model::{SigContent, SigSchema};
    let region = run_root_storage();
    let test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let types = test_run.types.clone();
    // Two distinct decl_scopes → two distinct `sig_id`s.
    let ordered_scope = region
        .brand()
        .alloc_scope(crate::machine::Scope::child_under_sig(
            scope,
            "Ordered".into(),
        ));
    let hashed_scope = region
        .brand()
        .alloc_scope(crate::machine::Scope::child_under_sig(
            scope,
            "Hashed".into(),
        ));
    let ordered = Rc::new(SigContent::new(
        "Ordered".into(),
        ordered_scope.id,
        SigSchema::project_decl(ordered_scope),
    ));
    let hashed = Rc::new(SigContent::new(
        "Hashed".into(),
        hashed_scope.id,
        SigSchema::project_decl(hashed_scope),
    ));

    let bare = KType::signature(Rc::clone(&ordered), Vec::new());
    let pinned_number = KType::signature(Rc::clone(&ordered), vec![("Type".into(), KType::Number)]);
    let pinned_str = KType::signature(Rc::clone(&ordered), vec![("Type".into(), KType::Str)]);
    let pinned_two = KType::signature(
        Rc::clone(&ordered),
        vec![("Type".into(), KType::Number), ("Elt".into(), KType::Str)],
    );
    let other_sig = KType::signature(hashed, vec![("Type".into(), KType::Number)]);
    let pinned_elt = KType::signature(ordered, vec![("Elt".into(), KType::Number)]);

    assert!(pinned_number.is_more_specific_than(&bare, &types));
    assert!(!bare.is_more_specific_than(&pinned_number, &types));
    assert!(pinned_two.is_more_specific_than(&pinned_number, &types));
    assert!(!pinned_number.is_more_specific_than(&pinned_two, &types));
    assert!(!pinned_number.is_more_specific_than(&pinned_str, &types));
    assert!(!pinned_str.is_more_specific_than(&pinned_number, &types));
    assert!(!pinned_number.is_more_specific_than(&pinned_elt, &types));
    assert!(!pinned_elt.is_more_specific_than(&pinned_number, &types));
    assert!(!pinned_number.is_more_specific_than(&other_sig, &types));
    assert!(!other_sig.is_more_specific_than(&pinned_number, &types));
}

/// A shared `Result` `TypeConstructor` set. Identity is now `(set ptr, index)`, so a
/// `ConstructorApply` ctor and a `Tagged` carrier only match when they reference the *same*
/// `Rc` — every test below threads this one set through both the slot ctor and the value.
fn result_set() -> Rc<RecursiveSet> {
    RecursiveSet::singleton(
        "Result".into(),
        NominalSchema::TypeConstructor {
            schema: std::collections::HashMap::new(),
            param_names: vec!["Ok".into(), "Error".into()],
        },
    )
}

/// The args record for a `Result` application, keyed by the carrier's parameter names.
fn result_args(ok: KType, error: KType) -> Record<KType> {
    Record::from_pairs([("Ok".to_string(), ok), ("Error".to_string(), error)])
}

/// Build a `Result`-carrier `Tagged` value occupying `tag` with `payload`, referencing
/// `set` (the shared `Result` allocation). The inner `payload` is itself a `Tagged` carrier
/// whose set is the error type's nominal identity.
fn result_value<'a>(set: &Rc<RecursiveSet>, tag: &str, payload: KObject<'a>) -> KObject<'a> {
    KObject::Tagged {
        tag: tag.into(),
        value: std::rc::Rc::new(payload),
        set: Rc::clone(set),
        index: 0,
        type_args: std::rc::Rc::new(Record::new()),
    }
}

/// A bare error carrier (`Tagged` over `set`) standing in for a caught error value.
fn error_carrier<'a>(set: &Rc<RecursiveSet>) -> KObject<'a> {
    KObject::Tagged {
        tag: "_".into(),
        value: std::rc::Rc::new(KObject::Number(0.0)),
        set: Rc::clone(set),
        index: 0,
        type_args: std::rc::Rc::new(Record::new()),
    }
}

/// A singleton `TypeConstructor`-kind set named `name`, for an error-type identity.
fn error_type_set(name: &str) -> Rc<RecursiveSet> {
    RecursiveSet::singleton(
        name.into(),
        NominalSchema::TypeConstructor {
            schema: std::collections::HashMap::new(),
            param_names: Vec::new(),
        },
    )
}

/// `:(Result {Ok = …, Error = …})` slot admission: a `ConstructorApply` slot whose ctor
/// identity matches the `Result` carrier admits an `Error(...)` value iff the inhabited
/// `Error` payload satisfies the slot's same-named arg. A caught `Error(KError)` is rejected
/// where that arg is `MyError` and accepted where it is `KError` / `Any`. Identity is
/// allocation-based, so the slot's arg and the value's payload carrier share one set per
/// error type.
#[test]
fn constructor_apply_result_checks_inhabited_error_param() {
    let types = TypeRegistry::new();

    let r_set = result_set();
    let ctor = Box::new(KType::SetRef {
        set: Rc::clone(&r_set),
        index: 0,
    });
    let kerror_set = error_type_set("KError");
    let my_error_set = error_type_set("MyError");
    let my_error_ty = KType::SetRef {
        set: Rc::clone(&my_error_set),
        index: 0,
    };
    let kerror_ty = KType::SetRef {
        set: Rc::clone(&kerror_set),
        index: 0,
    };

    let slot_my_error =
        KType::constructor_apply(ctor.clone(), result_args(KType::Any, my_error_ty.clone()));
    let caught = result_value(&r_set, "Error", error_carrier(&kerror_set));
    assert!(!slot_my_error.matches_value(&caught, &types));

    let slot_kerror =
        KType::constructor_apply(ctor.clone(), result_args(KType::Any, kerror_ty.clone()));
    assert!(slot_kerror.matches_value(&caught, &types));

    let my_error = result_value(&r_set, "Error", error_carrier(&my_error_set));
    assert!(slot_my_error.matches_value(&my_error, &types));
}

/// The `Ok` tag names the `Ok` parameter, so a slot checks the `Ok` payload against its
/// `Ok` arg regardless of the `Error` arg: an `Ok(42)` value admits any `Error` arg (the
/// uninhabited tag's parameter is unconstrained at the value).
#[test]
fn constructor_apply_result_ok_admits_any_error_param() {
    let types = TypeRegistry::new();
    let r_set = result_set();
    let ctor = Box::new(KType::SetRef {
        set: Rc::clone(&r_set),
        index: 0,
    });
    let my_error_ty = KType::SetRef {
        set: error_type_set("MyError"),
        index: 0,
    };
    let ok_value = result_value(&r_set, "Ok", KObject::Number(42.0));
    let slot = KType::constructor_apply(ctor.clone(), result_args(KType::Number, my_error_ty));
    assert!(slot.matches_value(&ok_value, &types));
    let slot_str = KType::constructor_apply(ctor, result_args(KType::Str, KType::Any));
    assert!(!slot_str.matches_value(&ok_value, &types));
}

/// Covariance for `ConstructorApply` carriers: a value stamped
/// `{Ok = Number, Error = MyError}` is admitted by the coarser `{Ok = Any, Error = Any}`
/// slot, and the refined slot is strictly more specific, so dispatch tie-breaks toward the
/// refined overload.
#[test]
fn constructor_apply_covariant_admission_and_specificity() {
    let types = TypeRegistry::new();
    let r_set = result_set();
    let ctor = Box::new(KType::SetRef {
        set: Rc::clone(&r_set),
        index: 0,
    });
    let my_error = KType::SetRef {
        set: error_type_set("MyError"),
        index: 0,
    };
    let stamped = KObject::Tagged {
        tag: "Ok".into(),
        value: std::rc::Rc::new(KObject::Number(1.0)),
        set: Rc::clone(&r_set),
        index: 0,
        type_args: std::rc::Rc::new(result_args(KType::Number, my_error.clone())),
    };
    let coarse = KType::constructor_apply(ctor.clone(), result_args(KType::Any, KType::Any));
    let refined = KType::constructor_apply(ctor, result_args(KType::Number, my_error));
    assert!(coarse.matches_value(&stamped, &types));
    assert!(refined.matches_value(&stamped, &types));
    assert!(refined.is_more_specific_than(&coarse, &types));
    assert!(!coarse.is_more_specific_than(&refined, &types));
}

/// A populated `type_args` carrier (stamped by ascription) is checked structurally against
/// the slot args, taking precedence over the inhabited-tag path.
#[test]
fn constructor_apply_stamped_type_args_checked_structurally() {
    let types = TypeRegistry::new();
    let r_set = result_set();
    let ctor = Box::new(KType::SetRef {
        set: Rc::clone(&r_set),
        index: 0,
    });
    let stamped = KObject::Tagged {
        tag: "Ok".into(),
        value: std::rc::Rc::new(KObject::Number(1.0)),
        set: Rc::clone(&r_set),
        index: 0,
        type_args: std::rc::Rc::new(result_args(KType::Number, KType::Str)),
    };
    let slot_ok = KType::constructor_apply(ctor.clone(), result_args(KType::Number, KType::Str));
    assert!(slot_ok.matches_value(&stamped, &types));
    let slot_any = KType::constructor_apply(ctor.clone(), result_args(KType::Any, KType::Any));
    assert!(slot_any.matches_value(&stamped, &types));
    let slot_bad = KType::constructor_apply(ctor, result_args(KType::Bool, KType::Str));
    assert!(!slot_bad.matches_value(&stamped, &types));
}

use crate::machine::model::ast::TypeIdentifier;
use crate::machine::model::types::{DeferredReturn, DeferredReturnSurface, ReturnType};

/// A function whose `ret` slot is a `DeferredReturn` carrier is strictly more specific
/// than the same shape with an `Any` return (covariant short-circuit), and the reverse
/// does not hold — `Any` never refines a precise placeholder.
#[test]
fn deferred_return_more_specific_than_any() {
    let types = TypeRegistry::new();
    let deferred = KType::function_type(
        Record::new(),
        Box::new(KType::DeferredReturn(DeferredReturnSurface::Type(
            TypeIdentifier::leaf("er".into()),
        ))),
    );
    let any = KType::function_type(Record::new(), Box::new(KType::Any));
    assert!(deferred.is_more_specific_than(&any, &types));
    assert!(!any.is_more_specific_than(&deferred, &types));
}

/// Two function types differing only in their deferred-return shadow are distinct: not equal,
/// neither more specific than the other, and they hash apart.
#[test]
fn two_functions_differ_only_in_deferred_return_are_distinct() {
    let types = TypeRegistry::new();
    use std::hash::{Hash, Hasher};
    let er = KType::function_type(
        Record::new(),
        Box::new(KType::DeferredReturn(DeferredReturnSurface::Type(
            TypeIdentifier::leaf("er".into()),
        ))),
    );
    let ar = KType::function_type(
        Record::new(),
        Box::new(KType::DeferredReturn(DeferredReturnSurface::Type(
            TypeIdentifier::leaf("Ar".into()),
        ))),
    );
    assert_ne!(er, ar);
    assert!(!er.is_more_specific_than(&ar, &types));
    assert!(!ar.is_more_specific_than(&er, &types));
    // `KType` carries interior mutability, so it can't key a `HashSet` (clippy
    // `mutable_key_type`). Hash each directly: the deferred-return shadow participates
    // in `KType`'s hash, so the two function types hash apart.
    let hash = |k: &KType| {
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
    let types = TypeRegistry::new();
    let candidate = ExpressionSignature {
        return_type: ReturnType::Deferred(DeferredReturn::Type(TypeIdentifier::leaf("er".into()))),
        elements: vec![],
    };
    let no_params = Record::new();

    // Matching shadow → admit.
    let slot_er = KType::DeferredReturn(DeferredReturnSurface::Type(TypeIdentifier::leaf(
        "er".into(),
    )));
    assert!(function_compat(&candidate, &no_params, &slot_er, &types));

    // Differing shadow → reject.
    let slot_ar = KType::DeferredReturn(DeferredReturnSurface::Type(TypeIdentifier::leaf(
        "Ar".into(),
    )));
    assert!(!function_compat(&candidate, &no_params, &slot_ar, &types));

    // Resolved slot → reject (opaque until elaboration).
    assert!(!function_compat(
        &candidate,
        &no_params,
        &KType::Number,
        &types
    ));

    // `Any` slot → admit.
    assert!(function_compat(&candidate, &no_params, &KType::Any, &types));
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
    let a = DeferredReturnSurface::Expression("ATTR er Type".into());
    let b = DeferredReturnSurface::Expression("ATTR er Type".into());
    let c = DeferredReturnSurface::Expression("ATTR Ar Type".into());
    assert_eq!(a, b);
    assert_eq!(h(&a), h(&b));
    assert_ne!(a, c);
}

// --- KType::Union admissibility and specificity ------------------------------------

/// A union slot admits a value any of its members admits, and refuses one no member
/// admits — via both `accepts_carried` and `matches_value`.
#[test]
fn union_admits_member_typed_value() {
    let types = TypeRegistry::new();
    use crate::machine::core::{run_root_storage, FrameStorageExt};
    let storage = run_root_storage();
    let region = storage.brand();
    let n: &KObject<'_> = region.alloc_object(KObject::Number(7.0));

    let number_or_str = KType::union_of(vec![KType::Number, KType::Str], &types);
    let str_or_bool = KType::union_of(vec![KType::Str, KType::Bool], &types);

    assert!(number_or_str.accepts_carried(Carried::Object(n), &types));
    assert!(!str_or_bool.accepts_carried(Carried::Object(n), &types));
    // `matches_value` agrees with `accepts_carried`.
    assert!(number_or_str.matches_value(n, &types));
    assert!(!str_or_bool.matches_value(n, &types));
}

/// A union honors a container value's memoized carried element type: a `List<Number>`
/// value is admitted by a union containing `:(LIST OF Number)`, refused by one without it.
#[test]
fn union_honors_memoized_list_element_type() {
    let types = TypeRegistry::new();
    use crate::machine::core::{run_root_storage, FrameStorageExt};
    let storage = run_root_storage();
    let region = storage.brand();
    let list_value: &KObject<'_> = region.alloc_object(KObject::list_with_type(
        Rc::new(vec![Held::Object(KObject::Number(1.0))]),
        KType::Number,
    ));

    let with_list = KType::union_of(
        vec![KType::list(Box::new(KType::Number)), KType::Str],
        &types,
    );
    let without_list = KType::union_of(vec![KType::Number, KType::Str], &types);

    assert!(with_list.accepts_carried(Carried::Object(list_value), &types));
    assert!(!without_list.accepts_carried(Carried::Object(list_value), &types));
}

/// Specificity: each member refines its union (AC3); a union refines `Any` and a superset
/// union; a union is not more specific than a bare member nor than an equal union.
#[test]
fn union_specificity_ordering() {
    let types = TypeRegistry::new();
    let number = KType::Number;
    let number_or_str = KType::union_of(vec![KType::Number, KType::Str], &types);
    let number_or_str_or_bool =
        KType::union_of(vec![KType::Number, KType::Str, KType::Bool], &types);

    // Each member is a subtype of the union.
    assert!(number.is_more_specific_than(&number_or_str, &types));
    // A union refines `Any`.
    assert!(number_or_str.is_more_specific_than(&KType::Any, &types));
    // A union is not more specific than one of its members.
    assert!(!number_or_str.is_more_specific_than(&number, &types));
    // A subset union refines a superset union; the reverse does not hold.
    assert!(number_or_str.is_more_specific_than(&number_or_str_or_bool, &types));
    assert!(!number_or_str_or_bool.is_more_specific_than(&number_or_str, &types));
    // Equal unions (order-blind) are not strictly more specific than each other.
    let str_or_number = KType::union_of(vec![KType::Str, KType::Number], &types);
    assert!(!number_or_str.is_more_specific_than(&str_or_number, &types));
}

/// A module value's `ktype()` reports `Signature { SelfOf(m) }`, and its identity is its self-sig
/// *content*: two modules with identical interfaces share one type, a differing member
/// distinguishes them.
#[test]
fn module_object_ktype_reports_self_sig() {
    use crate::builtins::test_support::TestRun;
    use crate::machine::core::{run_root_storage, FrameStorageExt};
    use crate::machine::model::values::Module;
    use crate::machine::model::KObject;
    use crate::machine::Scope;
    let region = run_root_storage();
    let test_run = TestRun::silent(&region);
    let scope = test_run.scope;

    let child = region
        .brand()
        .alloc_scope(Scope::child_under_module(scope, "Mod".into()));
    let m: &Module = region
        .brand()
        .alloc_module(Module::new("Mod".into(), child));
    m.type_members
        .borrow_mut()
        .insert("Elt".into(), KType::Number);
    let kt = KObject::Module(m).ktype();
    assert!(matches!(
        &kt,
        KType::Signature { content, pinned_slots, .. }
            if content.sig_id == m.scope_id() && pinned_slots.is_empty()
    ));
    // Identity is content: the same module equals its own re-derived signature.
    assert_eq!(
        kt,
        KType::signature(Rc::clone(m.self_sig_content()), Vec::new())
    );

    // A second module with the identical interface shares the type — content, not mint.
    let child2 = region
        .brand()
        .alloc_scope(Scope::child_under_module(scope, "Mod2".into()));
    let m2: &Module = region
        .brand()
        .alloc_module(Module::new("Mod2".into(), child2));
    m2.type_members
        .borrow_mut()
        .insert("Elt".into(), KType::Number);
    assert_eq!(kt, KObject::Module(m2).ktype());

    // A module whose interface differs by one member is a distinct type.
    let child3 = region
        .brand()
        .alloc_scope(Scope::child_under_module(scope, "Mod3".into()));
    let m3: &Module = region
        .brand()
        .alloc_module(Module::new("Mod3".into(), child3));
    m3.type_members
        .borrow_mut()
        .insert("Elt".into(), KType::Str);
    assert_ne!(kt, KObject::Module(m3).ktype());
}

/// `matches_value` admits a module *object* into a `Signature` slot: a `Declared` slot by
/// structural satisfaction (+ pin agreement), an `Empty` slot for any module and no non-module
/// value.
#[test]
fn matches_value_admits_module_object_via_signature_slot() {
    use crate::builtins::test_support::TestRun;
    use crate::machine::core::{run_root_storage, FrameStorageExt};
    use crate::machine::model::values::Module;
    use crate::machine::model::KObject;
    use crate::machine::model::{SigContent, SigSchema};
    use crate::machine::Scope;
    let region = run_root_storage();
    let test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let types = test_run.types.clone();

    // An empty signature (empty decl scope): every module bare-satisfies it, so the pins gate.
    let sig_scope = region
        .brand()
        .alloc_scope(Scope::child_under_sig(scope, "S".into()));
    let sig = Rc::new(SigContent::new(
        "S".into(),
        sig_scope.id,
        SigSchema::project_decl(sig_scope),
    ));

    let child = region
        .brand()
        .alloc_scope(Scope::child_under_module(scope, "M".into()));
    let m: &Module = region.brand().alloc_module(Module::new("M".into(), child));
    m.type_members
        .borrow_mut()
        .insert("Type".into(), KType::Number);

    let declared = KType::signature(Rc::clone(&sig), Vec::new());
    assert!(declared.matches_value(&KObject::Module(m), &types));

    let pinned_ok = KType::signature(Rc::clone(&sig), vec![("Type".into(), KType::Number)]);
    let pinned_bad = KType::signature(sig, vec![("Type".into(), KType::Str)]);
    assert!(pinned_ok.matches_value(&KObject::Module(m), &types));
    assert!(!pinned_bad.matches_value(&KObject::Module(m), &types));

    let empty = KType::empty_signature();
    assert!(empty.matches_value(&KObject::Module(m), &types));
    assert!(!empty.matches_value(&KObject::Number(1.0), &types));
}

/// Specificity over the module lattice: a module's `SelfOf` self-sig refines a `Declared`
/// signature it satisfies, and any non-empty signature refines the `Empty` top. The signature
/// and module carry real members: under content identity a member-less signature *is* the
/// `:Module` top ([`empty_signature`](KType::empty_signature)), so degenerate empty points would
/// collapse into one type and there would be no ordering to test.
#[test]
fn specificity_self_sig_refines_declared_and_empty() {
    use crate::builtins::test_support::{lookup_module, TestRun};
    use crate::machine::run_root_storage;
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let types = test_run.types.clone();

    // `Ordered` requires a `compare` slot; `int_ord` supplies it plus an extra member, so its
    // self-sig strictly satisfies `Ordered`.
    test_run.run(
        "SIG Ordered = ((VAL compare :Number))\n\
         MODULE int_ord = ((LET compare = 7) (LET extra = 1))",
    );
    let sig = match scope.resolve_type("Ordered") {
        Some(KType::Signature { content, .. }) => Rc::clone(content),
        _ => panic!("Ordered must bind a Signature KType"),
    };
    let m = lookup_module(scope, "int_ord", &types);

    let self_of = KType::signature(Rc::clone(m.self_sig_content()), Vec::new());
    let declared = KType::signature(sig, Vec::new());
    let empty = KType::empty_signature();

    // `SelfOf(m) ≺ Declared(sig)` because `m`'s self-sig satisfies `Ordered`.
    assert!(self_of.is_more_specific_than(&declared, &types));
    // Any non-empty signature `≺ Empty`; `Empty` refines nothing narrower.
    assert!(declared.is_more_specific_than(&empty, &types));
    assert!(self_of.is_more_specific_than(&empty, &types));
    assert!(!empty.is_more_specific_than(&declared, &types));
    // `satisfied_by` routes a memoized `SelfOf` element type through the `SelfOf ≺ Declared` arm.
    assert!(declared.satisfied_by(&self_of, &types));
}

/// A member-free declared signature and a module with the matching slot shape are ONE type: the
/// schema digest feeds only the member/slot content — never `sig_id` or `path` — so the module's
/// self-sig type equals the declared signature by digest, not merely by mutual satisfaction.
#[test]
fn self_sig_type_equals_member_free_declared_sig() {
    use crate::builtins::test_support::{lookup_module, TestRun};
    use crate::machine::model::KObject;
    use crate::machine::run_root_storage;
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let types = test_run.types.clone();

    test_run.run(
        "SIG HasLabel = ((VAL label :Str))\n\
         MODULE widget = ((LET label = (\"button\")))",
    );
    let declared = scope
        .resolve_type("HasLabel")
        .expect("HasLabel must bind a type");
    let m = lookup_module(scope, "widget", &types);
    assert_eq!(
        &KObject::Module(m).ktype(),
        declared,
        "a module's self-sig type must digest-equal the member-free declared sig of its shape",
    );
}

/// A fully-manifest declared signature — every type member fixed, a slot typed through one of
/// those members — digest-equals the self-sig of a module with the identical shape. Pins the
/// projection ruling: a self-sig's type members are **manifest** (concrete), so a module's type
/// coincides with the concrete signature describing it, and a SIG-body slot `:Elem` resolves
/// through the manifest member to the same slot type the module's binding derives.
#[test]
fn self_sig_type_equals_fully_manifest_declared_sig() {
    use crate::builtins::test_support::{lookup_module, TestRun};
    use crate::machine::model::KObject;
    use crate::machine::run_root_storage;
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let types = test_run.types.clone();

    test_run.run(
        "SIG Pinned = ((LET Elem = Number) (VAL x :Elem))\n\
         MODULE pinned_mod = ((LET Elem = Number) (LET x = 5))",
    );
    let declared = scope
        .resolve_type("Pinned")
        .expect("Pinned must bind a type");
    let m = lookup_module(scope, "pinned_mod", &types);
    assert_eq!(
        &KObject::Module(m).ktype(),
        declared,
        "a module's self-sig type must digest-equal the fully-manifest declared sig of its shape",
    );
}

/// The abstract variant of the same interface stays a DISTINCT type — a self-sig never projects a
/// type member abstract — and the pair is strictly ordered, not mutually satisfying: the module's
/// manifest `Elem` witnesses the sig's abstract slot, while the sig's abstract `Elem` cannot
/// witness the self-sig's manifest requirement. Under an abstract projection this pair would be
/// digest-equal yet verdict-divergent across modules, breaking digest-is-identity.
#[test]
fn self_sig_stays_distinct_from_and_refines_abstract_sig() {
    use crate::builtins::test_support::{lookup_module, TestRun};
    use crate::machine::model::KObject;
    use crate::machine::run_root_storage;
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let types = test_run.types.clone();

    test_run.run(
        "SIG Abstracted = ((TYPE Elem) (VAL x :Elem))\n\
         MODULE concrete = ((LET Elem = Number) (LET x = 5))",
    );
    let declared = scope
        .resolve_type("Abstracted")
        .expect("Abstracted must bind a type");
    let m = lookup_module(scope, "concrete", &types);
    let self_of = KObject::Module(m).ktype();

    assert_ne!(
        &self_of, declared,
        "an abstract declared sig is a distinct type from any self-sig",
    );
    assert!(
        self_of.is_more_specific_than(declared, &types),
        "the manifest self-sig strictly refines the abstract sig it satisfies",
    );
    assert!(
        !declared.is_more_specific_than(&self_of, &types),
        "the abstract sig must not refine the manifest self-sig back — the pair is ordered, \
         not mutually satisfying",
    );
}

// --- verdict-registry wiring (`is_more_specific_than` routes composite pairs through the run's
// `TypeRegistry`) ------------------------------------------------------------------------------

/// A repeat check of the same composite pair (`List<Number>` vs `List<Any>`, verdict `true`)
/// is a counter-verified registry hit on the second call, and the verdict is identical both times.
#[test]
fn verdict_repeat_composite_hit() {
    let types = TypeRegistry::new();
    let n = KType::list(Box::new(KType::Number));
    let a = KType::list(Box::new(KType::Any));

    let first = n.is_more_specific_than(&a, &types);
    assert!(first);
    assert_eq!(types.miss_count(), 1);
    assert_eq!(types.hit_count(), 0);

    let second = n.is_more_specific_than(&a, &types);
    assert_eq!(second, first);
    assert_eq!(types.hit_count(), 1, "second call must be a registry hit");
}

/// A negative verdict is recorded too: the second call to a pair the walk resolves `false` for
/// is a hit returning `false`.
#[test]
fn verdict_negative_also_recorded() {
    let types = TypeRegistry::new();
    let a = KType::list(Box::new(KType::Any));
    let n = KType::list(Box::new(KType::Number));

    let first = a.is_more_specific_than(&n, &types);
    assert!(!first);
    assert_eq!(types.miss_count(), 1);

    let second = a.is_more_specific_than(&n, &types);
    assert!(!second);
    assert_eq!(types.hit_count(), 1, "second call must be a registry hit");
}

/// Leaf pairs never probe the registry: `Number` vs `Any` takes `is_stored_digest_variant`'s
/// `else` branch (`Any` is not a stored-digest variant), so both counters stay at zero.
#[test]
fn verdict_leaf_pairs_move_no_counters() {
    let types = TypeRegistry::new();
    assert!(KType::Number.is_more_specific_than(&KType::Any, &types));
    assert_eq!(types.hit_count(), 0);
    assert_eq!(types.miss_count(), 0);
}

/// Purity sanity: a cold registry computes the same composite verdict a warm one does — the
/// registry is an accelerator, never load-bearing.
#[test]
fn verdict_purity_across_a_cold_registry() {
    let warm = TypeRegistry::new();
    let n = KType::list(Box::new(KType::Number));
    let a = KType::list(Box::new(KType::Any));

    let before = n.is_more_specific_than(&a, &warm);
    let cold = TypeRegistry::new();
    let after = n.is_more_specific_than(&a, &cold);
    assert_eq!(before, after);
}
