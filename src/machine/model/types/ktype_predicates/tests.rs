use super::*;
use crate::builtins::test_support::spliced_part;
use crate::machine::model::ast::ExpressionPart;
use crate::machine::model::types::{RecursiveGroupWindow, RelativeSchema};
use crate::machine::model::Carried;
use crate::machine::model::Record;
use std::rc::Rc;

/// A singleton newtype member handle for a record-repr newtype (an ex-struct) named `name`
/// (empty record repr is fine — the predicates key on the sealed member's `(component digest,
/// index)` + `kind`, never a schema descent).
fn record_newtype_member(name: &str, types: &TypeRegistry) -> KType {
    let repr = types.record(Record::new());
    RecursiveGroupWindow::seal_singleton(name.into(), RelativeSchema::NewType(repr), None, types)
}

/// A singleton newtype member handle named `name` over `repr`.
fn newtype_member(name: &str, repr: KType, types: &TypeRegistry) -> KType {
    RecursiveGroupWindow::seal_singleton(name.into(), RelativeSchema::NewType(repr), None, types)
}

#[test]
fn is_more_specific_concrete_beats_any() {
    let types = TypeRegistry::new();
    assert!(KType::NUMBER.is_more_specific_than(KType::ANY, &types));
    assert!(!KType::ANY.is_more_specific_than(KType::NUMBER, &types));
}

/// Dispatch treats two structurally identical nominal declarations interchangeably — the
/// content-digest identity a `NEWTYPE` elaborated twice (an FN body called twice) yields. Two
/// independently sealed same-content newtype members intern to the same handle, so a value of one
/// is admitted where the other is declared.
#[test]
fn dispatch_unifies_structurally_identical_nominals() {
    let types = TypeRegistry::new();
    let slot = newtype_member("Wrapper", KType::NUMBER, &types);
    let carried = newtype_member("Wrapper", KType::NUMBER, &types);
    assert_eq!(
        slot, carried,
        "same content unifies regardless of allocation"
    );
    assert!(slot.satisfied_by(carried, &types));
    assert!(carried.satisfied_by(slot, &types));

    // A different declared name is genuinely different content, so it is not admitted.
    let other = newtype_member("Boxer", KType::NUMBER, &types);
    assert!(!slot.satisfied_by(other, &types));
}

#[test]
fn is_more_specific_list_number_beats_list_any() {
    let types = TypeRegistry::new();
    let n = types.list(KType::NUMBER);
    let a = types.list(KType::ANY);
    assert!(n.is_more_specific_than(a, &types));
    assert!(!a.is_more_specific_than(n, &types));
}

#[test]
fn is_more_specific_disjoint_lists_incomparable() {
    let types = TypeRegistry::new();
    let n = types.list(KType::NUMBER);
    let s = types.list(KType::STR);
    assert!(!n.is_more_specific_than(s, &types));
    assert!(!s.is_more_specific_than(n, &types));
}

#[test]
fn is_more_specific_dict_refines_value() {
    let types = TypeRegistry::new();
    let strict = types.dict(KType::STR, KType::NUMBER);
    let loose = types.dict(KType::STR, KType::ANY);
    assert!(strict.is_more_specific_than(loose, &types));
    assert!(!loose.is_more_specific_than(strict, &types));
}

/// Width-subset specificity: a nullary function `{}` is strictly more specific than a
/// unary `{x}` (its param key set is a subset, so it fills the wider slot under
/// call-by-name width drop), and the unary is not more specific than the nullary
/// (the unary declares a param the nullary lacks → contravariant width violation).
#[test]
fn is_more_specific_function_width_subset() {
    let types = TypeRegistry::new();
    let unary = types.function_type(
        Record::from_pairs(vec![("x".into(), KType::NUMBER)]),
        KType::NUMBER,
    );
    let nullary = types.function_type(Record::new(), KType::NUMBER);
    assert!(nullary.is_more_specific_than(unary, &types));
    assert!(!unary.is_more_specific_than(nullary, &types));
}

/// Depth-contravariant function specificity: `(x :Any) -> R ≺ (x :Number) -> R`. The
/// more-general param (`Any` ⊐ `Number`) makes the function more specific, because a
/// value accepting `Any` fills a slot that promised only `Number`.
#[test]
fn is_more_specific_function_param_contravariant() {
    let types = TypeRegistry::new();
    let any_param = types.function_type(
        Record::from_pairs(vec![("x".into(), KType::ANY)]),
        KType::STR,
    );
    let number_param = types.function_type(
        Record::from_pairs(vec![("x".into(), KType::NUMBER)]),
        KType::STR,
    );
    assert!(any_param.is_more_specific_than(number_param, &types));
    assert!(!number_param.is_more_specific_than(any_param, &types));
}

