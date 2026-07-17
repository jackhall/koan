//! Unit tests for the signature-subtyping relation, its schema, and abstract-member
//! substitution. Schemas are built both directly (region-free `KType`s in `'static`) and by
//! projecting parsed SIG declarations through [`SigSchema::of_sig`].

use std::collections::HashMap;

use super::*;
use crate::machine::core::ScopeId;
use crate::machine::model::types::{NominalSchema, Record, RecursiveSet, SigSource};

// --- region-free builders -------------------------------------------------------------

/// A `TypeConstructor`-kind `SetRef` of the given arity. `ScopeId::SENTINEL` marks a SIG's
/// higher-kinded abstract slot; any other id marks a real constructor. Lifetime-generic (the
/// value is region-free) so each call site infers the lifetime it needs — `KType<'a>` is
/// invariant, so a `'static` value cannot be reused where a region lifetime is expected.
fn ctor<'a>(name: &str, arity: usize, scope_id: ScopeId) -> KType<'a> {
    let set = RecursiveSet::singleton(
        name.into(),
        scope_id,
        NominalSchema::TypeConstructor {
            schema: HashMap::new(),
            param_names: (0..arity).map(|i| format!("Param{i}")).collect(),
        },
    );
    KType::SetRef { set, index: 0 }
}

fn sig_abstract<'a>(id: ScopeId, name: &str) -> KType<'a> {
    KType::AbstractType {
        source: id,
        name: name.into(),
    }
}

fn fn_type<'a>(params: Vec<(&str, KType<'a>)>, ret: KType<'a>) -> KType<'a> {
    KType::function_type(
        Record::from_pairs(params.into_iter().map(|(n, t)| (n.to_string(), t))),
        Box::new(ret),
    )
}

