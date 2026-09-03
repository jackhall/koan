//! Unit tests for the signature-subtyping relation, its schema, and abstract-member
//! substitution. Schemas are built both directly (owned `KType` handles) and by projecting parsed
//! SIG declarations, pinned via [`SigSchema::fold_pins`].

use super::*;
use crate::builtins::test_support::lookup_type;
use crate::builtins::test_support::{type_name, value_name};
use crate::machine::core::ScopeId;
use crate::machine::model::types::{Record, RecursiveGroupWindow, RelativeSchema};

// --- region-free builders -------------------------------------------------------------

/// The parameter names a constructor of the given arity declares, shared by the declared-family
/// and abstract-slot builders so a sub binding and a sup slot agree by name.
fn params(arity: usize, registries: &RunRegistries) -> Vec<TypeSymbol> {
    (0..arity)
        .map(|i| type_name(&format!("Param{i}"), registries))
        .collect()
}

/// A declared constructor family: a `TypeConstructor`-kind sealed member of the given arity.
fn ctor(name: &str, arity: usize, registries: &RunRegistries) -> KType {
    RecursiveGroupWindow::seal_singleton(
        type_token(name),
        RelativeSchema::TypeConstructor {
            schema: TypeMemberMap::default(),
            param_names: params(arity, registries),
        },
        None,
        &registries.types,
    )
}

/// A SIG's first-order abstract member.
fn sig_abstract(id: ScopeId, name: &str, registries: &RunRegistries) -> KType {
    registries.types.intern(TypeNode::AbstractType {
        source: id,
        name: type_name(name, registries),
        param_names: Vec::new(),
        nonce: None,
    })
}

/// A SIG's higher-kinded abstract member, over `arity` parameters named as [`params`] names them.
fn sig_abstract_ctor(id: ScopeId, name: &str, arity: usize, registries: &RunRegistries) -> KType {
    registries.types.intern(TypeNode::AbstractType {
        source: id,
        name: type_name(name, registries),
        param_names: params(arity, registries),
        nonce: None,
    })
}

fn fn_type(params: Vec<(&str, KType)>, ret: KType, types: &TypeRegistry) -> KType {
    types.function_type(
        Record::from_pairs(
            params
                .into_iter()
                .map(|(n, t)| (crate::builtins::test_support::binder_token(n), t)),
        ),
        ret,
    )
}

fn schema(
    sig_id: Option<ScopeId>,
    abstract_members: Vec<(&str, KType)>,
    manifest_members: Vec<(&str, KType)>,
    value_slots: Vec<(&str, KType)>,
    registries: &RunRegistries,
) -> SigSchema {
    SigSchema {
        sig_id,
        abstract_members: abstract_members
            .into_iter()
            .map(|(n, k)| (type_name(n, registries), k))
            .collect(),
        manifest_members: manifest_members
            .into_iter()
            .map(|(n, k)| (type_name(n, registries), k))
            .collect(),
        value_slots: value_slots
            .into_iter()
            .map(|(n, k)| (value_name(n, registries), k))
            .collect(),
    }
}

const SUP_ID: ScopeId = ScopeId::from_raw(0, 0xDEAD);
const REAL_ID: ScopeId = ScopeId::from_raw(0, 0xC0DE);

use super::sig_subtype as relation;
use crate::builtins::test_support::type_token;
use crate::machine::model::RunRegistries;
use crate::machine::model::TypeMemberMap;

/// Run the relation against a caller-supplied registry and unbox the failure so `matches!` can
/// name the variant directly. The schemas' interned members and the relation walk must share one
/// registry, so every test that builds abstract or constructor members threads its own `types`.
#[allow(clippy::result_large_err)] // test ergonomics: unbox so assertions name the variant
fn check(
    sub: &SigSchema,
    sup: &SigSchema,
    registries: &RunRegistries,
) -> Result<(), SigSubtypeFailure> {
    relation(sub, sup, registries).map_err(|e| *e)
}

