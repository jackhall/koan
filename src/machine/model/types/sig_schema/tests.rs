//! Unit tests for the signature-subtyping relation, its schema, and abstract-member
//! substitution. Schemas are built both directly (owned `KType` handles) and by projecting parsed
//! SIG declarations, pinned via [`SigSchema::fold_pins`].

use std::collections::HashMap;

use super::*;
use crate::machine::core::ScopeId;
use crate::machine::model::types::{Record, RecursiveGroupWindow, RelativeSchema};

// --- region-free builders -------------------------------------------------------------

/// The parameter names a constructor of the given arity declares, shared by the declared-family
/// and abstract-slot builders so a sub binding and a sup slot agree by name.
fn params(arity: usize) -> Vec<String> {
    (0..arity).map(|i| format!("Param{i}")).collect()
}

/// A declared constructor family: a `TypeConstructor`-kind sealed member of the given arity.
fn ctor(name: &str, arity: usize, types: &TypeRegistry) -> KType {
    RecursiveGroupWindow::seal_singleton(
        name.into(),
        RelativeSchema::TypeConstructor {
            schema: HashMap::new(),
            param_names: params(arity),
        },
        None,
        types,
    )
}

/// A SIG's first-order abstract member.
fn sig_abstract(id: ScopeId, name: &str, types: &TypeRegistry) -> KType {
    types.intern(TypeNode::AbstractType {
        source: id,
        name: name.into(),
        param_names: Vec::new(),
        nonce: None,
    })
}

/// A SIG's higher-kinded abstract member, over `arity` parameters named as [`params`] names them.
fn sig_abstract_ctor(id: ScopeId, name: &str, arity: usize, types: &TypeRegistry) -> KType {
    types.intern(TypeNode::AbstractType {
        source: id,
        name: name.into(),
        param_names: params(arity),
        nonce: None,
    })
}

fn fn_type(params: Vec<(&str, KType)>, ret: KType, types: &TypeRegistry) -> KType {
    types.function_type(
        Record::from_pairs(params.into_iter().map(|(n, t)| (n.to_string(), t))),
        ret,
    )
}

fn schema(
    sig_id: Option<ScopeId>,
    abstract_members: Vec<(&str, KType)>,
    manifest_members: Vec<(&str, KType)>,
    value_slots: Vec<(&str, KType)>,
) -> SigSchema {
    SigSchema {
        sig_id,
        abstract_members: abstract_members
            .into_iter()
            .map(|(n, k)| (n.to_string(), k))
            .collect(),
        manifest_members: manifest_members
            .into_iter()
            .map(|(n, k)| (n.to_string(), k))
            .collect(),
        value_slots: value_slots
            .into_iter()
            .map(|(n, k)| (n.to_string(), k))
            .collect(),
    }
}

const SUP_ID: ScopeId = ScopeId::from_raw(0, 0xDEAD);
const REAL_ID: ScopeId = ScopeId::from_raw(0, 0xC0DE);

use super::sig_subtype as relation;

/// Run the relation against a caller-supplied registry and unbox the failure so `matches!` can
/// name the variant directly. The schemas' interned members and the relation walk must share one
/// registry, so every test that builds abstract or constructor members threads its own `types`.
#[allow(clippy::result_large_err)] // test ergonomics: unbox so assertions name the variant
fn check(sub: &SigSchema, sup: &SigSchema, types: &TypeRegistry) -> Result<(), SigSubtypeFailure> {
    relation(sub, sup, types).map_err(|e| *e)
}

// --- width ----------------------------------------------------------------------------

#[test]
fn width_extra_members_and_slots_still_subtype() {
    let types = TypeRegistry::new();
    let sup = schema(
        None,
        vec![],
        vec![("Tag", KType::NUMBER)],
        vec![("v", KType::STR)],
    );
    let sub = schema(
        None,
        vec![],
        vec![("Tag", KType::NUMBER), ("Extra", KType::BOOL)],
        vec![("v", KType::STR), ("w", KType::NUMBER)],
    );
    assert!(check(&sub, &sup, &types).is_ok());
}

// --- abstract members: first-order ----------------------------------------------------