fn schema<'a>(
    sig_id: Option<ScopeId>,
    abstract_members: Vec<(&str, KType<'a>, Option<usize>)>,
    manifest_members: Vec<(&str, KType<'a>)>,
    value_slots: Vec<(&str, KType<'a>)>,
) -> SigSchema<'a> {
    SigSchema {
        sig_id,
        abstract_members: abstract_members
            .into_iter()
            .map(|(n, k, a)| (n.to_string(), (k, a)))
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

/// Run the relation and unbox the failure so `matches!` can name the variant directly.
#[allow(clippy::result_large_err)] // test ergonomics: unbox so assertions name the variant
fn check<'s, 'p>(sub: &SigSchema<'s>, sup: &SigSchema<'p>) -> Result<(), SigSubtypeFailure> {
    relation(sub, sup).map_err(|e| *e)
}

// --- width ----------------------------------------------------------------------------

#[test]
fn width_extra_members_and_slots_still_subtype() {
    let sup = schema(
        None,
        vec![],
        vec![("Tag", KType::Number)],
        vec![("v", KType::Str)],
    );
    let sub = schema(
        None,
        vec![],
        vec![("Tag", KType::Number), ("Extra", KType::Bool)],
        vec![("v", KType::Str), ("w", KType::Number)],
    );
    assert!(check(&sub, &sup).is_ok());
}

// --- abstract members: first-order ----------------------------------------------------

#[test]
fn abstract_fo_satisfied_by_manifest_and_by_abstract() {
    let sup = schema(
        Some(SUP_ID),
        vec![("Elt", sig_abstract(SUP_ID, "Elt"), None)],
        vec![],
        vec![],
    );
    // Sub supplies `Elt` as a manifest non-constructor.
    let sub_manifest = schema(None, vec![], vec![("Elt", KType::Number)], vec![]);
    assert!(check(&sub_manifest, &sup).is_ok());
    // Sub supplies `Elt` as its own first-order abstract member.
    let sub_abstract = schema(
        Some(REAL_ID),
        vec![("Elt", sig_abstract(REAL_ID, "Elt"), None)],
        vec![],
        vec![],
    );
    assert!(check(&sub_abstract, &sup).is_ok());
}

#[test]
fn abstract_fo_refused_by_constructor() {
    let sup = schema(
        Some(SUP_ID),
        vec![("Elt", sig_abstract(SUP_ID, "Elt"), None)],
        vec![],
        vec![],
    );
    let sub = schema(None, vec![], vec![("Elt", ctor("Elt", 1, REAL_ID))], vec![]);
    assert!(matches!(
        check(&sub, &sup),
        Err(SigSubtypeFailure::KindMismatch {
            expected_arity: None,
            ..
        })
    ));
}

#[test]
fn abstract_member_missing_fails() {
    let sup = schema(
        Some(SUP_ID),
        vec![("Elt", sig_abstract(SUP_ID, "Elt"), None)],
        vec![],
        vec![],
    );
    let sub = schema(None, vec![], vec![], vec![]);
    assert!(matches!(
        check(&sub, &sup),
        Err(SigSubtypeFailure::MissingTypeMember { .. })
    ));
}

// --- abstract members: higher-kinded --------------------------------------------------

#[test]
fn abstract_hk_arity_one_satisfied_by_matching_constructor() {
    let sup = schema(
        Some(SUP_ID),
        vec![("Wrap", ctor("Wrap", 1, ScopeId::SENTINEL), Some(1))],
        vec![],
        vec![],
    );
    let sub = schema(
        None,
        vec![],
        vec![("Wrap", ctor("MyWrap", 1, REAL_ID))],
        vec![],
    );
    assert!(check(&sub, &sup).is_ok());
}

#[test]
fn abstract_hk_refused_by_proper_type_by_wrong_arity_and_by_abstract_fo() {
    let sup = schema(
        Some(SUP_ID),
        vec![("Wrap", ctor("Wrap", 1, ScopeId::SENTINEL), Some(1))],
        vec![],
        vec![],
    );
    // A proper type has no arity.
    let by_proper = schema(None, vec![], vec![("Wrap", KType::Number)], vec![]);
    assert!(matches!(
        check(&by_proper, &sup),
        Err(SigSubtypeFailure::KindMismatch {
            expected_arity: Some(1),
            ..
        })
    ));
    // An arity-2 constructor cannot fill an arity-1 slot.
    let by_arity2 = schema(
        None,
        vec![],
        vec![("Wrap", ctor("Pair", 2, REAL_ID))],
        vec![],
    );
    assert!(matches!(
        check(&by_arity2, &sup),
        Err(SigSubtypeFailure::KindMismatch {
            expected_arity: Some(1),
            ..
        })
    ));
    // A first-order abstract member is not a constructor.
    let by_fo = schema(
        Some(REAL_ID),
        vec![("Wrap", sig_abstract(REAL_ID, "Wrap"), None)],
        vec![],
        vec![],
    );
    assert!(matches!(
        check(&by_fo, &sup),
        Err(SigSubtypeFailure::KindMismatch {
            expected_arity: Some(1),
            ..
        })
    ));
}

// --- manifest members -----------------------------------------------------------------

#[test]
fn manifest_equal_passes_unequal_and_missing_fail() {
    let sup = schema(None, vec![], vec![("Tag", KType::Number)], vec![]);
    assert!(check(
        &schema(None, vec![], vec![("Tag", KType::Number)], vec![]),
        &sup
    )
    .is_ok());
    assert!(matches!(
        check(
            &schema(None, vec![], vec![("Tag", KType::Str)], vec![]),
            &sup
        ),
        Err(SigSubtypeFailure::ManifestMismatch { .. })
    ));
    assert!(matches!(
        check(&schema(None, vec![], vec![], vec![]), &sup),
        Err(SigSubtypeFailure::MissingTypeMember { .. })
    ));
}

#[test]
fn manifest_requirement_refuses_abstract_sub_member() {
    let sup = schema(None, vec![], vec![("Tag", KType::Number)], vec![]);
    let sub = schema(
        Some(REAL_ID),
        vec![("Tag", sig_abstract(REAL_ID, "Tag"), None)],
        vec![],
        vec![],
    );
    assert!(matches!(
        check(&sub, &sup),
        Err(SigSubtypeFailure::ManifestMismatch { .. })
    ));
}

// --- value slots: covariance ----------------------------------------------------------

#[test]
fn value_slot_covariant_depth() {
    // A slot declared `-> Any` is filled by a member `-> Number`; the reverse fails.
    let sup_any = schema(
        None,
        vec![],
        vec![],
        vec![("f", fn_type(vec![], KType::Any))],
    );
    let sub_number = schema(
        None,
        vec![],
        vec![],
        vec![("f", fn_type(vec![], KType::Number))],
    );
    assert!(check(&sub_number, &sup_any).is_ok());

    let sup_number = schema(
        None,
        vec![],
        vec![],
        vec![("f", fn_type(vec![], KType::Number))],
    );
    let sub_any = schema(
        None,
        vec![],
        vec![],
        vec![("f", fn_type(vec![], KType::Any))],
    );
    assert!(matches!(
        check(&sub_any, &sup_number),
        Err(SigSubtypeFailure::ValueSlotMismatch { .. })
    ));
}

#[test]
fn value_slot_equal_passes_missing_fails() {
    let sup = schema(None, vec![], vec![], vec![("v", KType::Number)]);
    assert!(check(
        &schema(None, vec![], vec![], vec![("v", KType::Number)]),
        &sup
    )
    .is_ok());
    assert!(matches!(
        check(&schema(None, vec![], vec![], vec![]), &sup),
        Err(SigSubtypeFailure::MissingValueSlot { .. })
    ));
}

// --- substitution through value-slot types --------------------------------------------

#[test]
fn value_slot_abstract_ref_substitutes_to_sub_manifest() {
    // Super: abstract `Type`, slot `compare :(FN (x :Type, y :Type) -> Number)`.
    let sup = schema(
        Some(SUP_ID),
        vec![("Type", sig_abstract(SUP_ID, "Type"), None)],
        vec![],
        vec![(
            "compare",
            fn_type(
                vec![
                    ("x", sig_abstract(SUP_ID, "Type")),
                    ("y", sig_abstract(SUP_ID, "Type")),
                ],
                KType::Number,
            ),
        )],
    );
    // Sub: manifest `Type = Number`, slot `compare :(FN (x :Number, y :Number) -> Number)`.
    let sub = schema(
        None,
        vec![],
        vec![("Type", KType::Number)],
        vec![(
            "compare",
            fn_type(
                vec![("x", KType::Number), ("y", KType::Number)],
                KType::Number,
            ),
        )],
    );
    assert!(check(&sub, &sup).is_ok());
}

#[test]
fn value_slot_list_of_abstract_ref_substitutes_nested() {
    // Super: abstract `Type`, slot `items :(LIST OF Type)` — the substitution point sits
    // *nested* inside a container, so the walk must descend the `List` before comparing.
    let sup = schema(
        Some(SUP_ID),
        vec![("Type", sig_abstract(SUP_ID, "Type"), None)],
        vec![],
        vec![("items", KType::list(Box::new(sig_abstract(SUP_ID, "Type"))))],
    );
    // Sub with `Type = Number` and `items :(LIST OF Number)` subtypes.
    let sub_ok = schema(
        None,
        vec![],
        vec![("Type", KType::Number)],
        vec![("items", KType::list(Box::new(KType::Number)))],
    );
    assert!(check(&sub_ok, &sup).is_ok());
    // `items :(LIST OF Str)` against `Type = Number` fails at the nested element compare.
    let sub_bad = schema(
        None,
        vec![],
        vec![("Type", KType::Number)],
        vec![("items", KType::list(Box::new(KType::Str)))],
    );
    assert!(matches!(
        check(&sub_bad, &sup),
        Err(SigSubtypeFailure::ValueSlotMismatch { .. })
    ));
}

#[test]
fn sig_subtype_runs_across_distinct_lifetimes() {
    // The relation takes `sub` and `sup` at independent lifetimes. Build one in a fresh
    // shorter-lived scope and the other in `'static` to prove the heterogeneous signature
    // is exercised (not just same-lifetime `check`).
    let sup: SigSchema<'static> = schema(
        Some(SUP_ID),
        vec![("Type", sig_abstract(SUP_ID, "Type"), None)],
        vec![],
        vec![("v", sig_abstract(SUP_ID, "Type"))],
    );
    let name = String::from("v");
    {
        let sub = schema(
            None,
            vec![],
            vec![("Type", KType::Number)],
            vec![(name.as_str(), KType::Number)],
        );
        assert!(sig_subtype(&sub, &sup).is_ok());
    }
}

// --- pins via of_sig ------------------------------------------------------------------

#[test]
fn pin_converts_abstract_to_manifest_via_parsed_sig() {
    use crate::builtins::test_support::{run, run_root_silent};
    use crate::machine::core::run_root_storage;

    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "SIG Pinnable = ((TYPE Elt) (VAL v :Number))");
    let s = match scope.resolve_type("Pinnable") {
        Some(KType::Signature {
            sig: SigSource::Declared(sig),
            ..
        }) => *sig,
        _ => panic!("Pinnable should resolve to a signature"),
    };
    // `S WITH {Elt = Number}` fixes the abstract member manifest.
    let pinned = SigSchema::of_sig(s, &[("Elt".to_string(), KType::Number)]);
    assert!(pinned.abstract_members.is_empty());
    assert_eq!(pinned.manifest_members.get("Elt"), Some(&KType::Number));

    let elt_str = schema(
        None,
        vec![],
        vec![("Elt", KType::Str)],
        vec![("v", KType::Number)],
    );
    assert!(matches!(
        check(&elt_str, &pinned),
        Err(SigSubtypeFailure::ManifestMismatch { .. })
    ));
    let elt_number = schema(
        None,
        vec![],
        vec![("Elt", KType::Number)],
        vec![("v", KType::Number)],
    );
    assert!(check(&elt_number, &pinned).is_ok());
}

#[test]
fn sig_to_sig_entailment_over_shared_abstract() {
    use crate::builtins::test_support::{run, run_root_silent};
    use crate::machine::core::run_root_storage;

    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG Alpha = ((TYPE Elem) (VAL compare :(FN (x :Elem) -> Number)))\n\
         SIG Beta = ((TYPE Elem) (VAL compare :(FN (x :Elem) -> Number)))",
    );
    let a = match scope.resolve_type("Alpha") {
        Some(KType::Signature {
            sig: SigSource::Declared(sig),
            ..
        }) => *sig,
        _ => panic!("Alpha should resolve to a signature"),
    };
    let b = match scope.resolve_type("Beta") {
        Some(KType::Signature {
            sig: SigSource::Declared(sig),
            ..
        }) => *sig,
        _ => panic!("Beta should resolve to a signature"),
    };
    // Two SIGs declaring the same abstract member and slot entail each other: the
    // substitution maps each super `Type` ref onto the sub's own abstract identity.
    assert!(check(&SigSchema::of_sig(a, &[]), &SigSchema::of_sig(b, &[])).is_ok());
    assert!(check(&SigSchema::of_sig(b, &[]), &SigSchema::of_sig(a, &[])).is_ok());
}