/// Return-covariant function specificity: `(x) -> Number ≺ (x) -> Any`. The narrower
/// return makes the function more specific.
#[test]
fn is_more_specific_function_return_covariant() {
    let types = TypeRegistry::new();
    let number_ret = types.function_type(
        Record::from_pairs(vec![("x".into(), KType::NUMBER)]),
        KType::NUMBER,
    );
    let any_ret = types.function_type(
        Record::from_pairs(vec![("x".into(), KType::NUMBER)]),
        KType::ANY,
    );
    assert!(number_ret.is_more_specific_than(any_ret, &types));
    assert!(!any_ret.is_more_specific_than(number_ret, &types));
}

fn record_ty(types: &TypeRegistry, fields: Vec<(&str, KType)>) -> KType {
    types.record(Record::from_pairs(
        fields.into_iter().map(|(n, t)| (n.to_string(), t)),
    ))
}

/// Record-value subtyping is the *dual* of function-param subtyping: a *wider* record is
/// strictly more specific (a `{x, y}` value fills an `{x}` slot, dropping `y`).
#[test]
fn record_width_superset_more_specific() {
    let types = TypeRegistry::new();
    let wide = record_ty(&types, vec![("x", KType::NUMBER), ("y", KType::STR)]);
    let narrow = record_ty(&types, vec![("x", KType::NUMBER)]);
    assert!(wide.is_more_specific_than(narrow, &types));
    assert!(!narrow.is_more_specific_than(wide, &types));
}

/// Covariant depth: `:{x :Number} ≺ :{x :Any}`.
#[test]
fn record_depth_covariant() {
    let types = TypeRegistry::new();
    let number = record_ty(&types, vec![("x", KType::NUMBER)]);
    let any = record_ty(&types, vec![("x", KType::ANY)]);
    assert!(number.is_more_specific_than(any, &types));
    assert!(!any.is_more_specific_than(number, &types));
}

/// Disjoint field sets are incomparable (`{x, y}` vs `{x, z}`) — dispatch ambiguity, not
/// an ordering.
#[test]
fn record_disjoint_fields_incomparable() {
    let types = TypeRegistry::new();
    let xy = record_ty(&types, vec![("x", KType::NUMBER), ("y", KType::STR)]);
    let xz = record_ty(&types, vec![("x", KType::NUMBER), ("z", KType::STR)]);
    assert!(!xy.is_more_specific_than(xz, &types));
    assert!(!xz.is_more_specific_than(xy, &types));
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
        (KType::NUMBER, Carried::Object(n)),
        (KType::STR, Carried::Object(s)),
        (KType::ANY, Carried::Object(n)),
    ] {
        // The delegation equivalence: classifying the spliced cell and opening the value directly agree.
        assert_eq!(
            ty.accepts_carried(carried, &types),
            ty.accepts_part(&spliced_part(carried), &types)
        );
    }
    // A numeric value is admitted by `:Number` / `:Any`, refused by `:Str`.
    assert!(KType::NUMBER.accepts_carried(Carried::Object(n), &types));
    assert!(KType::ANY.accepts_carried(Carried::Object(n), &types));
    assert!(!KType::STR.accepts_carried(Carried::Object(n), &types));
    // A type-channel value reaches the `OfKind` arm; a proper-type slot admits it.
    let kt_number = KType::NUMBER;
    assert!(KType::of_kind(KKind::ProperType).accepts_carried(Carried::Type(&kt_number), &types));
    // An object value reports a non-type `kind_of` and is refused by a type-channel slot.
    assert!(!KType::of_kind(KKind::ProperType).accepts_carried(Carried::Object(n), &types));
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
        (KType::NUMBER, true),
        (KType::ANY, true),
        (KType::STR, false),
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

    let narrow = record_ty(&types, vec![("x", KType::NUMBER)]);
    assert!(narrow.accepts_part(&spliced_part(Carried::Object(value)), &types));
    assert!(narrow.matches_value(value, &types));

    let mismatch = record_ty(&types, vec![("x", KType::STR)]);
    assert!(!mismatch.accepts_part(&spliced_part(Carried::Object(value)), &types));
    assert!(!mismatch.matches_value(value, &types));

    let extra = record_ty(&types, vec![("x", KType::NUMBER), ("q", KType::BOOL)]);
    assert!(!extra.accepts_part(&spliced_part(Carried::Object(value)), &types));
    assert!(!extra.matches_value(value, &types));

    // Unevaluated literal admits shape-only (defer-then-reevaluate on the typed value).
    assert!(mismatch.accepts_part(&ExpressionPart::RecordLiteral(vec![]), &types));
}