// --- width ----------------------------------------------------------------------------

#[test]
fn width_extra_members_and_slots_still_subtype() {
    let registries = RunRegistries::new();
    let sup = schema(
        None,
        vec![],
        vec![("Tag", KType::NUMBER)],
        vec![("v", KType::STR)],
        &registries,
    );
    let sub = schema(
        None,
        vec![],
        vec![("Tag", KType::NUMBER), ("Extra", KType::BOOL)],
        vec![("v", KType::STR), ("w", KType::NUMBER)],
        &registries,
    );
    assert!(check(&sub, &sup, &registries).is_ok());
}

// --- abstract members: first-order ----------------------------------------------------

#[test]
fn abstract_fo_satisfied_by_manifest_and_by_abstract() {
    let registries = RunRegistries::new();
    let sup = schema(
        Some(SUP_ID),
        vec![("Elt", sig_abstract(SUP_ID, "Elt", &registries))],
        vec![],
        vec![],
        &registries,
    );
    // Sub supplies `Elt` as a manifest non-constructor.
    let sub_manifest = schema(
        None,
        vec![],
        vec![("Elt", KType::NUMBER)],
        vec![],
        &registries,
    );
    assert!(check(&sub_manifest, &sup, &registries).is_ok());
    // Sub supplies `Elt` as its own first-order abstract member.
    let sub_abstract = schema(
        Some(REAL_ID),
        vec![("Elt", sig_abstract(REAL_ID, "Elt", &registries))],
        vec![],
        vec![],
        &registries,
    );
    assert!(check(&sub_abstract, &sup, &registries).is_ok());
}

#[test]
fn abstract_fo_refused_by_constructor() {
    let registries = RunRegistries::new();
    let sup = schema(
        Some(SUP_ID),
        vec![("Elt", sig_abstract(SUP_ID, "Elt", &registries))],
        vec![],
        vec![],
        &registries,
    );
    let sub = schema(
        None,
        vec![],
        vec![("Elt", ctor("Elt", 1, &registries))],
        vec![],
        &registries,
    );
    assert!(matches!(
        check(&sub, &sup, &registries),
        Err(SigSubtypeFailure::KindMismatch {
            expected_params: None,
            ..
        })
    ));
}

#[test]
fn abstract_member_missing_fails() {
    let registries = RunRegistries::new();
    let sup = schema(
        Some(SUP_ID),
        vec![("Elt", sig_abstract(SUP_ID, "Elt", &registries))],
        vec![],
        vec![],
        &registries,
    );
    let sub = schema(None, vec![], vec![], vec![], &registries);
    assert!(matches!(
        check(&sub, &sup, &registries),
        Err(SigSubtypeFailure::MissingTypeMember { .. })
    ));
}

// --- abstract members: higher-kinded --------------------------------------------------

#[test]
fn abstract_hk_arity_one_satisfied_by_matching_constructor() {
    let registries = RunRegistries::new();
    let sup = schema(
        Some(SUP_ID),
        vec![("Wrap", sig_abstract_ctor(SUP_ID, "Wrap", 1, &registries))],
        vec![],
        vec![],
        &registries,
    );
    let sub = schema(
        None,
        vec![],
        vec![("Wrap", ctor("MyWrap", 1, &registries))],
        vec![],
        &registries,
    );
    assert!(check(&sub, &sup, &registries).is_ok());
}