// --- substitute_sig_members units -----------------------------------------------------

#[test]
fn substitute_top_level_and_nested() {
    let mut map: HashMap<String, KType<'static>> = HashMap::new();
    map.insert("Type".into(), KType::Number);

    // Top level.
    assert_eq!(
        substitute_sig_members(&sig_abstract(SUP_ID, "Type"), SUP_ID, &map),
        KType::Number
    );
    // Inside KFunction params and ret.
    let f = fn_type(
        vec![("x", sig_abstract(SUP_ID, "Type"))],
        sig_abstract(SUP_ID, "Type"),
    );
    assert_eq!(
        substitute_sig_members(&f, SUP_ID, &map),
        fn_type(vec![("x", KType::Number)], KType::Number)
    );
    // Inside List, Record, Union.
    assert_eq!(
        substitute_sig_members(
            &KType::list(Box::new(sig_abstract(SUP_ID, "Type"))),
            SUP_ID,
            &map
        ),
        KType::list(Box::new(KType::Number))
    );
    let rec = KType::record(Box::new(Record::from_pairs([(
        "f".to_string(),
        sig_abstract(SUP_ID, "Type"),
    )])));
    assert_eq!(
        substitute_sig_members(&rec, SUP_ID, &map),
        KType::record(Box::new(Record::from_pairs([(
            "f".to_string(),
            KType::Number
        )])))
    );
    let union = KType::union_of(vec![sig_abstract(SUP_ID, "Type"), KType::Str]);
    assert_eq!(
        substitute_sig_members(&union, SUP_ID, &map),
        KType::union_of(vec![KType::Number, KType::Str])
    );
}