/// Admission table for `KType::accepts_part`: bare builtin type tokens, newtype / union
/// `Carried::Type` member identities, and a signature carrier all admit — a signature is a
/// type value, and `:Type` is the lattice top. The module value rejects (a module is a value,
/// reaching slots on the Object channel), a `:(OfKind Proper)` slot rejects the signature
/// (the proper tier is the non-signature tier), and non-type-denoting carriers reject.
#[test]
fn type_slot_admits_bare_builtin_tokens_and_user_type_carriers() {
    use crate::builtins::test_support::TestRun;
    use crate::machine::core::{run_root_storage, FrameStorageExt};
    use crate::machine::model::values::Module;
    let region = run_root_storage();
    let test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let types = test_run.types.clone();
    let t = KType::of_kind(KKind::AnyType);
    let kt_number: &KType = region.brand().alloc_ktype(KType::NUMBER);
    let kt_str: &KType = region.brand().alloc_ktype(KType::STR);
    let kt_bool: &KType = region.brand().alloc_ktype(KType::BOOL);
    let kt_null: &KType = region.brand().alloc_ktype(KType::NULL);
    assert!(t.accepts_part(&spliced_part(Carried::Type(kt_number)), &types));
    assert!(t.accepts_part(&spliced_part(Carried::Type(kt_str)), &types));
    assert!(t.accepts_part(&spliced_part(Carried::Type(kt_bool)), &types));
    assert!(t.accepts_part(&spliced_part(Carried::Type(kt_null)), &types));
    // NewType / union-variant type tokens flow as sealed member handles in the type channel — a
    // `:Type` slot admits them when the spliced cell opens to a `Carried::Type`.
    let newtype_token: &KType =
        region
            .brand()
            .alloc_ktype(newtype_member("Some", KType::NUMBER, &types));
    let struct_token: &KType = region
        .brand()
        .alloc_ktype(record_newtype_member("Point", &types));
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
    // A module value surfaces its principal signature, sealed once at construction — do the same
    // here so `ktype()` (reached by `alloc_object_checked`) has a filled cell.
    module.seal_self_sig(SigSchema::raw_self_sig(module), &types);
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
    let kt_sig: &KType = region
        .brand()
        .alloc_ktype(types.signature(SigSchema::project_decl(sig_scope, &types), Vec::new()));
    // A signature is a type value: the `:Type` lattice top admits it; the proper tier does not.
    assert!(t.accepts_part(&spliced_part(Carried::Type(kt_sig)), &types));
    assert!(!KType::of_kind(KKind::ProperType)
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
    assert!(KType::of_kind(KKind::Signature)
        .is_more_specific_than(KType::of_kind(KKind::AnyType), &types));
    assert!(!KType::of_kind(KKind::AnyType)
        .is_more_specific_than(KType::of_kind(KKind::Signature), &types));
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
    let newtype_ty = KType::of_kind(KKind::NewType);

    // The NewType *type value* — admitted in the type channel.
    let newtype_tv = newtype_member("Distance", KType::NUMBER, &types);
    assert!(newtype_ty.accepts_part(&spliced_part(Carried::Type(&newtype_tv)), &types));
    assert!(KType::of_kind(KKind::ProperType)
        .accepts_part(&spliced_part(Carried::Type(&newtype_tv)), &types));

    // A `TypeConstructor` type value is the wrong family — declined.
    let ctor_tv = RecursiveGroupWindow::seal_singleton(
        "Result".into(),
        RelativeSchema::TypeConstructor {
            schema: std::collections::HashMap::new(),
            param_names: Vec::new(),
        },
        None,
        &types,
    );
    assert!(!newtype_ty.accepts_part(&spliced_part(Carried::Type(&ctor_tv)), &types));

    // The runtime `Wrapped` *instance* is never matched by a kind slot.
    let inner: &KObject<'_> = region.alloc_object(KObject::Number(3.0));
    let w: &KObject<'_> = region
        .alloc_object_checked(
            KObject::Wrapped {
                inner: crate::machine::model::values::WrappedPayload::peel(inner),
                type_id: newtype_tv,
            },
            &types,
        )
        .expect("newtype_tv is a registry handle, borrowing no region");
    assert!(!newtype_ty.accepts_part(&spliced_part(Carried::Object(w)), &types));
    assert!(!newtype_ty.matches_value(w, &types));
}

/// Pins the kind refinement: a `NewType`-kind sealed member is strictly more specific than
/// `OfKind(NewType)`, and incomparable with `OfKind(TypeConstructor)` (a sibling family).
#[test]
fn user_type_newtype_specificity_lattice() {
    let types = TypeRegistry::new();
    let newtype_kind = KType::of_kind(KKind::NewType);
    let ctor_kind = KType::of_kind(KKind::TypeConstructor);
    let dist = newtype_member("Distance", KType::NUMBER, &types);
    assert!(dist.is_more_specific_than(newtype_kind, &types));
    assert!(!newtype_kind.is_more_specific_than(dist, &types));
    assert!(!dist.is_more_specific_than(ctor_kind, &types));
    assert!(!ctor_kind.is_more_specific_than(dist, &types));
}

/// Specificity ordering for a sealed member against the `OfKind` kind lattice:
/// - a nominal kind is strictly under `Any` and strictly under `OfKind(Proper)`;
/// - a member of kind `K` is strictly under `OfKind(K)`;
/// - a member of one kind and `OfKind` of a different kind are incomparable.
#[test]
fn user_type_specificity_lattice() {
    let types = TypeRegistry::new();
    let newtype_kind = KType::of_kind(KKind::NewType);
    let ctor_kind = KType::of_kind(KKind::TypeConstructor);
    let point = record_newtype_member("Point", &types);
    // A nominal kind strictly under `Any` and under `OfKind(Proper)`.
    assert!(newtype_kind.is_more_specific_than(KType::ANY, &types));
    assert!(!KType::ANY.is_more_specific_than(newtype_kind, &types));
    assert!(newtype_kind.is_more_specific_than(KType::of_kind(KKind::ProperType), &types));
    assert!(!KType::of_kind(KKind::ProperType).is_more_specific_than(newtype_kind, &types));
    // A `NewType`-kind member strictly under `OfKind(NewType)`.
    assert!(point.is_more_specific_than(newtype_kind, &types));
    assert!(!newtype_kind.is_more_specific_than(point, &types));
    // Different-kind pairs incomparable.
    assert!(!point.is_more_specific_than(ctor_kind, &types));
    assert!(!ctor_kind.is_more_specific_than(point, &types));
}

/// `Signature` `pinned_slots` specificity rules (constraint role):
/// - A non-empty `pinned_slots` strictly refines an empty same-schema form when every pin in the
///   empty side appears (with equal `KType`) in the non-empty side.
/// - Different interfaces compare by structural `sig_subtype`: two genuinely distinct interfaces
///   (disjoint value slots) are mutually unsatisfying, hence incomparable (neither strictly
///   refines).
/// - Same schema with disjoint constraint keys is incomparable.
/// - Same-key-different-`KType` is incomparable.
///
/// Ruling 12 folds the SIG identity into content: two structurally identical signatures are ONE
/// type, so the "different interface" case is exercised with genuinely different content (disjoint
/// value slots), not two distinct declaration scopes over one shape.
#[test]
fn is_more_specific_for_pinned_signature_bound() {
    let types = TypeRegistry::new();
    let ordered_schema = SigSchema {
        sig_id: Some(crate::machine::core::ScopeId::SENTINEL),
        abstract_members: std::collections::HashMap::new(),
        manifest_members: std::collections::HashMap::new(),
        value_slots: [("a".to_string(), KType::NUMBER)].into_iter().collect(),
    };
    let hashed_schema = SigSchema {
        sig_id: Some(crate::machine::core::ScopeId::SENTINEL),
        abstract_members: std::collections::HashMap::new(),
        manifest_members: std::collections::HashMap::new(),
        value_slots: [("b".to_string(), KType::NUMBER)].into_iter().collect(),
    };

    let bare = types.signature(ordered_schema.clone(), Vec::new());
    let pinned_number =
        types.signature(ordered_schema.clone(), vec![("Type".into(), KType::NUMBER)]);
    let pinned_str = types.signature(ordered_schema.clone(), vec![("Type".into(), KType::STR)]);
    let pinned_two = types.signature(
        ordered_schema.clone(),
        vec![("Type".into(), KType::NUMBER), ("Elt".into(), KType::STR)],
    );
    let other_sig = types.signature(hashed_schema, vec![("Type".into(), KType::NUMBER)]);
    let pinned_elt = types.signature(ordered_schema, vec![("Elt".into(), KType::NUMBER)]);

    assert!(pinned_number.is_more_specific_than(bare, &types));
    assert!(!bare.is_more_specific_than(pinned_number, &types));
    assert!(pinned_two.is_more_specific_than(pinned_number, &types));
    assert!(!pinned_number.is_more_specific_than(pinned_two, &types));
    assert!(!pinned_number.is_more_specific_than(pinned_str, &types));
    assert!(!pinned_str.is_more_specific_than(pinned_number, &types));
    assert!(!pinned_number.is_more_specific_than(pinned_elt, &types));
    assert!(!pinned_elt.is_more_specific_than(pinned_number, &types));
    assert!(!pinned_number.is_more_specific_than(other_sig, &types));
    assert!(!other_sig.is_more_specific_than(pinned_number, &types));
}

/// A shared `Result` `TypeConstructor` member handle. Identity is content, so a `ConstructorApply`
/// slot and a `Tagged` carrier match only when they name the *same* member — every test below
/// threads this one member through both the slot ctor and the value.
fn result_member(types: &TypeRegistry) -> KType {
    RecursiveGroupWindow::seal_singleton(
        "Result".into(),
        RelativeSchema::TypeConstructor {
            schema: std::collections::HashMap::new(),
            param_names: vec!["Ok".into(), "Error".into()],
        },
        None,
        types,
    )
}

/// The args record for a `Result` application, keyed by the carrier's parameter names.
fn result_args(ok: KType, error: KType) -> Record<KType> {
    Record::from_pairs([("Ok".to_string(), ok), ("Error".to_string(), error)])
}

/// Build a `Result`-carrier `Tagged` value occupying `tag` with `payload`, identified by the
/// erased `Result` member handle (no stamped type arguments). The inner `payload` is itself a
/// `Tagged` carrier identified by the error type's nominal member handle.
fn result_value<'a>(member: KType, tag: &str, payload: KObject<'a>) -> KObject<'a> {
    KObject::Tagged {
        tag: tag.into(),
        value: std::rc::Rc::new(payload),
        identity: member,
    }
}

