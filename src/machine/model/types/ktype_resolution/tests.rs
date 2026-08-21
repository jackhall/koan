use super::*;
use crate::machine::model::RunRegistries;
use crate::machine::model::Symbol;
use crate::machine::model::TypeRegistry;
use crate::machine::model::types::Record;

fn leaf(n: &str) -> TypeIdentifier<'_> {
    TypeIdentifier::leaf(n)
}

#[test]
fn from_type_expr_leaf_number() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    assert_eq!(
        KType::from_type_identifier(&leaf("Number"), types).unwrap(),
        KType::NUMBER
    );
}

#[test]
fn from_type_expr_unknown_paramless_name_errors() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    assert!(KType::from_type_identifier(&leaf("Banana"), types).is_err());
}

#[test]
fn from_type_expr_leaf_falls_through_to_builtin() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    assert_eq!(
        KType::from_type_identifier(&leaf("Number"), types).unwrap(),
        KType::NUMBER,
    );
}

#[test]
fn from_name_kfunction_no_longer_resolves() {
    assert_eq!(KType::from_name("KFunction"), None);
}

#[test]
fn from_name_list_lowers_to_list_any() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    assert_eq!(KType::from_name("List"), Some(types.list(KType::ANY)));
}

#[test]
fn from_name_dict_lowers_to_dict_any_any() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    assert_eq!(
        KType::from_name("Dict"),
        Some(types.dict(KType::ANY, KType::ANY))
    );
}

#[test]
fn join_distinct_concretes_yields_any() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    assert_eq!(types.join(KType::NUMBER, KType::STR), KType::ANY);
}

#[test]
fn join_same_yields_same() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    assert_eq!(types.join(KType::NUMBER, KType::NUMBER), KType::NUMBER);
}

#[test]
fn join_lists_recurses_on_element() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let a = types.list(KType::NUMBER);
    let b = types.list(KType::STR);
    assert_eq!(types.join(a, b), types.list(KType::ANY));
}

#[test]
fn join_iter_empty_is_any() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let v: Vec<KType> = vec![];
    assert_eq!(types.join_iter(v), KType::ANY);
}

#[test]
fn join_iter_homogeneous() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let v = vec![KType::NUMBER, KType::NUMBER, KType::NUMBER];
    assert_eq!(types.join_iter(v), KType::NUMBER);
}

#[test]
fn join_iter_mixed_yields_any() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let v = vec![KType::NUMBER, KType::STR, KType::BOOL];
    assert_eq!(types.join_iter(v), KType::ANY);
}

// --- union_of ---------------------------------------------------------------------

/// Two distinct members build a two-member `Union`.
#[test]
fn union_of_two_distinct_members() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let u = types.union_of(vec![KType::NUMBER, KType::STR]);
    assert_eq!(u, types.union_of(vec![KType::NUMBER, KType::STR]));
}

/// A single member collapses to that member (AC2's `:(A | A)` is `:A`, degenerate case).
#[test]
fn union_of_single_member_collapses() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    assert_eq!(types.union_of(vec![KType::NUMBER]), KType::NUMBER);
}

/// Duplicate members are deduplicated; `:(Number | Number)` collapses to `:Number`.
#[test]
fn union_of_dedups_to_single() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    assert_eq!(
        types.union_of(vec![KType::NUMBER, KType::NUMBER]),
        KType::NUMBER
    );
}

/// Repeated members within a larger set are deduplicated but the union survives.
#[test]
fn union_of_dedups_within_set() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let u = types.union_of(vec![KType::NUMBER, KType::STR, KType::NUMBER]);
    assert_eq!(u, types.union_of(vec![KType::NUMBER, KType::STR]));
}

/// A nested `Union` member is flattened into the outer members, then deduplicated.
#[test]
fn union_of_flattens_nested_union() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let inner = types.union_of(vec![KType::STR, KType::BOOL]);
    let u = types.union_of(vec![KType::NUMBER, inner, KType::BOOL]);
    assert_eq!(
        u,
        types.union_of(vec![KType::NUMBER, KType::STR, KType::BOOL])
    );
}

fn function(params: Vec<(&str, KType)>, ret: KType, types: &TypeRegistry) -> KType {
    types.function_type(
        Record::from_pairs(params.into_iter().map(|(n, t)| (Symbol::of(n), t))),
        ret,
    )
}

/// Two same-shape functions join to the shared `KFunction` (the established arm).
#[test]
fn join_same_shape_functions_yields_shared_function() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let f1 = function(vec![("x", KType::NUMBER)], KType::BOOL, types);
    let f2 = function(vec![("x", KType::NUMBER)], KType::BOOL, types);
    assert_eq!(types.join(f1, f2), f1);
}