#[test]
fn substitute_constructor_apply_sentinel_ctor_position() {
    let mut map: HashMap<String, KType<'static>> = HashMap::new();
    let real = ctor("MyWrap", 1, REAL_ID);
    map.insert("Wrap".into(), real.clone());
    let applied = KType::constructor_apply(
        Box::new(ctor("Wrap", 1, ScopeId::SENTINEL)),
        vec![KType::Number],
    );
    assert_eq!(
        substitute_sig_members(&applied, SUP_ID, &map),
        KType::constructor_apply(Box::new(real), vec![KType::Number])
    );
}

#[test]
fn substitute_leaves_non_matching_sig_id_and_unknown_names() {
    let map: HashMap<String, KType<'static>> = HashMap::new();
    // Unknown name — untouched even at the matching sig_id.
    let unknown = sig_abstract(SUP_ID, "Other");
    assert_eq!(substitute_sig_members(&unknown, SUP_ID, &map), unknown);
    // Non-matching sig_id — untouched.
    let mut with_type: HashMap<String, KType<'static>> = HashMap::new();
    with_type.insert("Type".into(), KType::Number);
    let other_sig = sig_abstract(SUP_ID, "Type");
    assert_eq!(
        substitute_sig_members(&other_sig, REAL_ID, &with_type),
        other_sig
    );
}

#[test]
fn constructor_arity_probe() {
    assert_eq!(constructor_arity(&ctor("W", 1, ScopeId::SENTINEL)), Some(1));
    assert_eq!(constructor_arity(&ctor("W", 2, REAL_ID)), Some(2));
    assert_eq!(constructor_arity(&KType::Number), None);
    assert_eq!(constructor_arity(&sig_abstract(SUP_ID, "Elt")), None);
}