/// A bare error carrier (`Tagged` identified by `member`) standing in for a caught error value.
fn error_carrier<'a>(member: KType) -> KObject<'a> {
    KObject::Tagged {
        tag: "_".into(),
        value: std::rc::Rc::new(KObject::Number(0.0)),
        identity: member,
    }
}

/// A singleton `TypeConstructor`-kind member named `name`, for an error-type identity.
fn error_type_member(name: &str, types: &TypeRegistry) -> KType {
    RecursiveGroupWindow::seal_singleton(
        name.into(),
        RelativeSchema::TypeConstructor {
            schema: std::collections::HashMap::new(),
            param_names: Vec::new(),
        },
        None,
        types,
    )
}

/// `:(Result {Ok = …, Error = …})` slot admission: a `ConstructorApply` slot whose ctor
/// identity matches the `Result` carrier admits an `Error(...)` value iff the inhabited
/// `Error` payload satisfies the slot's same-named arg. A caught `Error(KError)` is rejected
/// where that arg is `MyError` and accepted where it is `KError` / `Any`. Identity is
/// content, so the slot's arg and the value's payload carrier share one member per error type.
#[test]
fn constructor_apply_result_checks_inhabited_error_param() {
    let types = TypeRegistry::new();

    let r_member = result_member(&types);
    let kerror_ty = error_type_member("KError", &types);
    let my_error_ty = error_type_member("MyError", &types);

    let slot_my_error = types.constructor_apply(r_member, result_args(KType::ANY, my_error_ty));
    let caught = result_value(r_member, "Error", error_carrier(kerror_ty));
    assert!(!slot_my_error.matches_value(&caught, &types));

    let slot_kerror = types.constructor_apply(r_member, result_args(KType::ANY, kerror_ty));
    assert!(slot_kerror.matches_value(&caught, &types));

    let my_error = result_value(r_member, "Error", error_carrier(my_error_ty));
    assert!(slot_my_error.matches_value(&my_error, &types));
}

