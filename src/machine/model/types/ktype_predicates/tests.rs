use super::*;
use crate::builtins::test_support::lookup_type;
use crate::builtins::test_support::{spliced_part, type_name, type_token, value_name};
use crate::machine::core::SubstrateDoor;
use crate::machine::model::BinderSymbol;
use crate::machine::model::Carried;
use crate::machine::model::ModuleDraft;
use crate::machine::model::Record;
use crate::machine::model::Scalar;
use crate::machine::model::TypeMemberMap;
use crate::machine::model::ast::{ExpressionPart, WorkingPart};
use crate::machine::model::types::{RecursiveGroupWindow, RelativeSchema};

/// Mint the zero-dep fold door a `Tagged`/`Wrapped` test value needs, over a fresh root region, as
/// two `let` bindings in the caller's own scope (mirrors the `kobject` test macro). `forge_for_test`
/// is the sanctioned test-only placement mint; a statement macro (not a returning fn) keeps `door`'s
/// borrow of `storage` in the same frame.
macro_rules! container_door {
    ($storage:ident, $door:ident) => {
        use crate::machine::core::{FoldingBrand, FrameStorageExt, run_root_storage};
        use crate::witnessed::FoldedPlacement;
        let $storage = run_root_storage();
        let owned_cells = crate::machine::core::FrameCoverage::empty();
        let $door = FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(
            $storage.brand().handle(),
        ))
        .with_holder(&owned_cells);
    };
}

/// A singleton newtype member handle for a record-repr newtype (an ex-struct) named `name`
/// (empty record repr is fine — the predicates key on the sealed member's `(component digest,
/// index)` + `kind`, never a schema descent).
fn record_newtype_member(name: &str, types: &TypeRegistry) -> KType {
    let repr = types.record(Record::new());
    RecursiveGroupWindow::seal_singleton(
        type_token(name),
        RelativeSchema::NewType(repr),
        None,
        types,
    )
}

/// A singleton newtype member handle named `name` over `repr`.
fn newtype_member(name: &str, repr: KType, types: &TypeRegistry) -> KType {
    RecursiveGroupWindow::seal_singleton(
        type_token(name),
        RelativeSchema::NewType(repr),
        None,
        types,
    )
}

#[test]
fn is_more_specific_concrete_beats_any() {
    let registries = RunRegistries::new();
    assert!(KType::NUMBER.is_more_specific_than(KType::ANY, &registries));
    assert!(!KType::ANY.is_more_specific_than(KType::NUMBER, &registries));
}

/// Dispatch treats two structurally identical nominal declarations interchangeably — the
/// content-digest identity a `NEWTYPE` elaborated twice (an FN body called twice) yields. Two
/// independently sealed same-content newtype members intern to the same handle, so a value of one
/// is admitted where the other is declared.
#[test]
fn dispatch_unifies_structurally_identical_nominals() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let slot = newtype_member("Wrapper", KType::NUMBER, types);
    let carried = newtype_member("Wrapper", KType::NUMBER, types);
    assert_eq!(
        slot, carried,
        "same content unifies regardless of allocation"
    );
    assert!(slot.satisfied_by(carried, &registries));
    assert!(carried.satisfied_by(slot, &registries));

    // A different declared name is genuinely different content, so it is not admitted.
    let other = newtype_member("Boxer", KType::NUMBER, types);
    assert!(!slot.satisfied_by(other, &registries));
}

#[test]
fn is_more_specific_list_number_beats_list_any() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let n = types.list(KType::NUMBER);
    let a = types.list(KType::ANY);
    assert!(n.is_more_specific_than(a, &registries));
    assert!(!a.is_more_specific_than(n, &registries));
}

#[test]
fn is_more_specific_disjoint_lists_incomparable() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let n = types.list(KType::NUMBER);
    let s = types.list(KType::STR);
    assert!(!n.is_more_specific_than(s, &registries));
    assert!(!s.is_more_specific_than(n, &registries));
}

#[test]
fn is_more_specific_dict_refines_value() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let strict = types.dict(KType::STR, KType::NUMBER);
    let loose = types.dict(KType::STR, KType::ANY);
    assert!(strict.is_more_specific_than(loose, &registries));
    assert!(!loose.is_more_specific_than(strict, &registries));
}

/// Width-subset specificity: a nullary function `{}` is strictly more specific than a
/// unary `{x}` (its param key set is a subset, so it fills the wider slot under
/// call-by-name width drop), and the unary is not more specific than the nullary
/// (the unary declares a param the nullary lacks → contravariant width violation).
#[test]
fn is_more_specific_function_width_subset() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let unary = types.function_type(
        Record::from_pairs(vec![(
            crate::builtins::test_support::binder_token("x"),
            KType::NUMBER,
        )]),
        KType::NUMBER,
    );
    let nullary = types.function_type(Record::new(), KType::NUMBER);
    assert!(nullary.is_more_specific_than(unary, &registries));
    assert!(!unary.is_more_specific_than(nullary, &registries));
}

/// Depth-contravariant function specificity: `(x :Any) -> R ≺ (x :Number) -> R`. The
/// more-general param (`Any` ⊐ `Number`) makes the function more specific, because a
/// value accepting `Any` fills a slot that promised only `Number`.
#[test]
fn is_more_specific_function_param_contravariant() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let any_param = types.function_type(
        Record::from_pairs(vec![(
            crate::builtins::test_support::binder_token("x"),
            KType::ANY,
        )]),
        KType::STR,
    );
    let number_param = types.function_type(
        Record::from_pairs(vec![(
            crate::builtins::test_support::binder_token("x"),
            KType::NUMBER,
        )]),
        KType::STR,
    );
    assert!(any_param.is_more_specific_than(number_param, &registries));
    assert!(!number_param.is_more_specific_than(any_param, &registries));
}

/// Return-covariant function specificity: `(x) -> Number ≺ (x) -> Any`. The narrower
/// return makes the function more specific.
#[test]
fn is_more_specific_function_return_covariant() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let number_ret = types.function_type(
        Record::from_pairs(vec![(
            crate::builtins::test_support::binder_token("x"),
            KType::NUMBER,
        )]),
        KType::NUMBER,
    );
    let any_ret = types.function_type(
        Record::from_pairs(vec![(
            crate::builtins::test_support::binder_token("x"),
            KType::NUMBER,
        )]),
        KType::ANY,
    );
    assert!(number_ret.is_more_specific_than(any_ret, &registries));
    assert!(!any_ret.is_more_specific_than(number_ret, &registries));
}

fn record_ty(types: &TypeRegistry, fields: Vec<(&str, KType)>) -> KType {
    types.record(Record::from_pairs(
        fields
            .into_iter()
            .map(|(n, t)| (crate::builtins::test_support::binder_token(n), t)),
    ))
}

/// Record-value subtyping is the *dual* of function-param subtyping: a *wider* record is
/// strictly more specific (a `{x, y}` value fills an `{x}` slot, dropping `y`).
#[test]
fn record_width_superset_more_specific() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let wide = record_ty(types, vec![("x", KType::NUMBER), ("y", KType::STR)]);
    let narrow = record_ty(types, vec![("x", KType::NUMBER)]);
    assert!(wide.is_more_specific_than(narrow, &registries));
    assert!(!narrow.is_more_specific_than(wide, &registries));
}

/// Covariant depth: `:{x :Number} ≺ :{x :Any}`.
#[test]
fn record_depth_covariant() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let number = record_ty(types, vec![("x", KType::NUMBER)]);
    let any = record_ty(types, vec![("x", KType::ANY)]);
    assert!(number.is_more_specific_than(any, &registries));
    assert!(!any.is_more_specific_than(number, &registries));
}

/// Disjoint field sets are incomparable (`{x, y}` vs `{x, z}`) — dispatch ambiguity, not
/// an ordering.
#[test]
fn record_disjoint_fields_incomparable() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let xy = record_ty(types, vec![("x", KType::NUMBER), ("y", KType::STR)]);
    let xz = record_ty(types, vec![("x", KType::NUMBER), ("z", KType::STR)]);
    assert!(!xy.is_more_specific_than(xz, &registries));
    assert!(!xz.is_more_specific_than(xy, &registries));
}