#[test]
fn abstract_fo_satisfied_by_manifest_and_by_abstract() {
    let types = TypeRegistry::new();
    let sup = schema(
        Some(SUP_ID),
        vec![("Elt", sig_abstract(SUP_ID, "Elt", &types))],
        vec![],
        vec![],
    );
    // Sub supplies `Elt` as a manifest non-constructor.
    let sub_manifest = schema(None, vec![], vec![("Elt", KType::NUMBER)], vec![]);
    assert!(check(&sub_manifest, &sup, &types).is_ok());
    // Sub supplies `Elt` as its own first-order abstract member.
    let sub_abstract = schema(
        Some(REAL_ID),
        vec![("Elt", sig_abstract(REAL_ID, "Elt", &types))],
        vec![],
        vec![],
    );
    assert!(check(&sub_abstract, &sup, &types).is_ok());
}

#[test]
fn abstract_fo_refused_by_constructor() {
    let types = TypeRegistry::new();
    let sup = schema(
        Some(SUP_ID),
        vec![("Elt", sig_abstract(SUP_ID, "Elt", &types))],
        vec![],
        vec![],
    );
    let sub = schema(None, vec![], vec![("Elt", ctor("Elt", 1, &types))], vec![]);
    assert!(matches!(
        check(&sub, &sup, &types),
        Err(SigSubtypeFailure::KindMismatch {
            expected_params: None,
            ..
        })
    ));
}

#[test]
fn abstract_member_missing_fails() {
    let types = TypeRegistry::new();
    let sup = schema(
        Some(SUP_ID),
        vec![("Elt", sig_abstract(SUP_ID, "Elt", &types))],
        vec![],
        vec![],
    );
    let sub = schema(None, vec![], vec![], vec![]);
    assert!(matches!(
        check(&sub, &sup, &types),
        Err(SigSubtypeFailure::MissingTypeMember { .. })
    ));
}

// --- abstract members: higher-kinded --------------------------------------------------

#[test]
fn abstract_hk_arity_one_satisfied_by_matching_constructor() {
    let types = TypeRegistry::new();
    let sup = schema(
        Some(SUP_ID),
        vec![("Wrap", sig_abstract_ctor(SUP_ID, "Wrap", 1, &types))],
        vec![],
        vec![],
    );
    let sub = schema(
        None,
        vec![],
        vec![("Wrap", ctor("MyWrap", 1, &types))],
        vec![],
    );
    assert!(check(&sub, &sup, &types).is_ok());
}

#[test]
fn abstract_hk_refused_by_proper_type_by_wrong_arity_and_by_abstract_fo() {
    let types = TypeRegistry::new();
    let sup = schema(
        Some(SUP_ID),
        vec![("Wrap", sig_abstract_ctor(SUP_ID, "Wrap", 1, &types))],
        vec![],
        vec![],
    );
    // A proper type has no arity.
    let by_proper = schema(None, vec![], vec![("Wrap", KType::NUMBER)], vec![]);
    assert!(matches!(
        check(&by_proper, &sup, &types),
        Err(SigSubtypeFailure::KindMismatch {
            expected_params: Some(_),
            ..
        })
    ));
    // An arity-2 constructor cannot fill an arity-1 slot.
    let by_arity2 = schema(
        None,
        vec![],
        vec![("Wrap", ctor("Pair", 2, &types))],
        vec![],
    );
    assert!(matches!(
        check(&by_arity2, &sup, &types),
        Err(SigSubtypeFailure::KindMismatch {
            expected_params: Some(_),
            ..
        })
    ));
    // A first-order abstract member is not a constructor.
    let by_fo = schema(
        Some(REAL_ID),
        vec![("Wrap", sig_abstract(REAL_ID, "Wrap", &types))],
        vec![],
        vec![],
    );
    assert!(matches!(
        check(&by_fo, &sup, &types),
        Err(SigSubtypeFailure::KindMismatch {
            expected_params: Some(_),
            ..
        })
    ));
}

// --- manifest members -----------------------------------------------------------------

#[test]
fn manifest_equal_passes_unequal_and_missing_fail() {
    let types = TypeRegistry::new();
    let sup = schema(None, vec![], vec![("Tag", KType::NUMBER)], vec![]);
    assert!(check(
        &schema(None, vec![], vec![("Tag", KType::NUMBER)], vec![]),
        &sup,
        &types
    )
    .is_ok());
    assert!(matches!(
        check(
            &schema(None, vec![], vec![("Tag", KType::STR)], vec![]),
            &sup,
            &types
        ),
        Err(SigSubtypeFailure::ManifestMismatch { .. })
    ));
    assert!(matches!(
        check(&schema(None, vec![], vec![], vec![]), &sup, &types),
        Err(SigSubtypeFailure::MissingTypeMember { .. })
    ));
}