#[test]
fn abstract_hk_refused_by_proper_type_by_wrong_arity_and_by_abstract_fo() {
    let registries = RunRegistries::new();
    let sup = schema(
        Some(SUP_ID),
        vec![("Wrap", sig_abstract_ctor(SUP_ID, "Wrap", 1, &registries))],
        vec![],
        vec![],
        &registries,
    );
    // A proper type has no arity.
    let by_proper = schema(
        None,
        vec![],
        vec![("Wrap", KType::NUMBER)],
        vec![],
        &registries,
    );
    assert!(matches!(
        check(&by_proper, &sup, &registries),
        Err(SigSubtypeFailure::KindMismatch {
            expected_params: Some(_),
            ..
        })
    ));
    // An arity-2 constructor cannot fill an arity-1 slot.
    let by_arity2 = schema(
        None,
        vec![],
        vec![("Wrap", ctor("Pair", 2, &registries))],
        vec![],
        &registries,
    );
    assert!(matches!(
        check(&by_arity2, &sup, &registries),
        Err(SigSubtypeFailure::KindMismatch {
            expected_params: Some(_),
            ..
        })
    ));
    // A first-order abstract member is not a constructor.
    let by_fo = schema(
        Some(REAL_ID),
        vec![("Wrap", sig_abstract(REAL_ID, "Wrap", &registries))],
        vec![],
        vec![],
        &registries,
    );
    assert!(matches!(
        check(&by_fo, &sup, &registries),
        Err(SigSubtypeFailure::KindMismatch {
            expected_params: Some(_),
            ..
        })
    ));
}

// --- manifest members -----------------------------------------------------------------

#[test]
fn manifest_equal_passes_unequal_and_missing_fail() {
    let registries = RunRegistries::new();
    let sup = schema(
        None,
        vec![],
        vec![("Tag", KType::NUMBER)],
        vec![],
        &registries,
    );
    assert!(
        check(
            &schema(
                None,
                vec![],
                vec![("Tag", KType::NUMBER)],
                vec![],
                &registries
            ),
            &sup,
            &registries
        )
        .is_ok()
    );
    assert!(matches!(
        check(
            &schema(None, vec![], vec![("Tag", KType::STR)], vec![], &registries),
            &sup,
            &registries
        ),
        Err(SigSubtypeFailure::ManifestMismatch { .. })
    ));
    assert!(matches!(
        check(
            &schema(None, vec![], vec![], vec![], &registries),
            &sup,
            &registries
        ),
        Err(SigSubtypeFailure::MissingTypeMember { .. })
    ));
}

#[test]
fn manifest_requirement_refuses_abstract_sub_member() {
    let registries = RunRegistries::new();
    let sup = schema(
        None,
        vec![],
        vec![("Tag", KType::NUMBER)],
        vec![],
        &registries,
    );
    let sub = schema(
        Some(REAL_ID),
        vec![("Tag", sig_abstract(REAL_ID, "Tag", &registries))],
        vec![],
        vec![],
        &registries,
    );
    assert!(matches!(
        check(&sub, &sup, &registries),
        Err(SigSubtypeFailure::ManifestMismatch { .. })
    ));
}

// --- value slots: covariance ----------------------------------------------------------

#[test]
fn value_slot_covariant_depth() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    // A slot declared `-> Any` is filled by a member `-> Number`; the reverse fails.
    let sup_any = schema(
        None,
        vec![],
        vec![],
        vec![("f", fn_type(vec![], KType::ANY, types))],
        &registries,
    );
    let sub_number = schema(
        None,
        vec![],
        vec![],
        vec![("f", fn_type(vec![], KType::NUMBER, types))],
        &registries,
    );
    assert!(check(&sub_number, &sup_any, &registries).is_ok());

    let sup_number = schema(
        None,
        vec![],
        vec![],
        vec![("f", fn_type(vec![], KType::NUMBER, types))],
        &registries,
    );
    let sub_any = schema(
        None,
        vec![],
        vec![],
        vec![("f", fn_type(vec![], KType::ANY, types))],
        &registries,
    );
    assert!(matches!(
        check(&sub_any, &sup_number, &registries),
        Err(SigSubtypeFailure::ValueSlotMismatch { .. })
    ));
}