/// `accepts_carried` is the classifier `accepts_working_part`'s `Spliced` arm
/// delegates to: a resolved value classifies identically whether reached as a spliced part or opened
/// directly. Also pins the value-shaped arms (object type-tag, type-channel `OfKind`) it owns.
#[test]
fn accepts_carried_matches_spliced_delegation() {
    let registries = RunRegistries::new();
    use crate::machine::core::{FrameStorageExt, run_root_storage};
    let storage = run_root_storage();
    let region = storage.brand();
    let n: &KObject<'_> = region.alloc_scalar(Scalar::Number(7.0));
    let s: &KObject<'_> = region.alloc_string("hi");

    for (ty, carried) in [
        (KType::NUMBER, Carried::Object(n)),
        (KType::STR, Carried::Object(s)),
        (KType::ANY, Carried::Object(n)),
    ] {
        // The delegation equivalence: classifying the spliced cell and opening the value directly agree.
        assert_eq!(
            ty.accepts_carried(carried, &registries),
            ty.accepts_working_part(&spliced_part(&storage, carried), &registries)
        );
    }
    // A numeric value is admitted by `:Number` / `:Any`, refused by `:Str`.
    assert!(KType::NUMBER.accepts_carried(Carried::Object(n), &registries));
    assert!(KType::ANY.accepts_carried(Carried::Object(n), &registries));
    assert!(!KType::STR.accepts_carried(Carried::Object(n), &registries));
    // A type-channel value reaches the `OfKind` arm; a proper-type slot admits it.
    let kt_number = KType::NUMBER;
    assert!(
        KType::of_kind(KKind::ProperType).accepts_carried(Carried::Type(kt_number), &registries)
    );
    // An object value reports a non-type `kind_of` and is refused by a type-channel slot.
    assert!(!KType::of_kind(KKind::ProperType).accepts_carried(Carried::Object(n), &registries));
}

/// A spliced **cell** classifies through `accepts_working_part` by opening at its own brand and
/// handing the value to `accepts_carried` (no re-anchoring): a `7.0` value is admitted
/// by `:Number` / `:Any` and refused by `:Str`, matching a direct `accepts_carried`. Built through
/// the scope's own carrier surface (`resident_value_carrier` + `Sealed::seal`) — the exact
/// construction a real splice rests on the working expression.
#[test]
fn spliced_cell_classifies_by_opening() {
    let registries = RunRegistries::new();
    use crate::builtins::test_support::run_root_bare;
    use crate::machine::core::run_root_storage;
    use crate::machine::model::values::Carried;
    use crate::machine::model::values::KObject;

    let storage = run_root_storage();
    let scope = run_root_bare(&storage);
    let obj: &KObject = scope.brand().alloc_scalar(Scalar::Number(7.0));
    let cell_part = WorkingPart::Spliced {
        from_name: None,
        cell: scope.seal_resident(Carried::Object(obj)),
    };

    for (ty, admits) in [
        (KType::NUMBER, true),
        (KType::ANY, true),
        (KType::STR, false),
    ] {
        assert_eq!(
            ty.accepts_working_part(&cell_part, &registries),
            admits,
            "cell classification for {ty:?}",
        );
        // Agrees with opening the value directly.
        assert_eq!(
            ty.accepts_working_part(&cell_part, &registries),
            ty.accepts_carried(Carried::Object(obj), &registries)
        );
    }
}

/// A `{x = 1, y = "a"}` value (carried type `:{x :Number, y :Str}`) admits and matches a
/// narrower `:{x :Number}` slot (width drop); rejects a field-type mismatch (`:{x :Str}`)
/// and a slot demanding a field the value lacks (`:{x :Number, q :Bool}`). A bare record
/// literal admits any record slot shape-only.
#[test]
fn record_value_admission_and_matches() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    use crate::machine::core::{FoldingBrand, FrameStorageExt, run_root_storage};
    use crate::witnessed::FoldedPlacement;
    let storage = run_root_storage();
    let region = storage.brand();
    let owned_cells = crate::machine::core::FrameCoverage::empty();
    let door = FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(region.handle()))
        .with_holder(&owned_cells);
    let value: &KObject<'_> = door.alloc_object_folded(KObject::record(
        door,
        &[
            (
                crate::builtins::test_support::binder_token("x"),
                KObject::Number(1.0),
            ),
            (
                crate::builtins::test_support::binder_token("y"),
                KObject::KString("a"),
            ),
        ],
        types,
    ));

    let narrow = record_ty(types, vec![("x", KType::NUMBER)]);
    assert!(
        narrow.accepts_working_part(&spliced_part(&storage, Carried::Object(value)), &registries)
    );
    assert!(narrow.matches_value(value, &registries));

    let mismatch = record_ty(types, vec![("x", KType::STR)]);
    assert!(
        !mismatch
            .accepts_working_part(&spliced_part(&storage, Carried::Object(value)), &registries)
    );
    assert!(!mismatch.matches_value(value, &registries));

    let extra = record_ty(types, vec![("x", KType::NUMBER), ("q", KType::BOOL)]);
    assert!(
        !extra.accepts_working_part(&spliced_part(&storage, Carried::Object(value)), &registries)
    );
    assert!(!extra.matches_value(value, &registries));

    // Unevaluated literal admits shape-only (defer-then-reevaluate on the typed value).
    assert!(mismatch.accepts_part(&ExpressionPart::RecordLiteral(&[]), types));
}

/// Admission table for `KType::accepts_part`: bare builtin type tokens, newtype / union
/// `Carried::Type` member identities, and a signature carrier all admit — a signature is a
/// type value, and `:Type` is the lattice top. The module value rejects (a module is a value,
/// reaching slots on the Object channel), a `:(OfKind Proper)` slot rejects the signature
/// (the proper tier is the non-signature tier), and non-type-denoting carriers reject.
#[test]
fn type_slot_admits_bare_builtin_tokens_and_user_type_carriers() {
    use crate::builtins::test_support::TestRun;
    use crate::machine::core::{FrameStorageExt, program_storage, run_root_storage};
    use crate::machine::model::values::Module;
    let program = program_storage();
    let region = run_root_storage();
    let test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let types = test_run.registry_handle();
    let t = KType::of_kind(KKind::AnyType);
    let kt_number: KType = KType::NUMBER;
    let kt_str: KType = KType::STR;
    let kt_bool: KType = KType::BOOL;
    let kt_null: KType = KType::NULL;
    assert!(t.accepts_working_part(
        &spliced_part(&region, Carried::Type(kt_number)),
        types.registries()
    ));
    assert!(t.accepts_working_part(
        &spliced_part(&region, Carried::Type(kt_str)),
        types.registries()
    ));
    assert!(t.accepts_working_part(
        &spliced_part(&region, Carried::Type(kt_bool)),
        types.registries()
    ));
    assert!(t.accepts_working_part(
        &spliced_part(&region, Carried::Type(kt_null)),
        types.registries()
    ));
    // NewType / union-variant type tokens flow as sealed member handles in the type channel — a
    // `:Type` slot admits them when the spliced cell opens to a `Carried::Type`.
    let newtype_token: KType = newtype_member("Some", KType::NUMBER, &types);
    let struct_token: KType = record_newtype_member("Point", &types);
    assert!(t.accepts_working_part(
        &spliced_part(&region, Carried::Type(newtype_token)),
        types.registries()
    ));
    assert!(t.accepts_working_part(
        &spliced_part(&region, Carried::Type(struct_token)),
        types.registries()
    ));
    let child = scope.alloc_child_under_module(None);
    // A module value surfaces its principal signature, interned from its members before the value
    // exists — build it through the same door production does.
    let draft = ModuleDraft::empty();
    let self_sig = types.signature(SigSchema::raw_self_sig(child, &draft));
    let module = Module::alloc_at_child_scope("IntMod", child, draft, self_sig);
    // A module is a value: it reaches a slot on the Object channel, and a `:Type` slot refuses it.
    let module_value = scope.brand().allocator().value(KObject::Module(module));
    assert!(!t.accepts_working_part(
        &spliced_part(&region, Carried::Object(module_value)),
        types.registries()
    ));
    let sig_scope = scope.alloc_child_under_sig(type_token("Ordered"));
    let kt_sig: KType = types.signature(SigSchema::project_decl(sig_scope, types.registries()));
    // A signature is a type value: the `:Type` lattice top admits it; the proper tier does not.
    assert!(t.accepts_working_part(
        &spliced_part(&region, Carried::Type(kt_sig)),
        types.registries()
    ));
    assert!(!KType::of_kind(KKind::ProperType).accepts_working_part(
        &spliced_part(&region, Carried::Type(kt_sig)),
        types.registries()
    ));
    let n: &KObject<'_> = region.brand().alloc_scalar(Scalar::Number(7.0));
    let s: &KObject<'_> = region.brand().alloc_string("hi");
    assert!(!t.accepts_working_part(
        &spliced_part(&region, Carried::Object(n)),
        types.registries()
    ));
    assert!(!t.accepts_working_part(
        &spliced_part(&region, Carried::Object(s)),
        types.registries()
    ));
}