#[test]
fn manifest_requirement_refuses_abstract_sub_member() {
    let types = TypeRegistry::new();
    let sup = schema(None, vec![], vec![("Tag", KType::NUMBER)], vec![]);
    let sub = schema(
        Some(REAL_ID),
        vec![("Tag", sig_abstract(REAL_ID, "Tag", &types))],
        vec![],
        vec![],
    );
    assert!(matches!(
        check(&sub, &sup, &types),
        Err(SigSubtypeFailure::ManifestMismatch { .. })
    ));
}

// --- value slots: covariance ----------------------------------------------------------

#[test]
fn value_slot_covariant_depth() {
    let types = TypeRegistry::new();
    // A slot declared `-> Any` is filled by a member `-> Number`; the reverse fails.
    let sup_any = schema(
        None,
        vec![],
        vec![],
        vec![("f", fn_type(vec![], KType::ANY, &types))],
    );
    let sub_number = schema(
        None,
        vec![],
        vec![],
        vec![("f", fn_type(vec![], KType::NUMBER, &types))],
    );
    assert!(check(&sub_number, &sup_any, &types).is_ok());

    let sup_number = schema(
        None,
        vec![],
        vec![],
        vec![("f", fn_type(vec![], KType::NUMBER, &types))],
    );
    let sub_any = schema(
        None,
        vec![],
        vec![],
        vec![("f", fn_type(vec![], KType::ANY, &types))],
    );
    assert!(matches!(
        check(&sub_any, &sup_number, &types),
        Err(SigSubtypeFailure::ValueSlotMismatch { .. })
    ));
}

#[test]
fn value_slot_equal_passes_missing_fails() {
    let types = TypeRegistry::new();
    let sup = schema(None, vec![], vec![], vec![("v", KType::NUMBER)]);
    assert!(check(
        &schema(None, vec![], vec![], vec![("v", KType::NUMBER)]),
        &sup,
        &types
    )
    .is_ok());
    assert!(matches!(
        check(&schema(None, vec![], vec![], vec![]), &sup, &types),
        Err(SigSubtypeFailure::MissingValueSlot { .. })
    ));
}

// --- substitution through value-slot types --------------------------------------------

#[test]
fn value_slot_abstract_ref_substitutes_to_sub_manifest() {
    let types = TypeRegistry::new();
    // Super: abstract `Type`, slot `compare :(FN (x :Type, y :Type) -> Number)`.
    let sup = schema(
        Some(SUP_ID),
        vec![("Type", sig_abstract(SUP_ID, "Type", &types))],
        vec![],
        vec![(
            "compare",
            fn_type(
                vec![
                    ("x", sig_abstract(SUP_ID, "Type", &types)),
                    ("y", sig_abstract(SUP_ID, "Type", &types)),
                ],
                KType::NUMBER,
                &types,
            ),
        )],
    );
    // Sub: manifest `Type = Number`, slot `compare :(FN (x :Number, y :Number) -> Number)`.
    let sub = schema(
        None,
        vec![],
        vec![("Type", KType::NUMBER)],
        vec![(
            "compare",
            fn_type(
                vec![("x", KType::NUMBER), ("y", KType::NUMBER)],
                KType::NUMBER,
                &types,
            ),
        )],
    );
    assert!(check(&sub, &sup, &types).is_ok());
}

#[test]
fn value_slot_list_of_abstract_ref_substitutes_nested() {
    let types = TypeRegistry::new();
    // Super: abstract `Type`, slot `items :(LIST OF Type)` — the substitution point sits
    // *nested* inside a container, so the walk must descend the `List` before comparing.
    let sup = schema(
        Some(SUP_ID),
        vec![("Type", sig_abstract(SUP_ID, "Type", &types))],
        vec![],
        vec![("items", types.list(sig_abstract(SUP_ID, "Type", &types)))],
    );
    // Sub with `Type = Number` and `items :(LIST OF Number)` subtypes.
    let sub_ok = schema(
        None,
        vec![],
        vec![("Type", KType::NUMBER)],
        vec![("items", types.list(KType::NUMBER))],
    );
    assert!(check(&sub_ok, &sup, &types).is_ok());
    // `items :(LIST OF Str)` against `Type = Number` fails at the nested element compare.
    let sub_bad = schema(
        None,
        vec![],
        vec![("Type", KType::NUMBER)],
        vec![("items", types.list(KType::STR))],
    );
    assert!(matches!(
        check(&sub_bad, &sup, &types),
        Err(SigSubtypeFailure::ValueSlotMismatch { .. })
    ));
}