#[test]
fn value_slot_equal_passes_missing_fails() {
    let registries = RunRegistries::new();
    let sup = schema(
        None,
        vec![],
        vec![],
        vec![("v", KType::NUMBER)],
        &registries,
    );
    assert!(
        check(
            &schema(
                None,
                vec![],
                vec![],
                vec![("v", KType::NUMBER)],
                &registries
            ),
            &sup,
            &registries
        )
        .is_ok()
    );
    assert!(matches!(
        check(
            &schema(None, vec![], vec![], vec![], &registries),
            &sup,
            &registries
        ),
        Err(SigSubtypeFailure::MissingValueSlot { .. })
    ));
}

// --- substitution through value-slot types --------------------------------------------

#[test]
fn value_slot_abstract_ref_substitutes_to_sub_manifest() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    // Super: abstract `Type`, slot `compare :(FN :{x :Type, y :Type} -> Number)`.
    let sup = schema(
        Some(SUP_ID),
        vec![("Type", sig_abstract(SUP_ID, "Type", &registries))],
        vec![],
        vec![(
            "compare",
            fn_type(
                vec![
                    ("x", sig_abstract(SUP_ID, "Type", &registries)),
                    ("y", sig_abstract(SUP_ID, "Type", &registries)),
                ],
                KType::NUMBER,
                types,
            ),
        )],
        &registries,
    );
    // Sub: manifest `Type = Number`, slot `compare :(FN :{x :Number, y :Number} -> Number)`.
    let sub = schema(
        None,
        vec![],
        vec![("Type", KType::NUMBER)],
        vec![(
            "compare",
            fn_type(
                vec![("x", KType::NUMBER), ("y", KType::NUMBER)],
                KType::NUMBER,
                types,
            ),
        )],
        &registries,
    );
    assert!(check(&sub, &sup, &registries).is_ok());
}

#[test]
fn value_slot_list_of_abstract_ref_substitutes_nested() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    // Super: abstract `Type`, slot `items :(LIST OF Type)` — the substitution point sits
    // *nested* inside a container, so the walk must descend the `List` before comparing.
    let sup = schema(
        Some(SUP_ID),
        vec![("Type", sig_abstract(SUP_ID, "Type", &registries))],
        vec![],
        vec![(
            "items",
            types.list(sig_abstract(SUP_ID, "Type", &registries)),
        )],
        &registries,
    );
    // Sub with `Type = Number` and `items :(LIST OF Number)` subtypes.
    let sub_ok = schema(
        None,
        vec![],
        vec![("Type", KType::NUMBER)],
        vec![("items", types.list(KType::NUMBER))],
        &registries,
    );
    assert!(check(&sub_ok, &sup, &registries).is_ok());
    // `items :(LIST OF Str)` against `Type = Number` fails at the nested element compare.
    let sub_bad = schema(
        None,
        vec![],
        vec![("Type", KType::NUMBER)],
        vec![("items", types.list(KType::STR))],
        &registries,
    );
    assert!(matches!(
        check(&sub_bad, &sup, &registries),
        Err(SigSubtypeFailure::ValueSlotMismatch { .. })
    ));
}

// --- pins via fold_pins ----------------------------------------------------------------

#[test]
fn pin_converts_abstract_to_manifest_via_parsed_sig() {
    use crate::builtins::test_support::TestRun;
    use crate::machine::core::{program_storage, run_root_storage};

    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("SIG Pinnable = ((TYPE Elt) (VAL v :Number))");
    let sig_schema = match lookup_type(scope, "Pinnable") {
        Some(kt) => match test_run.types().node(kt) {
            TypeNode::Signature { schema, .. } => schema,
            _ => panic!("Pinnable should resolve to a signature"),
        },
        _ => panic!("Pinnable should resolve to a signature"),
    };
    // `S WITH {Elt = Number}` fixes the abstract member manifest.
    let elt = type_name("Elt", test_run.registries());
    let pinned = sig_schema.fold_pins(&[(elt, KType::NUMBER)], test_run.types());
    assert!(pinned.abstract_members.is_empty());
    assert_eq!(pinned.manifest_members.get(&elt), Some(&KType::NUMBER));

    let elt_str = schema(
        None,
        vec![],
        vec![("Elt", KType::STR)],
        vec![("v", KType::NUMBER)],
        test_run.registries(),
    );
    assert!(matches!(
        check(&elt_str, &pinned, test_run.registries()),
        Err(SigSubtypeFailure::ManifestMismatch { .. })
    ));
    let elt_number = schema(
        None,
        vec![],
        vec![("Elt", KType::NUMBER)],
        vec![("v", KType::NUMBER)],
        test_run.registries(),
    );
    assert!(check(&elt_number, &pinned, test_run.registries()).is_ok());
}