/// `:Signature` sits strictly below the `:Type` lattice top: a signature-slotted overload
/// out-specifies a `:Type` sibling when both admit a signature value, and the reverse fails.
#[test]
fn of_kind_signature_more_specific_than_any_type() {
    let registries = RunRegistries::new();
    assert!(
        KType::of_kind(KKind::Signature)
            .is_more_specific_than(KType::of_kind(KKind::AnyType), &registries)
    );
    assert!(
        !KType::of_kind(KKind::AnyType)
            .is_more_specific_than(KType::of_kind(KKind::Signature), &registries)
    );
}

/// `OfKind` is type-channel-only: a nominal-kind slot classifies a *type value* by its
/// `kind_of`, and never matches a runtime instance (a value is matched by a type, not a kind).
/// `OfKind(NewType)` admits a NewType *type* value, declines a `TypeConstructor` type value, and
/// declines the runtime `Wrapped` *instance* entirely; `OfKind(Proper)` subsumes the NewType type.
#[test]
fn of_kind_nominal_is_type_channel_only() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    use crate::machine::core::{FoldingBrand, FrameStorageExt, run_root_storage};
    use crate::witnessed::FoldedPlacement;
    let storage = run_root_storage();
    let region = storage.brand();
    let owned_cells = crate::machine::core::FrameCoverage::empty();
    let door = FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(region.handle()))
        .with_holder(&owned_cells);
    let newtype_ty = KType::of_kind(KKind::NewType);

    // The NewType *type value* — admitted in the type channel.
    let newtype_tv = newtype_member("Distance", KType::NUMBER, types);
    assert!(newtype_ty.accepts_working_part(
        &spliced_part(&storage, Carried::Type(newtype_tv)),
        &registries
    ));
    assert!(KType::of_kind(KKind::ProperType).accepts_working_part(
        &spliced_part(&storage, Carried::Type(newtype_tv)),
        &registries
    ));

    // A `TypeConstructor` type value is the wrong family — declined.
    let ctor_tv = RecursiveGroupWindow::seal_singleton(
        type_token("Result"),
        RelativeSchema::TypeConstructor {
            schema: TypeMemberMap::default(),
            param_names: Vec::new(),
        },
        None,
        types,
    );
    assert!(
        !newtype_ty
            .accepts_working_part(&spliced_part(&storage, Carried::Type(ctor_tv)), &registries)
    );

    // The runtime `Wrapped` *instance* is never matched by a kind slot.
    let w: &KObject<'_> = door.alloc_object_folded(KObject::wrapped_peel(
        door,
        &KObject::Number(3.0),
        newtype_tv,
    ));
    assert!(
        !newtype_ty.accepts_working_part(&spliced_part(&storage, Carried::Object(w)), &registries)
    );
    assert!(!newtype_ty.matches_value(w, &registries));
}

/// Pins the kind refinement: a `NewType`-kind sealed member is strictly more specific than
/// `OfKind(NewType)`, and incomparable with `OfKind(TypeConstructor)` (a sibling family).
#[test]
fn user_type_newtype_specificity_lattice() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let newtype_kind = KType::of_kind(KKind::NewType);
    let ctor_kind = KType::of_kind(KKind::TypeConstructor);
    let dist = newtype_member("Distance", KType::NUMBER, types);
    assert!(dist.is_more_specific_than(newtype_kind, &registries));
    assert!(!newtype_kind.is_more_specific_than(dist, &registries));
    assert!(!dist.is_more_specific_than(ctor_kind, &registries));
    assert!(!ctor_kind.is_more_specific_than(dist, &registries));
}

/// Specificity ordering for a sealed member against the `OfKind` kind lattice:
/// - a nominal kind is strictly under `Any` and strictly under `OfKind(Proper)`;
/// - a member of kind `K` is strictly under `OfKind(K)`;
/// - a member of one kind and `OfKind` of a different kind are incomparable.
#[test]
fn user_type_specificity_lattice() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let newtype_kind = KType::of_kind(KKind::NewType);
    let ctor_kind = KType::of_kind(KKind::TypeConstructor);
    let point = record_newtype_member("Point", types);
    // A nominal kind strictly under `Any` and under `OfKind(Proper)`.
    assert!(newtype_kind.is_more_specific_than(KType::ANY, &registries));
    assert!(!KType::ANY.is_more_specific_than(newtype_kind, &registries));
    assert!(newtype_kind.is_more_specific_than(KType::of_kind(KKind::ProperType), &registries));
    assert!(!KType::of_kind(KKind::ProperType).is_more_specific_than(newtype_kind, &registries));
    // A `NewType`-kind member strictly under `OfKind(NewType)`.
    assert!(point.is_more_specific_than(newtype_kind, &registries));
    assert!(!newtype_kind.is_more_specific_than(point, &registries));
    // Different-kind pairs incomparable.
    assert!(!point.is_more_specific_than(ctor_kind, &registries));
    assert!(!ctor_kind.is_more_specific_than(point, &registries));
}

/// Folded-pin `Signature` specificity rules (constraint role):
/// - A folded pin strictly refines the pin-free form: the extra manifest member satisfies
///   forward and blocks reverse.
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
    let registries = RunRegistries::new();
    let types = &registries.types;
    let ordered_schema = SigSchema {
        sig_id: Some(crate::machine::core::ScopeId::SENTINEL),
        abstract_members: TypeMemberMap::default(),
        manifest_members: TypeMemberMap::default(),
        value_slots: [(value_name("a", &registries), KType::NUMBER)]
            .into_iter()
            .collect(),
    };
    let hashed_schema = SigSchema {
        sig_id: Some(crate::machine::core::ScopeId::SENTINEL),
        abstract_members: TypeMemberMap::default(),
        manifest_members: TypeMemberMap::default(),
        value_slots: [(value_name("b", &registries), KType::NUMBER)]
            .into_iter()
            .collect(),
    };

    let type_sym = type_name("Type", &registries);
    let elt_sym = type_name("Elt", &registries);

    let bare = types.signature(ordered_schema.clone());
    let pinned_number = types.signature(
        ordered_schema
            .clone()
            .fold_pins(&[(type_sym, KType::NUMBER)], types),
    );
    let pinned_str = types.signature(
        ordered_schema
            .clone()
            .fold_pins(&[(type_sym, KType::STR)], types),
    );
    let pinned_two = types.signature(
        ordered_schema
            .clone()
            .fold_pins(&[(type_sym, KType::NUMBER), (elt_sym, KType::STR)], types),
    );
    let other_sig = types.signature(hashed_schema.fold_pins(&[(type_sym, KType::NUMBER)], types));
    let pinned_elt = types.signature(ordered_schema.fold_pins(&[(elt_sym, KType::NUMBER)], types));

    assert!(pinned_number.is_more_specific_than(bare, &registries));
    assert!(!bare.is_more_specific_than(pinned_number, &registries));
    assert!(pinned_two.is_more_specific_than(pinned_number, &registries));
    assert!(!pinned_number.is_more_specific_than(pinned_two, &registries));
    assert!(!pinned_number.is_more_specific_than(pinned_str, &registries));
    assert!(!pinned_str.is_more_specific_than(pinned_number, &registries));
    assert!(!pinned_number.is_more_specific_than(pinned_elt, &registries));
    assert!(!pinned_elt.is_more_specific_than(pinned_number, &registries));
    assert!(!pinned_number.is_more_specific_than(other_sig, &registries));
    assert!(!other_sig.is_more_specific_than(pinned_number, &registries));
}