/// The `Ok` tag names the `Ok` parameter, so a slot checks the `Ok` payload against its
/// `Ok` arg regardless of the `Error` arg: an `Ok(42)` value admits any `Error` arg (the
/// uninhabited tag's parameter is unconstrained at the value).
#[test]
fn constructor_apply_result_ok_admits_any_error_param() {
    let types = TypeRegistry::new();
    let r_member = result_member(&types);
    let my_error_ty = error_type_member("MyError", &types);
    let ok_value = result_value(r_member, "Ok", KObject::Number(42.0));
    let slot = types.constructor_apply(r_member, result_args(KType::NUMBER, my_error_ty));
    assert!(slot.matches_value(&ok_value, &types));
    let slot_str = types.constructor_apply(r_member, result_args(KType::STR, KType::ANY));
    assert!(!slot_str.matches_value(&ok_value, &types));
}

/// Covariance for `ConstructorApply` carriers: a value stamped
/// `{Ok = Number, Error = MyError}` is admitted by the coarser `{Ok = Any, Error = Any}`
/// slot, and the refined slot is strictly more specific, so dispatch tie-breaks toward the
/// refined overload.
#[test]
fn constructor_apply_covariant_admission_and_specificity() {
    let types = TypeRegistry::new();
    let r_member = result_member(&types);
    let my_error = error_type_member("MyError", &types);
    let stamped = KObject::Tagged {
        tag: "Ok".into(),
        value: std::rc::Rc::new(KObject::Number(1.0)),
        identity: types.constructor_apply(r_member, result_args(KType::NUMBER, my_error)),
    };
    let coarse = types.constructor_apply(r_member, result_args(KType::ANY, KType::ANY));
    let refined = types.constructor_apply(r_member, result_args(KType::NUMBER, my_error));
    assert!(coarse.matches_value(&stamped, &types));
    assert!(refined.matches_value(&stamped, &types));
    assert!(refined.is_more_specific_than(coarse, &types));
    assert!(!coarse.is_more_specific_than(refined, &types));
}