#[test]
fn sig_to_sig_entailment_over_shared_abstract() {
    use crate::builtins::test_support::TestRun;
    use crate::machine::core::{program_storage, run_root_storage};

    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "SIG Alpha = ((TYPE Elem) (VAL compare :(FN :{x :Elem} -> Number)))\n\
         SIG Beta = ((TYPE Elem) (VAL compare :(FN :{x :Elem} -> Number)))",
    );
    let a = match lookup_type(scope, "Alpha") {
        Some(kt) => match test_run.types().node(kt) {
            TypeNode::Signature { schema, .. } => schema,
            _ => panic!("Alpha should resolve to a signature"),
        },
        _ => panic!("Alpha should resolve to a signature"),
    };
    let b = match lookup_type(scope, "Beta") {
        Some(kt) => match test_run.types().node(kt) {
            TypeNode::Signature { schema, .. } => schema,
            _ => panic!("Beta should resolve to a signature"),
        },
        _ => panic!("Beta should resolve to a signature"),
    };
    // Two SIGs declaring the same abstract member and slot entail each other: the
    // substitution maps each super `Type` ref onto the sub's own abstract identity.
    assert!(check(&a, &b, test_run.registries()).is_ok());
    assert!(check(&b, &a, test_run.registries()).is_ok());
}

// --- substitute_sig_members units -----------------------------------------------------

/// An opaque ascription's generative mint shares its declaring binder's `source` and name — only
/// the nonce separates them — and must not be mistaken for a reference to that declaration.
#[test]
fn substitute_leaves_a_generative_mint_alone() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let mut map = TypeMemberMap::default();
    map.insert(type_name("Type", &registries), KType::NUMBER);

    let mint = types.intern(TypeNode::AbstractType {
        source: SUP_ID,
        name: type_name("Type", &registries),
        param_names: Vec::new(),
        nonce: Some(ScopeId::from_raw(0, 0xBEEF)),
    });
    assert_eq!(substitute_sig_members(mint, SUP_ID, &map, types), mint);
    // The declaration it was minted from still substitutes.
    assert_eq!(
        substitute_sig_members(
            sig_abstract(SUP_ID, "Type", &registries),
            SUP_ID,
            &map,
            types
        ),
        KType::NUMBER
    );
}

#[test]
fn substitute_top_level_and_nested() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let mut map = TypeMemberMap::default();
    map.insert(type_name("Type", &registries), KType::NUMBER);

    // Top level.
    assert_eq!(
        substitute_sig_members(
            sig_abstract(SUP_ID, "Type", &registries),
            SUP_ID,
            &map,
            types
        ),
        KType::NUMBER
    );
    // Inside KFunction params and ret.
    let f = fn_type(
        vec![("x", sig_abstract(SUP_ID, "Type", &registries))],
        sig_abstract(SUP_ID, "Type", &registries),
        types,
    );
    assert_eq!(
        substitute_sig_members(f, SUP_ID, &map, types),
        fn_type(vec![("x", KType::NUMBER)], KType::NUMBER, types)
    );
    // Inside List, Record, Union.
    assert_eq!(
        substitute_sig_members(
            types.list(sig_abstract(SUP_ID, "Type", &registries)),
            SUP_ID,
            &map,
            types
        ),
        types.list(KType::NUMBER)
    );
    let rec = types.record(Record::from_pairs([(
        crate::builtins::test_support::binder_token("f"),
        sig_abstract(SUP_ID, "Type", &registries),
    )]));
    assert_eq!(
        substitute_sig_members(rec, SUP_ID, &map, types),
        types.record(Record::from_pairs([(
            crate::builtins::test_support::binder_token("f"),
            KType::NUMBER
        )]))
    );
    let union = types.union_of(&[sig_abstract(SUP_ID, "Type", &registries), KType::STR]);
    assert_eq!(
        substitute_sig_members(union, SUP_ID, &map, types),
        types.union_of(&[KType::NUMBER, KType::STR])
    );
}