/// The `Result` union's two sealed newtype members, as the prelude registers them. Identity is
/// content, so a `ConstructorApply` slot and a variant carrier match only when they name the
/// *same* member — every test below threads these two handles through both the slot and the value.
fn result_members(registries: &RunRegistries) -> (KType, KType) {
    let types = &registries.types;
    let window = RecursiveGroupWindow::for_binder(
        type_token("Result"),
        vec![type_token("Ok"), type_token("Error")],
    );
    window.fill_member(0, RelativeSchema::NewType(KType::ANY), types);
    let sealed = window
        .fill_member(1, RelativeSchema::NewType(KType::ANY), types)
        .expect("a two-member window seals on its second fill");
    (sealed.members[0], sealed.members[1])
}

/// The type `:(Result {Ok = …, Error = …})` lowers to: the union of one `ConstructorApply` per
/// member, each carrying its own same-named argument.
fn result_slot(members: (KType, KType), ok: KType, error: KType, types: &TypeRegistry) -> KType {
    types.union_of(&[
        types.constructor_apply(
            members.0,
            Record::from_pairs([(crate::builtins::test_support::binder_token("Ok"), ok)]),
        ),
        types.constructor_apply(
            members.1,
            Record::from_pairs([(crate::builtins::test_support::binder_token("Error"), error)]),
        ),
    ])
}

/// A `Result` variant value: `payload` wrapped under `member`, with no stamped type arguments —
/// the erased carrier a bare `Result.Ok 1` builds.
fn result_value<'a>(
    door: SubstrateDoor<'a, '_>,
    member: KType,
    payload: &KObject<'a>,
) -> KObject<'a> {
    KObject::wrapped_hold(door, payload, member)
}

/// A bare error carrier standing in for a caught error value: a `Wrapped` identified by `member`,
/// the shape `KError` lowering builds.
fn error_carrier<'a>(door: SubstrateDoor<'a, '_>, member: KType) -> KObject<'a> {
    KObject::wrapped_hold(door, &KObject::Number(0.0), member)
}

/// A singleton newtype member named `name`, for an error-type identity.
fn error_type_member(name: &str, types: &TypeRegistry) -> KType {
    RecursiveGroupWindow::seal_singleton(
        type_token(name),
        RelativeSchema::NewType(KType::ANY),
        None,
        types,
    )
}

/// `:(Result {Ok = …, Error = …})` slot admission: the union-of-applies admits an `Error` value
/// iff the inhabited `Error` member's payload satisfies that member's same-named argument. A
/// caught `Error(KError)` is rejected where that argument is `MyError` and accepted where it is
/// `KError` / `Any`. Identity is content, so the slot's argument and the value's payload carrier
/// share one member per error type.
#[test]
fn constructor_apply_result_checks_inhabited_error_param() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    container_door!(_storage, door);

    let members = result_members(&registries);
    let kerror_ty = error_type_member("KError", types);
    let my_error_ty = error_type_member("MyError", types);

    let slot_my_error = result_slot(members, KType::ANY, my_error_ty, types);
    let caught = result_value(door, members.1, &error_carrier(door, kerror_ty));
    assert!(!slot_my_error.matches_value(&caught, &registries));

    let slot_kerror = result_slot(members, KType::ANY, kerror_ty, types);
    assert!(slot_kerror.matches_value(&caught, &registries));

    let my_error = result_value(door, members.1, &error_carrier(door, my_error_ty));
    assert!(slot_my_error.matches_value(&my_error, &registries));
}

/// A value inhabits exactly one member, so the slot checks the `Ok` payload against the `Ok`
/// argument regardless of the `Error` one: an `Ok(42)` value admits any `Error` argument, because
/// the uninhabited member's application is never reached.
#[test]
fn constructor_apply_result_ok_admits_any_error_param() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    container_door!(_storage, door);
    let members = result_members(&registries);
    let my_error_ty = error_type_member("MyError", types);
    let ok_value = result_value(door, members.0, &KObject::Number(42.0));
    let slot = result_slot(members, KType::NUMBER, my_error_ty, types);
    assert!(slot.matches_value(&ok_value, &registries));
    let slot_str = result_slot(members, KType::STR, KType::ANY, types);
    assert!(!slot_str.matches_value(&ok_value, &registries));
}

/// Covariance for `ConstructorApply` carriers: a value stamped `apply(Ok, {Ok = Number})` is
/// admitted by the coarser `apply(Ok, {Ok = Any})` slot, and the refined slot is strictly more
/// specific, so dispatch tie-breaks toward the refined overload.
#[test]
fn constructor_apply_covariant_admission_and_specificity() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    container_door!(_storage, door);
    let members = result_members(&registries);
    let ok_arg = |arg| {
        types.constructor_apply(
            members.0,
            Record::from_pairs([(crate::builtins::test_support::binder_token("Ok"), arg)]),
        )
    };
    let stamped = KObject::wrapped_hold(door, &KObject::Number(1.0), ok_arg(KType::NUMBER));
    let coarse = ok_arg(KType::ANY);
    let refined = ok_arg(KType::NUMBER);
    assert!(coarse.matches_value(&stamped, &registries));
    assert!(refined.matches_value(&stamped, &registries));
    assert!(refined.is_more_specific_than(coarse, &registries));
    assert!(!coarse.is_more_specific_than(refined, &registries));
}

/// A stamped `ConstructorApply` identity (from ascription) is checked structurally against the
/// slot arguments, taking precedence over the erased inhabited-member path.
#[test]
fn constructor_apply_stamped_type_args_checked_structurally() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    container_door!(_storage, door);
    let members = result_members(&registries);
    let ok_arg = |arg| {
        types.constructor_apply(
            members.0,
            Record::from_pairs([(crate::builtins::test_support::binder_token("Ok"), arg)]),
        )
    };
    let stamped = KObject::wrapped_hold(door, &KObject::Number(1.0), ok_arg(KType::NUMBER));
    assert!(ok_arg(KType::NUMBER).matches_value(&stamped, &registries));
    assert!(ok_arg(KType::ANY).matches_value(&stamped, &registries));
    assert!(!ok_arg(KType::BOOL).matches_value(&stamped, &registries));
}

/// A union slot walks its members, so the whole `:(Result {…})` lowering admits a value through
/// whichever member it inhabits — the same verdict each per-member application gives alone.
#[test]
fn union_of_applies_admits_through_the_inhabited_member() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    container_door!(_storage, door);
    let members = result_members(&registries);
    let slot = result_slot(members, KType::NUMBER, KType::STR, types);
    assert!(slot.matches_value(
        &result_value(door, members.0, &KObject::Number(1.0)),
        &registries
    ));
    assert!(!slot.matches_value(
        &result_value(door, members.0, &KObject::Bool(true)),
        &registries
    ));
    let text = KObject::KString(door.allocator().text("x"));
    assert!(slot.matches_value(&result_value(door, members.1, &text), &registries));
    assert!(!slot.matches_value(
        &result_value(door, members.1, &KObject::Number(1.0)),
        &registries
    ));
}

use crate::machine::model::RunRegistries;
use crate::machine::model::types::{DeferredReturn, DeferredReturnSurface, ReturnType};

/// A function whose `ret` slot is a `DeferredReturn` carrier is strictly more specific
/// than the same shape with an `Any` return (covariant short-circuit), and the reverse
/// does not hold — `Any` never refines a precise placeholder.
#[test]
fn deferred_return_more_specific_than_any() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let deferred_ret = types.intern(TypeNode::DeferredReturn(DeferredReturnSurface::Type(
        type_token("Er"),
    )));
    let deferred = types.function_type(Record::new(), deferred_ret);
    let any = types.function_type(Record::new(), KType::ANY);
    assert!(deferred.is_more_specific_than(any, &registries));
    assert!(!any.is_more_specific_than(deferred, &registries));
}