/// A stamped `ConstructorApply` identity (from ascription) is checked structurally against
/// the slot args, taking precedence over the inhabited-tag path.
#[test]
fn constructor_apply_stamped_type_args_checked_structurally() {
    let types = TypeRegistry::new();
    let r_member = result_member(&types);
    let stamped = KObject::Tagged {
        tag: "Ok".into(),
        value: std::rc::Rc::new(KObject::Number(1.0)),
        identity: types.constructor_apply(r_member, result_args(KType::NUMBER, KType::STR)),
    };
    let slot_ok = types.constructor_apply(r_member, result_args(KType::NUMBER, KType::STR));
    assert!(slot_ok.matches_value(&stamped, &types));
    let slot_any = types.constructor_apply(r_member, result_args(KType::ANY, KType::ANY));
    assert!(slot_any.matches_value(&stamped, &types));
    let slot_bad = types.constructor_apply(r_member, result_args(KType::BOOL, KType::STR));
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
    let deferred_ret = types.intern(TypeNode::DeferredReturn(DeferredReturnSurface::Type(
        TypeIdentifier::leaf("er".into()),
    )));
    let deferred = types.function_type(Record::new(), deferred_ret);
    let any = types.function_type(Record::new(), KType::ANY);
    assert!(deferred.is_more_specific_than(any, &types));
    assert!(!any.is_more_specific_than(deferred, &types));
}

/// Two function types differing only in their deferred-return shadow are distinct: not equal,
/// neither more specific than the other, and they hash apart.
#[test]
fn two_functions_differ_only_in_deferred_return_are_distinct() {
    let types = TypeRegistry::new();
    use std::hash::{Hash, Hasher};
    let er_ret = types.intern(TypeNode::DeferredReturn(DeferredReturnSurface::Type(
        TypeIdentifier::leaf("er".into()),
    )));
    let ar_ret = types.intern(TypeNode::DeferredReturn(DeferredReturnSurface::Type(
        TypeIdentifier::leaf("Ar".into()),
    )));
    let er = types.function_type(Record::new(), er_ret);
    let ar = types.function_type(Record::new(), ar_ret);
    assert_ne!(er, ar);
    assert!(!er.is_more_specific_than(ar, &types));
    assert!(!ar.is_more_specific_than(er, &types));
    // A `KType` is a `Copy` `u128` handle whose hash is its content digest, so the two function
    // types — differing only in their deferred-return shadow — hash apart.
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
    let slot_er = types.intern(TypeNode::DeferredReturn(DeferredReturnSurface::Type(
        TypeIdentifier::leaf("er".into()),
    )));
    assert!(function_compat(&candidate, &no_params, slot_er, &types));

    // Differing shadow → reject.
    let slot_ar = types.intern(TypeNode::DeferredReturn(DeferredReturnSurface::Type(
        TypeIdentifier::leaf("Ar".into()),
    )));
    assert!(!function_compat(&candidate, &no_params, slot_ar, &types));

    // Resolved slot → reject (opaque until elaboration).
    assert!(!function_compat(
        &candidate,
        &no_params,
        KType::NUMBER,
        &types
    ));

    // `Any` slot → admit.
    assert!(function_compat(&candidate, &no_params, KType::ANY, &types));
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

    let number_or_str = types.union_of(vec![KType::NUMBER, KType::STR]);
    let str_or_bool = types.union_of(vec![KType::STR, KType::BOOL]);

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
        types.list(KType::NUMBER),
    ));

    let with_list = types.union_of(vec![types.list(KType::NUMBER), KType::STR]);
    let without_list = types.union_of(vec![KType::NUMBER, KType::STR]);

    assert!(with_list.accepts_carried(Carried::Object(list_value), &types));
    assert!(!without_list.accepts_carried(Carried::Object(list_value), &types));
}