#[test]
fn substitute_constructor_apply_abstract_ctor_position() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let mut map = TypeMemberMap::default();
    let real = ctor("MyWrap", 1, &registries);
    map.insert(type_name("Wrap", &registries), real);
    let applied = types.constructor_apply(
        sig_abstract_ctor(SUP_ID, "Wrap", 1, &registries),
        Record::from_pairs([(
            crate::builtins::test_support::binder_token("Type"),
            KType::NUMBER,
        )]),
    );
    assert_eq!(
        substitute_sig_members(applied, SUP_ID, &map, types),
        types.constructor_apply(
            real,
            Record::from_pairs([(
                crate::builtins::test_support::binder_token("Type"),
                KType::NUMBER
            )])
        )
    );
}

#[test]
fn substitute_leaves_non_matching_sig_id_and_unknown_names() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let map = TypeMemberMap::default();
    // Unknown name — untouched even at the matching sig_id.
    let unknown = sig_abstract(SUP_ID, "Other", &registries);
    assert_eq!(
        substitute_sig_members(unknown, SUP_ID, &map, types),
        unknown
    );
    // Non-matching sig_id — untouched.
    let mut with_type = TypeMemberMap::default();
    with_type.insert(type_name("Type", &registries), KType::NUMBER);
    let other_sig = sig_abstract(SUP_ID, "Type", &registries);
    assert_eq!(
        substitute_sig_members(other_sig, REAL_ID, &with_type, types),
        other_sig
    );
}

#[test]
fn constructor_param_names_probe() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    assert_eq!(
        constructor_param_names(sig_abstract_ctor(SUP_ID, "Wrap", 1, &registries), types),
        Some(params(1, &registries)),
    );
    assert_eq!(
        constructor_param_names(ctor("Wrap", 2, &registries), types),
        Some(params(2, &registries)),
    );
    assert_eq!(constructor_param_names(KType::NUMBER, types), None);
    assert_eq!(
        constructor_param_names(sig_abstract(SUP_ID, "Elt", &registries), types),
        None
    );
}

/// Parameter names are interface: a family declaring a differently-named parameter does not
/// supply the slot, and the failure names the expected set.
#[test]
fn abstract_hk_refused_by_differently_named_parameter() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let sup = schema(
        Some(SUP_ID),
        vec![("Wrap", sig_abstract_ctor(SUP_ID, "Wrap", 1, &registries))],
        vec![],
        vec![],
        &registries,
    );
    let other_names = RecursiveGroupWindow::seal_singleton(
        type_token("MyWrap"),
        RelativeSchema::TypeConstructor {
            schema: TypeMemberMap::default(),
            param_names: vec![type_name("Item", &registries)],
        },
        None,
        types,
    );
    let sub = schema(
        None,
        vec![],
        vec![("Wrap", other_names)],
        vec![],
        &registries,
    );
    let failure =
        check(&sub, &sup, &registries).expect_err("a differently-named parameter must fail");
    assert!(
        failure.render_fragment().contains("parameters {Param0}"),
        "expected the failure to name the declared parameter set, got {}",
        failure.render_fragment(),
    );
}

// --- join -----------------------------------------------------------------------------

/// The joined schema must be an upper bound: both operands `sig_subtype` it.
#[track_caller]
fn assert_upper_bound(
    a: &SigSchema,
    b: &SigSchema,
    joined: &SigSchema,
    registries: &RunRegistries,
) {
    assert!(
        check(a, joined, registries).is_ok(),
        "the left operand must satisfy the join"
    );
    assert!(
        check(b, joined, registries).is_ok(),
        "the right operand must satisfy the join"
    );
}