/// Two function types differing only in their deferred-return shadow are distinct: not equal,
/// neither more specific than the other, and they hash apart.
#[test]
fn two_functions_differ_only_in_deferred_return_are_distinct() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    use std::hash::{Hash, Hasher};
    let er_ret = types.intern(TypeNode::DeferredReturn(DeferredReturnSurface::Type(
        type_token("Er"),
    )));
    let ar_ret = types.intern(TypeNode::DeferredReturn(DeferredReturnSurface::Type(
        type_token("Ar"),
    )));
    let er = types.function_type(Record::new(), er_ret);
    let ar = types.function_type(Record::new(), ar_ret);
    assert_ne!(er, ar);
    assert!(!er.is_more_specific_than(ar, &registries));
    assert!(!ar.is_more_specific_than(er, &registries));
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
    let registries = RunRegistries::new();
    let types = &registries.types;
    let program = crate::machine::core::program_storage();
    let candidate = ExpressionSignature::mint(
        program.brand().region(),
        ReturnType::Deferred(DeferredReturn::Type(type_token("Er"))),
        &[],
    );
    let no_params = Record::new();

    // Matching shadow → admit.
    let slot_er = types.intern(TypeNode::DeferredReturn(DeferredReturnSurface::Type(
        type_token("Er"),
    )));
    assert!(function_compat(
        &candidate,
        &no_params,
        slot_er,
        &registries
    ));

    // Differing shadow → reject.
    let slot_ar = types.intern(TypeNode::DeferredReturn(DeferredReturnSurface::Type(
        type_token("Ar"),
    )));
    assert!(!function_compat(
        &candidate,
        &no_params,
        slot_ar,
        &registries
    ));

    // Resolved slot → reject (opaque until elaboration).
    assert!(!function_compat(
        &candidate,
        &no_params,
        KType::NUMBER,
        &registries
    ));

    // `Any` slot → admit.
    assert!(function_compat(
        &candidate,
        &no_params,
        KType::ANY,
        &registries
    ));
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

// --- Union-type admissibility and specificity ------------------------------------

/// A union slot admits a value any of its members admits, and refuses one no member
/// admits — via both `accepts_carried` and `matches_value`.
#[test]
fn union_admits_member_typed_value() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    use crate::machine::core::{FrameStorageExt, run_root_storage};
    let storage = run_root_storage();
    let region = storage.brand();
    let n: &KObject<'_> = region.alloc_scalar(Scalar::Number(7.0));

    let number_or_str = types.union_of(&[KType::NUMBER, KType::STR]);
    let str_or_bool = types.union_of(&[KType::STR, KType::BOOL]);

    assert!(number_or_str.accepts_carried(Carried::Object(n), &registries));
    assert!(!str_or_bool.accepts_carried(Carried::Object(n), &registries));
    // `matches_value` agrees with `accepts_carried`.
    assert!(number_or_str.matches_value(n, &registries));
    assert!(!str_or_bool.matches_value(n, &registries));
}

/// A union honors a container value's memoized carried element type: a `List<Number>`
/// value is admitted by a union containing `:(LIST OF Number)`, refused by one without it.
#[test]
fn union_honors_memoized_list_element_type() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    use crate::machine::core::{FoldingBrand, FrameStorageExt, run_root_storage};
    use crate::witnessed::FoldedPlacement;
    let storage = run_root_storage();
    let owned_cells = crate::machine::core::FrameCoverage::empty();
    let door =
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(storage.brand().handle()))
            .with_holder(&owned_cells);
    let list_value: &KObject<'_> = door.alloc_object_folded(KObject::list_of_held(
        door,
        &[Held::Object(KObject::Number(1.0))],
        types,
    ));

    let with_list = types.union_of(&[types.list(KType::NUMBER), KType::STR]);
    let without_list = types.union_of(&[KType::NUMBER, KType::STR]);

    assert!(with_list.accepts_carried(Carried::Object(list_value), &registries));
    assert!(!without_list.accepts_carried(Carried::Object(list_value), &registries));
}

/// Specificity: each member refines its union (AC3); a union refines `Any` and a superset
/// union; a union is not more specific than a bare member nor than an equal union.
#[test]
fn union_specificity_ordering() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let number = KType::NUMBER;
    let number_or_str = types.union_of(&[KType::NUMBER, KType::STR]);
    let number_or_str_or_bool = types.union_of(&[KType::NUMBER, KType::STR, KType::BOOL]);

    // Each member is a subtype of the union.
    assert!(number.is_more_specific_than(number_or_str, &registries));
    // A union refines `Any`.
    assert!(number_or_str.is_more_specific_than(KType::ANY, &registries));
    // A union is not more specific than one of its members.
    assert!(!number_or_str.is_more_specific_than(number, &registries));
    // A subset union refines a superset union; the reverse does not hold.
    assert!(number_or_str.is_more_specific_than(number_or_str_or_bool, &registries));
    assert!(!number_or_str_or_bool.is_more_specific_than(number_or_str, &registries));
    // Equal unions (order-blind) are not strictly more specific than each other.
    let str_or_number = types.union_of(&[KType::STR, KType::NUMBER]);
    assert!(!number_or_str.is_more_specific_than(str_or_number, &registries));
}

/// A module value's `ktype()` reports its principal signature, and its identity is its self-sig
/// *content*: two modules with identical interfaces share one type, a differing member
/// distinguishes them.
#[test]
fn module_object_ktype_reports_self_sig() {
    use crate::builtins::test_support::TestRun;
    use crate::machine::core::{program_storage, run_root_storage};
    use crate::machine::model::KObject;
    use crate::machine::model::values::Module;

    let program = program_storage();
    let region = run_root_storage();
    let test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let types = test_run.registry_handle();

    // One-member modules built through the production door: the draft carries `Elt`, and the
    // self-sig is derived from it and interned before the value exists.
    let one_member = |name: &str, elt: KType| {
        let child = scope.alloc_child_under_module(None);
        let mut draft = ModuleDraft::empty();
        draft
            .type_members
            .insert(type_name("Elt", types.registries()), elt);
        let self_sig = types.signature(SigSchema::raw_self_sig(child, &draft));
        (
            Module::alloc_at_child_scope(name, child, draft, self_sig),
            self_sig,
        )
    };

    let (m, m_self_sig) = one_member("Mod", KType::NUMBER);
    let kt = KObject::Module(m).ktype();
    // Ruling 12: the `Signature` node carries no `sig_id` — identity is its self-sig
    // *content*, checked below.
    assert!(matches!(types.node(kt), TypeNode::Signature { .. }));
    // Identity is content: the module's type is the self-sig its members derive.
    assert_eq!(kt, m_self_sig);

    // A second module with the identical interface shares the type — content, not mint.
    let (m2, _) = one_member("Mod2", KType::NUMBER);
    assert_eq!(kt, KObject::Module(m2).ktype());

    // A module whose interface differs by one member is a distinct type.
    let (m3, _) = one_member("Mod3", KType::STR);
    assert_ne!(kt, KObject::Module(m3).ktype());
}

/// `matches_value` admits a module *object* into a `Signature` slot: a declared slot by
/// structural satisfaction (+ pin agreement), the empty signature for any module and no non-module
/// value.
#[test]
fn matches_value_admits_module_object_via_signature_slot() {
    use crate::builtins::test_support::TestRun;
    use crate::machine::core::{program_storage, run_root_storage};
    use crate::machine::model::KObject;
    use crate::machine::model::values::Module;

    let program = program_storage();
    let region = run_root_storage();
    let test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let types = test_run.registry_handle();

    // An empty signature (empty decl scope): every module bare-satisfies it, so the pins gate.
    let sig_scope = scope.alloc_child_under_sig(type_token("Ss"));
    let schema = SigSchema::project_decl(sig_scope, types.registries());

    let child = scope.alloc_child_under_module(None);
    let mut draft = ModuleDraft::empty();
    draft
        .type_members
        .insert(type_name("Type", types.registries()), KType::NUMBER);
    let self_sig = types.signature(SigSchema::raw_self_sig(child, &draft));
    let m: &Module = Module::alloc_at_child_scope("M", child, draft, self_sig);

    let declared = types.signature(schema.clone());
    assert!(declared.matches_value(&KObject::Module(m), types.registries()));

    let type_sym = type_name("Type", types.registries());
    let pinned_ok = types.signature(
        schema
            .clone()
            .fold_pins(&[(type_sym, KType::NUMBER)], &types),
    );
    let pinned_bad = types.signature(schema.fold_pins(&[(type_sym, KType::STR)], &types));
    assert!(pinned_ok.matches_value(&KObject::Module(m), types.registries()));
    assert!(!pinned_bad.matches_value(&KObject::Module(m), types.registries()));

    let empty = KType::EMPTY_SIGNATURE;
    assert!(empty.matches_value(&KObject::Module(m), types.registries()));
    assert!(!empty.matches_value(&KObject::Number(1.0), types.registries()));
}