/// Specificity: each member refines its union (AC3); a union refines `Any` and a superset
/// union; a union is not more specific than a bare member nor than an equal union.
#[test]
fn union_specificity_ordering() {
    let types = TypeRegistry::new();
    let number = KType::NUMBER;
    let number_or_str = types.union_of(vec![KType::NUMBER, KType::STR]);
    let number_or_str_or_bool = types.union_of(vec![KType::NUMBER, KType::STR, KType::BOOL]);

    // Each member is a subtype of the union.
    assert!(number.is_more_specific_than(number_or_str, &types));
    // A union refines `Any`.
    assert!(number_or_str.is_more_specific_than(KType::ANY, &types));
    // A union is not more specific than one of its members.
    assert!(!number_or_str.is_more_specific_than(number, &types));
    // A subset union refines a superset union; the reverse does not hold.
    assert!(number_or_str.is_more_specific_than(number_or_str_or_bool, &types));
    assert!(!number_or_str_or_bool.is_more_specific_than(number_or_str, &types));
    // Equal unions (order-blind) are not strictly more specific than each other.
    let str_or_number = types.union_of(vec![KType::STR, KType::NUMBER]);
    assert!(!number_or_str.is_more_specific_than(str_or_number, &types));
}

/// A module value's `ktype()` reports its principal signature, and its identity is its self-sig
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
    let types = test_run.types.clone();

    let child = region
        .brand()
        .alloc_scope(Scope::child_under_module(scope, "Mod".into()));
    let m: &Module = region
        .brand()
        .alloc_module(Module::new("Mod".into(), child));
    m.type_members
        .borrow_mut()
        .insert("Elt".into(), KType::NUMBER);
    m.seal_self_sig(SigSchema::raw_self_sig(m), &types);
    let kt = KObject::Module(m).ktype();
    // Ruling 12: the `Signature` node carries no `sig_id`, so a module's principal type is a
    // Signature with no `WITH` pins — identity is its self-sig *content*, checked below.
    assert!(matches!(
        types.node(kt),
        TypeNode::Signature { pinned_slots, .. } if pinned_slots.is_empty()
    ));
    // Identity is content: the module's type equals its own re-derived self-sig.
    assert_eq!(kt, types.signature(SigSchema::raw_self_sig(m), Vec::new()));

    // A second module with the identical interface shares the type — content, not mint.
    let child2 = region
        .brand()
        .alloc_scope(Scope::child_under_module(scope, "Mod2".into()));
    let m2: &Module = region
        .brand()
        .alloc_module(Module::new("Mod2".into(), child2));
    m2.type_members
        .borrow_mut()
        .insert("Elt".into(), KType::NUMBER);
    m2.seal_self_sig(SigSchema::raw_self_sig(m2), &types);
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
        .insert("Elt".into(), KType::STR);
    m3.seal_self_sig(SigSchema::raw_self_sig(m3), &types);
    assert_ne!(kt, KObject::Module(m3).ktype());
}

/// `matches_value` admits a module *object* into a `Signature` slot: a declared slot by
/// structural satisfaction (+ pin agreement), the empty signature for any module and no non-module
/// value.
#[test]
fn matches_value_admits_module_object_via_signature_slot() {
    use crate::builtins::test_support::TestRun;
    use crate::machine::core::{run_root_storage, FrameStorageExt};
    use crate::machine::model::values::Module;
    use crate::machine::model::KObject;
    use crate::machine::Scope;
    let region = run_root_storage();
    let test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let types = test_run.types.clone();

    // An empty signature (empty decl scope): every module bare-satisfies it, so the pins gate.
    let sig_scope = region
        .brand()
        .alloc_scope(Scope::child_under_sig(scope, "S".into()));
    let schema = SigSchema::project_decl(sig_scope, &types);

    let child = region
        .brand()
        .alloc_scope(Scope::child_under_module(scope, "M".into()));
    let m: &Module = region.brand().alloc_module(Module::new("M".into(), child));
    m.type_members
        .borrow_mut()
        .insert("Type".into(), KType::NUMBER);
    m.seal_self_sig(SigSchema::raw_self_sig(m), &types);

    let declared = types.signature(schema.clone(), Vec::new());
    assert!(declared.matches_value(&KObject::Module(m), &types));

    let pinned_ok = types.signature(schema.clone(), vec![("Type".into(), KType::NUMBER)]);
    let pinned_bad = types.signature(schema, vec![("Type".into(), KType::STR)]);
    assert!(pinned_ok.matches_value(&KObject::Module(m), &types));
    assert!(!pinned_bad.matches_value(&KObject::Module(m), &types));

    let empty = KType::EMPTY_SIGNATURE;
    assert!(empty.matches_value(&KObject::Module(m), &types));
    assert!(!empty.matches_value(&KObject::Number(1.0), &types));
}