/// A member both operands fix to the same type survives manifest — the join keeps every
/// requirement both operands already meet.
#[test]
fn join_keeps_an_equal_manifest_member_manifest() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let a = schema(
        None,
        vec![],
        vec![("Tag", KType::NUMBER)],
        vec![("v", KType::NUMBER)],
        &registries,
    );
    let b = schema(
        None,
        vec![],
        vec![("Tag", KType::NUMBER)],
        vec![("v", KType::NUMBER)],
        &registries,
    );
    let joined = join_schemas(&a, &b, types);
    assert_eq!(
        joined.manifest_members.get(&type_name("Tag", &registries)),
        Some(&KType::NUMBER)
    );
    assert!(joined.abstract_members.is_empty());
    assert_eq!(joined.sig_id, None, "no abstract member, nothing to bind");
    assert_upper_bound(&a, &b, &joined, &registries);
}

/// Two differing manifest bindings at the same kind demote to an abstract member, and a value slot
/// typed by each operand's binding generalizes to a reference to it rather than coarsening to
/// `Any`. The joined interface is exactly what a `SIG` declaring `(TYPE Carrier) (VAL item
/// :Carrier)` asks for, so both operands satisfy it — and so does that SIG's own schema.
#[test]
fn join_demotes_a_differing_manifest_member_and_generalizes_its_slots() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let a = schema(
        None,
        vec![],
        vec![("Carrier", KType::NUMBER)],
        vec![("item", KType::NUMBER)],
        &registries,
    );
    let b = schema(
        None,
        vec![],
        vec![("Carrier", KType::STR)],
        vec![("item", KType::STR)],
        &registries,
    );
    let joined = join_schemas(&a, &b, types);
    let carrier = joined
        .abstract_members
        .get(&type_name("Carrier", &registries))
        .copied()
        .expect("a differing manifest member demotes to abstract");
    assert_eq!(joined.sig_id, Some(ScopeId::SENTINEL));
    assert!(joined.manifest_members.is_empty());
    assert_eq!(
        joined.value_slots.get(&value_name("item", &registries)),
        Some(&carrier),
        "the slot rejoins as a reference to the demoted member"
    );
    assert_upper_bound(&a, &b, &joined, &registries);

    // And the joined content is the interface an equivalent SIG declaration carries — the
    // canonical binder and nonce-free members are what make the two one type.
    let ordered = schema(
        Some(ScopeId::SENTINEL),
        vec![(
            "Carrier",
            sig_abstract(ScopeId::SENTINEL, "Carrier", &registries),
        )],
        vec![],
        vec![(
            "item",
            sig_abstract(ScopeId::SENTINEL, "Carrier", &registries),
        )],
        &registries,
    );
    assert_eq!(
        types.signature(joined.clone()),
        types.signature(ordered),
        "a joined signature is the handle the equivalent SIG declaration projects to"
    );
}

/// Kind is a requirement of its own: a first-order binding against a constructor binding has no
/// common requirement, so the member drops rather than demoting. Two constructors agree only over
/// the same parameter-name set.
#[test]
fn join_drops_a_member_whose_kinds_disagree() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let one_param = ctor("Boxed", 1, &registries);
    let first_order = schema(
        None,
        vec![],
        vec![("Wrap", KType::NUMBER)],
        vec![],
        &registries,
    );
    let constructor = schema(None, vec![], vec![("Wrap", one_param)], vec![], &registries);
    assert!(
        join_schemas(&first_order, &constructor, types)
            .abstract_members
            .is_empty(),
        "a proper type and a constructor share no requirement"
    );

    // Equal parameter-name sets keep the member, at constructor kind.
    let other = ctor("Wrapper", 1, &registries);
    let joined = join_schemas(
        &constructor,
        &schema(None, vec![], vec![("Wrap", other)], vec![], &registries),
        types,
    );
    let demoted = joined
        .abstract_members
        .get(&type_name("Wrap", &registries))
        .copied()
        .expect("same parameter names keep the member");
    assert_eq!(
        constructor_param_names(demoted, types),
        Some(params(1, &registries)),
        "the demoted member stays at the shared constructor kind"
    );

    // Unequal parameter-name sets do not.
    let two_params = ctor("Pair", 2, &registries);
    assert!(
        join_schemas(
            &constructor,
            &schema(
                None,
                vec![],
                vec![("Wrap", two_params)],
                vec![],
                &registries
            ),
            types,
        )
        .abstract_members
        .is_empty()
    );
}