// --- pins via fold_pins ----------------------------------------------------------------

#[test]
fn pin_converts_abstract_to_manifest_via_parsed_sig() {
    use crate::builtins::test_support::TestRun;
    use crate::machine::core::run_root_storage;

    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run("SIG Pinnable = ((TYPE Elt) (VAL v :Number))");
    let sig_schema = match scope.resolve_type("Pinnable") {
        Some(kt) => match test_run.types.node(kt) {
            TypeNode::Signature { schema, .. } => schema,
            _ => panic!("Pinnable should resolve to a signature"),
        },
        _ => panic!("Pinnable should resolve to a signature"),
    };
    // `S WITH {Elt = Number}` fixes the abstract member manifest.
    let pinned = sig_schema.fold_pins(&[("Elt".to_string(), KType::NUMBER)], &test_run.types);
    assert!(pinned.abstract_members.is_empty());
    assert_eq!(pinned.manifest_members.get("Elt"), Some(&KType::NUMBER));

    let elt_str = schema(
        None,
        vec![],
        vec![("Elt", KType::STR)],
        vec![("v", KType::NUMBER)],
    );
    assert!(matches!(
        check(&elt_str, &pinned, &test_run.types),
        Err(SigSubtypeFailure::ManifestMismatch { .. })
    ));
    let elt_number = schema(
        None,
        vec![],
        vec![("Elt", KType::NUMBER)],
        vec![("v", KType::NUMBER)],
    );
    assert!(check(&elt_number, &pinned, &test_run.types).is_ok());
}

#[test]
fn sig_to_sig_entailment_over_shared_abstract() {
    use crate::builtins::test_support::TestRun;
    use crate::machine::core::run_root_storage;

    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run(
        "SIG Alpha = ((TYPE Elem) (VAL compare :(FN (x :Elem) -> Number)))\n\
         SIG Beta = ((TYPE Elem) (VAL compare :(FN (x :Elem) -> Number)))",
    );
    let a = match scope.resolve_type("Alpha") {
        Some(kt) => match test_run.types.node(kt) {
            TypeNode::Signature { schema, .. } => schema,
            _ => panic!("Alpha should resolve to a signature"),
        },
        _ => panic!("Alpha should resolve to a signature"),
    };
    let b = match scope.resolve_type("Beta") {
        Some(kt) => match test_run.types.node(kt) {
            TypeNode::Signature { schema, .. } => schema,
            _ => panic!("Beta should resolve to a signature"),
        },
        _ => panic!("Beta should resolve to a signature"),
    };
    // Two SIGs declaring the same abstract member and slot entail each other: the
    // substitution maps each super `Type` ref onto the sub's own abstract identity.
    assert!(check(&a, &b, &test_run.types).is_ok());
    assert!(check(&b, &a, &test_run.types).is_ok());
}

// --- substitute_sig_members units -----------------------------------------------------

/// An opaque ascription's generative mint shares its declaring binder's `source` and name — only
/// the nonce separates them — and must not be mistaken for a reference to that declaration.
#[test]
fn substitute_leaves_a_generative_mint_alone() {
    let types = TypeRegistry::new();
    let mut map: HashMap<String, KType> = HashMap::new();
    map.insert("Type".into(), KType::NUMBER);

    let mint = types.intern(TypeNode::AbstractType {
        source: SUP_ID,
        name: "Type".into(),
        param_names: Vec::new(),
        nonce: Some(ScopeId::from_raw(0, 0xBEEF)),
    });
    assert_eq!(substitute_sig_members(mint, SUP_ID, &map, &types), mint);
    // The declaration it was minted from still substitutes.
    assert_eq!(
        substitute_sig_members(sig_abstract(SUP_ID, "Type", &types), SUP_ID, &map, &types),
        KType::NUMBER
    );
}