/// Specificity over the module lattice: a module's self-sig refines a declared
/// signature it satisfies, and any non-empty signature refines the empty top. The signature
/// and module carry real members: under content identity a member-less signature *is* the
/// `:Module` top ([`EMPTY_SIGNATURE`](KType::EMPTY_SIGNATURE)), so degenerate empty points would
/// collapse into one type and there would be no ordering to test.
#[test]
fn specificity_self_sig_refines_declared_and_empty() {
    use crate::builtins::test_support::{lookup_module, TestRun};
    use crate::machine::model::KObject;
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
    let declared = *scope
        .resolve_type("Ordered")
        .expect("Ordered must bind a Signature KType");
    let m = lookup_module(scope, "int_ord", &types);

    let self_of = KObject::Module(m).ktype();
    let empty = KType::EMPTY_SIGNATURE;

    // `self_of ≺ declared` because `m`'s self-sig satisfies `Ordered`.
    assert!(self_of.is_more_specific_than(declared, &types));
    // Any non-empty signature `≺ Empty`; `Empty` refines nothing narrower.
    assert!(declared.is_more_specific_than(empty, &types));
    assert!(self_of.is_more_specific_than(empty, &types));
    assert!(!empty.is_more_specific_than(declared, &types));
    // `satisfied_by` routes a memoized self-sig element type through the `self-sig ≺ Declared` arm.
    assert!(declared.satisfied_by(self_of, &types));
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
        self_of.is_more_specific_than(*declared, &types),
        "the manifest self-sig strictly refines the abstract sig it satisfies",
    );
    assert!(
        !declared.is_more_specific_than(self_of, &types),
        "the abstract sig must not refine the manifest self-sig back — the pair is ordered, \
         not mutually satisfying",
    );
}

// --- verdict-registry wiring (`is_more_specific_than` routes every pair through the run's
// `TypeRegistry`) ------------------------------------------------------------------------------

/// A repeat check of the same composite pair (`List<Number>` vs `List<Any>`, verdict `true`)
/// is a counter-verified registry hit on the second call, and the verdict is identical both times.
#[test]
fn verdict_repeat_composite_hit() {
    let types = TypeRegistry::new();
    let n = types.list(KType::NUMBER);
    let a = types.list(KType::ANY);

    let first = n.is_more_specific_than(a, &types);
    assert!(first);
    // Memoized unconditionally now: the outer `List` pair misses, and the walk's inner
    // `Number` vs `Any` leaf pair misses too (the old registry-probe gate is gone).
    assert_eq!(types.miss_count(), 2);
    assert_eq!(types.hit_count(), 0);

    let second = n.is_more_specific_than(a, &types);
    assert_eq!(second, first);
    assert_eq!(types.hit_count(), 1, "second call must be a registry hit");
}

/// A negative verdict is recorded too: the second call to a pair the walk resolves `false` for
/// is a hit returning `false`.
#[test]
fn verdict_negative_also_recorded() {
    let types = TypeRegistry::new();
    let a = types.list(KType::ANY);
    let n = types.list(KType::NUMBER);

    let first = a.is_more_specific_than(n, &types);
    assert!(!first);
    // Outer `List<Any>` vs `List<Number>` misses, and the inner `Any` vs `Number` leaf misses.
    assert_eq!(types.miss_count(), 2);

    let second = a.is_more_specific_than(n, &types);
    assert!(!second);
    assert_eq!(types.hit_count(), 1, "second call must be a registry hit");
}

/// Every specificity query is memoized unconditionally — no representation-probe gates the cache —
/// so even a leaf pair records a verdict on the first call and hits on the repeat.
#[test]
fn verdict_leaf_pairs_memoized_unconditionally() {
    let types = TypeRegistry::new();
    assert!(KType::NUMBER.is_more_specific_than(KType::ANY, &types));
    assert_eq!(types.miss_count(), 1, "the first leaf query is a miss");
    assert_eq!(types.hit_count(), 0);

    assert!(KType::NUMBER.is_more_specific_than(KType::ANY, &types));
    assert_eq!(types.hit_count(), 1, "the repeat is a registry hit");
}

/// Purity sanity: a cold registry computes the same composite verdict a warm one does — the
/// verdict cache is an accelerator, never load-bearing. The content must be present in each
/// registry (content lives in the registry now), so the cold registry re-interns it before the
/// query.
#[test]
fn verdict_purity_across_a_cold_registry() {
    let warm = TypeRegistry::new();
    let n = warm.list(KType::NUMBER);
    let a = warm.list(KType::ANY);
    let before = n.is_more_specific_than(a, &warm);

    let cold = TypeRegistry::new();
    let n_cold = cold.list(KType::NUMBER);
    let a_cold = cold.list(KType::ANY);
    let after = n_cold.is_more_specific_than(a_cold, &cold);
    assert_eq!(before, after);
}