/// Specificity over the module lattice: a module's self-sig refines a declared
/// signature it satisfies, and any non-empty signature refines the empty top. The signature
/// and module carry real members: under content identity a member-less signature *is* the
/// `:Module` top ([`EMPTY_SIGNATURE`](KType::EMPTY_SIGNATURE)), so degenerate empty points would
/// collapse into one type and there would be no ordering to test.
#[test]
fn specificity_self_sig_refines_declared_and_empty() {
    use crate::builtins::test_support::{TestRun, lookup_module};
    use crate::machine::model::KObject;
    use crate::machine::{program_storage, run_root_storage};
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let types = test_run.registry_handle();

    // `Ordered` requires a `compare` slot; `int_ord` supplies it plus an extra member, so its
    // self-sig strictly satisfies `Ordered`.
    test_run.run(
        "SIG Ordered = ((VAL compare :Number))\n\
         MODULE int_ord = ((LET compare = 7) (LET extra = 1))",
    );
    let declared = lookup_type(scope, "Ordered").expect("Ordered must bind a Signature KType");
    let m = lookup_module(scope, "int_ord", types.registries());

    let self_of = KObject::Module(m).ktype();
    let empty = KType::EMPTY_SIGNATURE;

    // `self_of ≺ declared` because `m`'s self-sig satisfies `Ordered`.
    assert!(self_of.is_more_specific_than(declared, types.registries()));
    // Any non-empty signature `≺ Empty`; `Empty` refines nothing narrower.
    assert!(declared.is_more_specific_than(empty, types.registries()));
    assert!(self_of.is_more_specific_than(empty, types.registries()));
    assert!(!empty.is_more_specific_than(declared, types.registries()));
    // `satisfied_by` routes a memoized self-sig element type through the `self-sig ≺ Declared` arm.
    assert!(declared.satisfied_by(self_of, types.registries()));
}

/// A member-free declared signature and a module with the matching slot shape are ONE type: the
/// schema digest feeds only the member/slot content — never `sig_id` or `path` — so the module's
/// self-sig type equals the declared signature by digest, not merely by mutual satisfaction.
#[test]
fn self_sig_type_equals_member_free_declared_sig() {
    use crate::builtins::test_support::{TestRun, lookup_module};
    use crate::machine::model::KObject;
    use crate::machine::{program_storage, run_root_storage};
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let types = test_run.registry_handle();

    test_run.run(
        "SIG HasLabel = ((VAL label :Str))\n\
         MODULE widget = ((LET label = (\"button\")))",
    );
    let declared = lookup_type(scope, "HasLabel").expect("HasLabel must bind a type");
    let m = lookup_module(scope, "widget", types.registries());
    assert_eq!(
        KObject::Module(m).ktype(),
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
    use crate::builtins::test_support::{TestRun, lookup_module};
    use crate::machine::model::KObject;
    use crate::machine::{program_storage, run_root_storage};
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let types = test_run.registry_handle();

    test_run.run(
        "SIG Pinned = ((LET Elem = Number) (VAL x :Elem))\n\
         MODULE pinned_mod = ((LET Elem = Number) (LET x = 5))",
    );
    let declared = lookup_type(scope, "Pinned").expect("Pinned must bind a type");
    let m = lookup_module(scope, "pinned_mod", types.registries());
    assert_eq!(
        KObject::Module(m).ktype(),
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
    use crate::builtins::test_support::{TestRun, lookup_module};
    use crate::machine::model::KObject;
    use crate::machine::{program_storage, run_root_storage};
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let types = test_run.registry_handle();

    test_run.run(
        "SIG Abstracted = ((TYPE Elem) (VAL x :Elem))\n\
         MODULE concrete = ((LET Elem = Number) (LET x = 5))",
    );
    let declared = lookup_type(scope, "Abstracted").expect("Abstracted must bind a type");
    let m = lookup_module(scope, "concrete", types.registries());
    let self_of = KObject::Module(m).ktype();

    assert_ne!(
        self_of, declared,
        "an abstract declared sig is a distinct type from any self-sig",
    );
    assert!(
        self_of.is_more_specific_than(declared, types.registries()),
        "the manifest self-sig strictly refines the abstract sig it satisfies",
    );
    assert!(
        !declared.is_more_specific_than(self_of, types.registries()),
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
    let registries = RunRegistries::new();
    let types = &registries.types;
    let n = types.list(KType::NUMBER);
    let a = types.list(KType::ANY);

    let first = n.is_more_specific_than(a, &registries);
    assert!(first);
    // Memoized unconditionally now: the outer `List` pair misses, and the walk's inner
    // `Number` vs `Any` leaf pair misses too (the old registry-probe gate is gone).
    assert_eq!(types.miss_count(), 2);
    assert_eq!(types.hit_count(), 0);

    let second = n.is_more_specific_than(a, &registries);
    assert_eq!(second, first);
    assert_eq!(types.hit_count(), 1, "second call must be a registry hit");
}

/// A negative verdict is recorded too: the second call to a pair the walk resolves `false` for
/// is a hit returning `false`.
#[test]
fn verdict_negative_also_recorded() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let a = types.list(KType::ANY);
    let n = types.list(KType::NUMBER);

    let first = a.is_more_specific_than(n, &registries);
    assert!(!first);
    // Outer `List<Any>` vs `List<Number>` misses, and the inner `Any` vs `Number` leaf misses.
    assert_eq!(types.miss_count(), 2);

    let second = a.is_more_specific_than(n, &registries);
    assert!(!second);
    assert_eq!(types.hit_count(), 1, "second call must be a registry hit");
}

/// Every specificity query is memoized unconditionally — no representation-probe gates the cache —
/// so even a leaf pair records a verdict on the first call and hits on the repeat.
#[test]
fn verdict_leaf_pairs_memoized_unconditionally() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    assert!(KType::NUMBER.is_more_specific_than(KType::ANY, &registries));
    assert_eq!(types.miss_count(), 1, "the first leaf query is a miss");
    assert_eq!(types.hit_count(), 0);

    assert!(KType::NUMBER.is_more_specific_than(KType::ANY, &registries));
    assert_eq!(types.hit_count(), 1, "the repeat is a registry hit");
}

/// Purity sanity: a cold registry computes the same composite verdict a warm one does — the
/// verdict cache is an accelerator, never load-bearing. The content must be present in each
/// registry (content lives in the registry now), so the cold registry re-interns it before the
/// query.
#[test]
fn verdict_purity_across_a_cold_registry() {
    let warm = RunRegistries::new();
    let n = warm.types.list(KType::NUMBER);
    let a = warm.types.list(KType::ANY);
    let before = n.is_more_specific_than(a, &warm);

    let cold = RunRegistries::new();
    let n_cold = cold.types.list(KType::NUMBER);
    let a_cold = cold.types.list(KType::ANY);
    let after = n_cold.is_more_specific_than(a_cold, &cold);
    assert_eq!(before, after);
}

#[test]
fn is_more_specific_identifier_beats_str() {
    let registries = RunRegistries::new();
    // A bare field token binds bare: `ATTR <field :Identifier>` outranks `ATTR <field :Str>`,
    // so a local string binding that shadows the name cannot claim the token.
    assert!(KType::IDENTIFIER.is_more_specific_than(KType::STR, &registries));
    assert!(!KType::STR.is_more_specific_than(KType::IDENTIFIER, &registries));
}

#[test]
fn is_more_specific_identifier_ranking_otherwise_intact() {
    let registries = RunRegistries::new();
    // `m.x` routes to the module overload, which depends on a signature out-specifying the
    // unconstrained-name slot.
    assert!(KType::EMPTY_SIGNATURE.is_more_specific_than(KType::IDENTIFIER, &registries));
    assert!(KType::IDENTIFIER.is_more_specific_than(KType::ANY, &registries));
    assert!(KType::NUMBER.is_more_specific_than(KType::IDENTIFIER, &registries));
}

/// [`KType::slot_ktype`] reads as the inverse of [`KType::accepts_part`], so the two must agree:
/// the type a part reports must be one that admits that part. Covers every `ExpressionPart` arm,
/// so adding an arm to either without the other fails here rather than silently rendering a type
/// dispatch would not have matched.
#[test]
fn slot_ktype_round_trips_through_accepts_part() {
    let registries = RunRegistries::new();
    let program = crate::machine::program_storage();
    let brand = program.brand();
    let node = brand.nested_node(&[]);
    let parts = [
        ExpressionPart::Literal(KLiteral::Number(1.0)),
        ExpressionPart::Literal(KLiteral::String("s")),
        ExpressionPart::Literal(KLiteral::Boolean(true)),
        ExpressionPart::Literal(KLiteral::Null),
        ExpressionPart::ListLiteral(&[]),
        ExpressionPart::DictLiteral(&[]),
        ExpressionPart::RecordLiteral(&[]),
        ExpressionPart::Identifier(value_name("x", &registries)),
        ExpressionPart::Type(type_name("Ty", &registries)),
        ExpressionPart::Expression(node),
        ExpressionPart::QuotedExpression(node),
        ExpressionPart::SigiledTypeExpr(node),
        ExpressionPart::RecordType(node),
    ];
    for part in parts {
        let slot = KType::slot_ktype(&part, &registries.types)
            .unwrap_or_else(|| panic!("every non-keyword part reports a slot type: {part:?}"));
        assert!(
            slot.accepts_part(&part, &registries.types),
            "slot_ktype({part:?}) = {} must be a type accepts_part admits",
            slot.name(&registries),
        );
        // `Any` admits everything, so the round trip alone would pass an arm that gave up and
        // reported it. The point of the door is a type dispatch actually matched on.
        assert_ne!(
            slot,
            KType::ANY,
            "slot_ktype({part:?}) must report the part's own type, not a stand-in",
        );
    }
}

/// A container's slot type is derived from its contents, so its element (or key/value, or field)
/// types are the join of what the literal holds — not a stand-in the round trip would wave through.
#[test]
fn a_container_slot_type_carries_its_contents() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let numbers = [
        ExpressionPart::Literal(KLiteral::Number(1.0)),
        ExpressionPart::Literal(KLiteral::Number(2.0)),
    ];
    assert_eq!(
        KType::slot_ktype(&ExpressionPart::ListLiteral(&numbers), types),
        Some(types.list(KType::NUMBER)),
    );

    let pairs = [(
        ExpressionPart::Literal(KLiteral::String("k")),
        ExpressionPart::Literal(KLiteral::Boolean(true)),
    )];
    assert_eq!(
        KType::slot_ktype(&ExpressionPart::DictLiteral(&pairs), types),
        Some(types.dict(KType::STR, KType::BOOL)),
    );

    let field = BinderSymbol::Value(value_name("x", &registries));
    let fields = [(field, ExpressionPart::Literal(KLiteral::Number(1.0)))];
    assert_eq!(
        KType::slot_ktype(&ExpressionPart::RecordLiteral(&fields), types),
        Some(types.record(Record::from_pairs([(field, KType::NUMBER)]))),
    );
}