#[test]
fn substitute_top_level_and_nested() {
    let types = TypeRegistry::new();
    let mut map: HashMap<String, KType> = HashMap::new();
    map.insert("Type".into(), KType::NUMBER);

    // Top level.
    assert_eq!(
        substitute_sig_members(sig_abstract(SUP_ID, "Type", &types), SUP_ID, &map, &types),
        KType::NUMBER
    );
    // Inside KFunction params and ret.
    let f = fn_type(
        vec![("x", sig_abstract(SUP_ID, "Type", &types))],
        sig_abstract(SUP_ID, "Type", &types),
        &types,
    );
    assert_eq!(
        substitute_sig_members(f, SUP_ID, &map, &types),
        fn_type(vec![("x", KType::NUMBER)], KType::NUMBER, &types)
    );
    // Inside List, Record, Union.
    assert_eq!(
        substitute_sig_members(
            types.list(sig_abstract(SUP_ID, "Type", &types)),
            SUP_ID,
            &map,
            &types
        ),
        types.list(KType::NUMBER)
    );
    let rec = types.record(Record::from_pairs([(
        "f".to_string(),
        sig_abstract(SUP_ID, "Type", &types),
    )]));
    assert_eq!(
        substitute_sig_members(rec, SUP_ID, &map, &types),
        types.record(Record::from_pairs([("f".to_string(), KType::NUMBER)]))
    );
    let union = types.union_of(vec![sig_abstract(SUP_ID, "Type", &types), KType::STR]);
    assert_eq!(
        substitute_sig_members(union, SUP_ID, &map, &types),
        types.union_of(vec![KType::NUMBER, KType::STR])
    );
}

#[test]
fn substitute_constructor_apply_abstract_ctor_position() {
    let types = TypeRegistry::new();
    let mut map: HashMap<String, KType> = HashMap::new();
    let real = ctor("MyWrap", 1, &types);
    map.insert("Wrap".into(), real);
    let applied = types.constructor_apply(
        sig_abstract_ctor(SUP_ID, "Wrap", 1, &types),
        Record::from_pairs([("Type".to_string(), KType::NUMBER)]),
    );
    assert_eq!(
        substitute_sig_members(applied, SUP_ID, &map, &types),
        types.constructor_apply(
            real,
            Record::from_pairs([("Type".to_string(), KType::NUMBER)])
        )
    );
}

#[test]
fn substitute_leaves_non_matching_sig_id_and_unknown_names() {
    let types = TypeRegistry::new();
    let map: HashMap<String, KType> = HashMap::new();
    // Unknown name — untouched even at the matching sig_id.
    let unknown = sig_abstract(SUP_ID, "Other", &types);
    assert_eq!(
        substitute_sig_members(unknown, SUP_ID, &map, &types),
        unknown
    );
    // Non-matching sig_id — untouched.
    let mut with_type: HashMap<String, KType> = HashMap::new();
    with_type.insert("Type".into(), KType::NUMBER);
    let other_sig = sig_abstract(SUP_ID, "Type", &types);
    assert_eq!(
        substitute_sig_members(other_sig, REAL_ID, &with_type, &types),
        other_sig
    );
}

#[test]
fn constructor_param_names_probe() {
    let types = TypeRegistry::new();
    assert_eq!(
        constructor_param_names(sig_abstract_ctor(SUP_ID, "Wrap", 1, &types), &types),
        Some(params(1)),
    );
    assert_eq!(
        constructor_param_names(ctor("Wrap", 2, &types), &types),
        Some(params(2)),
    );
    assert_eq!(constructor_param_names(KType::NUMBER, &types), None);
    assert_eq!(
        constructor_param_names(sig_abstract(SUP_ID, "Elt", &types), &types),
        None
    );
}

/// Parameter names are interface: a family declaring a differently-named parameter does not
/// supply the slot, and the failure names the expected set.
#[test]
fn abstract_hk_refused_by_differently_named_parameter() {
    let types = TypeRegistry::new();
    let sup = schema(
        Some(SUP_ID),
        vec![("Wrap", sig_abstract_ctor(SUP_ID, "Wrap", 1, &types))],
        vec![],
        vec![],
    );
    let other_names = RecursiveGroupWindow::seal_singleton(
        "MyWrap".into(),
        RelativeSchema::TypeConstructor {
            schema: HashMap::new(),
            param_names: vec!["Item".to_string()],
        },
        None,
        &types,
    );
    let sub = schema(None, vec![], vec![("Wrap", other_names)], vec![]);
    let failure = check(&sub, &sup, &types).expect_err("a differently-named parameter must fail");
    assert!(
        failure.render_fragment().contains("parameters {Param0}"),
        "expected the failure to name the declared parameter set, got {}",
        failure.render_fragment(),
    );
}