/// Width intersection with nothing in common is the empty interface — the module-lattice top
/// `:Module` — not `Any`.
#[test]
fn join_of_disjoint_signatures_is_the_empty_interface() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let a = schema(
        None,
        vec![],
        vec![("Tag", KType::NUMBER)],
        vec![("left", KType::NUMBER)],
        &registries,
    );
    let b = schema(
        None,
        vec![],
        vec![("Other", KType::STR)],
        vec![("right", KType::STR)],
        &registries,
    );
    let joined = types.signature(join_schemas(&a, &b, types));
    assert_eq!(joined, KType::EMPTY_SIGNATURE);
    assert_eq!(types.join(types.signature(a), types.signature(b)), joined);
}

/// The registry arm: a signature joined with itself is itself, and with a non-signature it
/// coarsens to `Any` as any unrelated pair does.
#[test]
fn join_through_the_registry_arm() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let sig = types.signature(schema(
        None,
        vec![],
        vec![("Tag", KType::NUMBER)],
        vec![],
        &registries,
    ));
    assert_eq!(types.join(sig, sig), sig);
    assert_eq!(types.join(sig, KType::NUMBER), KType::ANY);
}

/// A function-typed slot generalizes through the demoted member in both variances: the parameters
/// are contravariant, so a pair the operands bind the member to is still that member there.
#[test]
fn join_generalizes_a_function_slot_through_its_parameters() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let compare =
        |element: KType| fn_type(vec![("a", element), ("b", element)], KType::BOOL, types);
    let a = schema(
        None,
        vec![],
        vec![("Carrier", KType::NUMBER)],
        vec![("compare", compare(KType::NUMBER))],
        &registries,
    );
    let b = schema(
        None,
        vec![],
        vec![("Carrier", KType::STR)],
        vec![("compare", compare(KType::STR))],
        &registries,
    );
    let joined = join_schemas(&a, &b, types);
    let carrier = joined
        .abstract_members
        .get(&type_name("Carrier", &registries))
        .copied()
        .expect("the member demotes");
    assert_eq!(
        joined.value_slots.get(&value_name("compare", &registries)),
        Some(&compare(carrier)),
        "both parameter positions and the return generalize"
    );
    assert_upper_bound(&a, &b, &joined, &registries);
}

/// A parameter position that does *not* generalize meets rather than joining: widening it would
/// claim a satisfying module accepts arguments neither operand does. The joined slot bottoms out
/// at `Never`, which is the true — and useless — bound.
#[test]
fn join_meets_an_ungeneralizable_function_parameter() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let a = schema(
        None,
        vec![],
        vec![],
        vec![(
            "apply",
            fn_type(vec![("x", KType::NUMBER)], KType::BOOL, types),
        )],
        &registries,
    );
    let b = schema(
        None,
        vec![],
        vec![],
        vec![(
            "apply",
            fn_type(vec![("x", KType::STR)], KType::BOOL, types),
        )],
        &registries,
    );
    let joined = join_schemas(&a, &b, types);
    assert_eq!(
        joined.value_slots.get(&value_name("apply", &registries)),
        Some(&fn_type(vec![("x", KType::NEVER)], KType::BOOL, types))
    );
    assert_upper_bound(&a, &b, &joined, &registries);
}