/// A keyword fills no slot, so it reports no type — the arm the summary path renders as its own
/// spelling instead.
#[test]
fn a_keyword_reports_no_slot_type() {
    let registries = RunRegistries::new();
    let keyword = ExpressionPart::Keyword(
        crate::machine::model::KeywordSymbol::declared("PRINT", &registries.labels)
            .expect("a fixture keyword"),
    );
    assert!(KType::slot_ktype(&keyword, &registries.types).is_none());
}

/// The two binder-position slots' part admission. `NameToken` takes a bare name token of either
/// class and nothing else; `TypeNameToken` narrows that to the Type class. Neither takes a
/// literal, a container literal, or a raw type expression — a binder position is a name, so the
/// shapes that denote a *type* have no business there.
#[test]
fn name_token_slots_admit_only_bare_name_tokens() {
    use crate::builtins::test_support::identifier_part;
    let registries = RunRegistries::new();
    let types = &registries.types;
    let value_part = identifier_part("x");
    let type_part = ExpressionPart::Type(type_token("Point"));

    assert!(KType::NAME_TOKEN.accepts_part(&value_part, types));
    assert!(KType::NAME_TOKEN.accepts_part(&type_part, types));
    assert!(!KType::TYPE_NAME_TOKEN.accepts_part(&value_part, types));
    assert!(KType::TYPE_NAME_TOKEN.accepts_part(&type_part, types));

    for other in [
        ExpressionPart::Literal(KLiteral::Number(1.0)),
        ExpressionPart::Literal(KLiteral::Null),
        ExpressionPart::ListLiteral(&[]),
        ExpressionPart::RecordLiteral(&[]),
    ] {
        assert!(!KType::NAME_TOKEN.accepts_part(&other, types));
        assert!(!KType::TYPE_NAME_TOKEN.accepts_part(&other, types));
    }
}

/// Specificity for the binder slots. A bare name token outranks the `:Str` sibling that reads the
/// same position resolved (ATTR's dynamic read must not steal a spelled field), the two narrower
/// name slots outrank `NameToken`, and any concrete type outranks all of them — the
/// unconstrained-name rule.
#[test]
fn name_token_slots_order_against_str_and_concrete_slots() {
    let registries = RunRegistries::new();
    assert!(KType::NAME_TOKEN.is_more_specific_than(KType::STR, &registries));
    assert!(!KType::STR.is_more_specific_than(KType::NAME_TOKEN, &registries));

    assert!(KType::IDENTIFIER.is_more_specific_than(KType::NAME_TOKEN, &registries));
    assert!(KType::TYPE_NAME_TOKEN.is_more_specific_than(KType::NAME_TOKEN, &registries));
    assert!(!KType::NAME_TOKEN.is_more_specific_than(KType::IDENTIFIER, &registries));
    assert!(!KType::NAME_TOKEN.is_more_specific_than(KType::TYPE_NAME_TOKEN, &registries));

    for slot in [KType::NAME_TOKEN, KType::TYPE_NAME_TOKEN] {
        assert!(KType::NUMBER.is_more_specific_than(slot, &registries));
        assert!(!slot.is_more_specific_than(KType::NUMBER, &registries));
        assert!(slot.is_more_specific_than(KType::ANY, &registries));
        assert!(!KType::ANY.is_more_specific_than(slot, &registries));
    }
}

// ---------- union carrier slots ----------

/// The workhorse union: every carrier spelling of a type slot plus the value-name token — the
/// exact union the downstream type-slot untangle registers.
fn carrier_union(types: &TypeRegistry) -> KType {
    types.union_of(&[
        KType::TYPE_NAME_TOKEN,
        KType::SIGILED_TYPE_EXPR,
        KType::RECORD_TYPE,
        KType::IDENTIFIER,
    ])
}

/// Each carrier constant claims exactly the raw part shapes it captures, and the workhorse
/// union's members are pairwise disjoint — the property registration enforces, and the reason a
/// part maps to one member regardless of member order.
#[test]
fn capture_footprints_pin_each_carrier_to_its_own_shapes() {
    let shapes = |kt| capture_footprint(kt);
    let of = CaptureShapes::of;

    assert_eq!(shapes(KType::IDENTIFIER), of(CaptureShape::Identifier));
    assert_eq!(shapes(KType::TYPE_NAME_TOKEN), of(CaptureShape::TypeToken));
    assert_eq!(shapes(KType::SIGILED_TYPE_EXPR), of(CaptureShape::TypeExpr));
    assert_eq!(shapes(KType::RECORD_TYPE), of(CaptureShape::RecordType));
    assert_eq!(shapes(KType::KEXPRESSION), of(CaptureShape::Code));
    assert_eq!(
        shapes(KType::NAME_TOKEN),
        of(CaptureShape::Identifier).with(of(CaptureShape::TypeToken)),
    );
    // A kind slot claims the token it lowers and the two type-expression shapes it shape-admits.
    for kind in [KType::PROPER_TYPE, KType::ANY_TYPE] {
        assert_eq!(
            shapes(kind),
            of(CaptureShape::TypeToken)
                .with(of(CaptureShape::TypeExpr))
                .with(of(CaptureShape::RecordType)),
        );
    }
    // A value type claims nothing, so it never collides with a carrier member.
    for value in [KType::NUMBER, KType::STR, KType::ANY] {
        assert!(shapes(value).is_empty());
    }

    let workhorse = [
        KType::TYPE_NAME_TOKEN,
        KType::SIGILED_TYPE_EXPR,
        KType::RECORD_TYPE,
        KType::IDENTIFIER,
    ];
    for (i, a) in workhorse.iter().enumerate() {
        for b in &workhorse[i + 1..] {
            assert!(!shapes(*a).intersects(shapes(*b)));
        }
    }
    // The collisions registration rejects.
    assert!(shapes(KType::NAME_TOKEN).intersects(shapes(KType::TYPE_NAME_TOKEN)));
    assert!(shapes(KType::NAME_TOKEN).intersects(shapes(KType::IDENTIFIER)));
    assert!(shapes(KType::PROPER_TYPE).intersects(shapes(KType::SIGILED_TYPE_EXPR)));
    assert!(shapes(KType::PROPER_TYPE).intersects(shapes(KType::TYPE_NAME_TOKEN)));
}

/// `is_exact_carrier` names the five raw-capture constants and nothing else — `KExpression` is a
/// carrier but never a union member, and a kind slot is an ordinary eager member.
#[test]
fn exact_carrier_membership_is_the_five_raw_capture_constants() {
    for carrier in [
        KType::IDENTIFIER,
        KType::NAME_TOKEN,
        KType::TYPE_NAME_TOKEN,
        KType::SIGILED_TYPE_EXPR,
        KType::RECORD_TYPE,
    ] {
        assert!(is_exact_carrier(carrier));
    }
    for other in [
        KType::KEXPRESSION,
        KType::PROPER_TYPE,
        KType::ANY_TYPE,
        KType::NUMBER,
        KType::ANY,
    ] {
        assert!(!is_exact_carrier(other));
    }
}

/// The routing lookup: each raw part shape picks out the one member that claims it, and a part
/// no member claims answers `None` so the slot falls through to eager handling.
#[test]
fn raw_capture_member_picks_the_claiming_member() {
    let program = crate::machine::core::program_storage();
    let brand = program.brand();
    let registries = RunRegistries::new();
    let types = &registries.types;
    let slot = carrier_union(types);

    let cases = [
        (
            ExpressionPart::Type(type_token("Meters")),
            KType::TYPE_NAME_TOKEN,
        ),
        (
            ExpressionPart::SigiledTypeExpr(brand.nested_node_from_iter(std::iter::empty())),
            KType::SIGILED_TYPE_EXPR,
        ),
        (
            ExpressionPart::RecordType(brand.nested_node_from_iter(std::iter::empty())),
            KType::RECORD_TYPE,
        ),
        (
            ExpressionPart::Identifier(value_name("width", &registries)),
            KType::IDENTIFIER,
        ),
    ];
    for (part, expected) in cases {
        assert_eq!(slot.raw_capture_member(&part, types), Some(expected));
        // A bare carrier is not a union, so it routes by its own exact-constant arm, not here.
        assert_eq!(expected.raw_capture_member(&part, types), None);
    }
    // A literal is no carrier's shape.
    assert_eq!(
        slot.raw_capture_member(&ExpressionPart::Literal(KLiteral::Number(1.0)), types),
        None,
    );
}

/// Raw capture rides exact carrier members only: an `of_kind(…)` member is an ordinary eager
/// member, so a union carrying one routes a bare `Type` token nowhere raw.
#[test]
fn a_kind_member_carries_no_raw_capture() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let slot = types.union_of(&[KType::PROPER_TYPE, KType::NUMBER]);
    let part = ExpressionPart::Type(type_token("Meters"));
    assert_eq!(slot.raw_capture_member(&part, types), None);
    // Bind-time reduction still finds it: the member lowers the token rather than capturing it.
    assert_eq!(
        slot.capture_member_for(&part, types),
        Some(KType::PROPER_TYPE),
    );
}

/// `capture_member_for` widens to a kind member for a bare `Type` token and for nothing else —
/// the two type-expression shapes a kind member shape-admits arrive already sub-dispatched.
#[test]
fn capture_member_for_widens_only_for_a_type_token() {
    let program = crate::machine::core::program_storage();
    let brand = program.brand();
    let registries = RunRegistries::new();
    let types = &registries.types;
    let slot = types.union_of(&[KType::ANY_TYPE, KType::NUMBER]);

    assert_eq!(
        slot.capture_member_for(&ExpressionPart::Type(type_token("Meters")), types),
        Some(KType::ANY_TYPE),
    );
    let sigil = ExpressionPart::SigiledTypeExpr(brand.nested_node_from_iter(std::iter::empty()));
    assert_eq!(slot.capture_member_for(&sigil, types), None);
}

/// Membership distribution: the primitive every structural exact-constant dispatch rule reads
/// through, true for the bare constant and for a union carrying it.
#[test]
fn union_has_member_covers_the_bare_and_union_spellings() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let slot = carrier_union(types);

    assert!(slot.union_has_member(KType::SIGILED_TYPE_EXPR, types));
    assert!(slot.union_has_member(KType::RECORD_TYPE, types));
    assert!(!slot.union_has_member(KType::KEXPRESSION, types));
    assert!(KType::SIGILED_TYPE_EXPR.union_has_member(KType::SIGILED_TYPE_EXPR, types));
    assert!(!KType::SIGILED_TYPE_EXPR.union_has_member(KType::RECORD_TYPE, types));
}

/// The auto-wrap exclusion is blanket by slot type and distributes: a union owns a bare name as
/// soon as any *literal-name* member does, and a union of a kind expectation with a value type
/// owns none — a kind slot asks for a type value, so its bare name resolves.
#[test]
fn owns_bare_name_distributes_over_union_members() {
    let registries = RunRegistries::new();
    let types = &registries.types;

    assert!(carrier_union(types).owns_bare_name(types));
    assert!(
        !types
            .union_of(&[KType::PROPER_TYPE, KType::NUMBER])
            .owns_bare_name(types)
    );
    assert!(KType::NAME_TOKEN.owns_bare_name(types));
    assert!(!KType::KEXPRESSION.owns_bare_name(types));
    assert!(
        !types
            .union_of(&[KType::NUMBER, KType::STR])
            .owns_bare_name(types)
    );
    // A union of pure carriers with no name member owns nothing bare.
    assert!(
        !types
            .union_of(&[KType::SIGILED_TYPE_EXPR, KType::RECORD_TYPE])
            .owns_bare_name(types)
    );
}

/// Registration's two rules on a union carrier slot, and the unions they leave alone.
#[test]
fn carrier_union_validation_rejects_code_and_overlapping_members() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let accepts = |kt| carrier_union_error(kt, &registries).is_none();

    assert!(accepts(carrier_union(types)));
    assert!(accepts(
        types.union_of(&[KType::TYPE_NAME_TOKEN, KType::NUMBER])
    ));
    // Not a carrier union at all: a user-spellable value union is an ordinary eager slot.
    assert!(accepts(types.union_of(&[KType::NUMBER, KType::STR])));
    assert!(accepts(
        types.union_of(&[KType::KEXPRESSION, KType::NUMBER])
    ));
    // And a bare carrier is not a union.
    assert!(accepts(KType::SIGILED_TYPE_EXPR));

    for rejected in [
        types.union_of(&[KType::KEXPRESSION, KType::SIGILED_TYPE_EXPR]),
        types.union_of(&[KType::TYPE_NAME_TOKEN, KType::NAME_TOKEN]),
        types.union_of(&[KType::IDENTIFIER, KType::NAME_TOKEN]),
        types.union_of(&[KType::PROPER_TYPE, KType::SIGILED_TYPE_EXPR]),
        types.union_of(&[KType::PROPER_TYPE, KType::TYPE_NAME_TOKEN]),
        types.union_of(&[KType::ANY_TYPE, KType::RECORD_TYPE]),
    ] {
        assert!(
            carrier_union_error(rejected, &registries).is_some(),
            "{} should be rejected at registration",
            rejected.name(&registries),
        );
    }
}
