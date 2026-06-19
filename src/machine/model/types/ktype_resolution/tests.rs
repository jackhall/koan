use super::*;

fn leaf(n: &str) -> TypeIdentifier {
    TypeIdentifier::leaf(n.into())
}

#[test]
fn from_type_expr_leaf_number() {
    assert_eq!(
        KType::from_type_identifier(&leaf("Number")).unwrap(),
        KType::Number
    );
}

#[test]
fn from_type_expr_unknown_paramless_name_errors() {
    assert!(KType::from_type_identifier(&leaf("Banana")).is_err());
}

#[test]
fn from_type_expr_leaf_falls_through_to_builtin() {
    assert_eq!(
        KType::from_type_identifier(&leaf("Number")).unwrap(),
        KType::Number,
    );
}

#[test]
fn from_name_kfunction_no_longer_resolves() {
    assert_eq!(KType::from_name("KFunction"), None);
}

#[test]
fn from_name_list_lowers_to_list_any() {
    assert_eq!(
        KType::from_name("List"),
        Some(KType::List(Box::new(KType::Any)))
    );
}

#[test]
fn from_name_dict_lowers_to_dict_any_any() {
    assert_eq!(
        KType::from_name("Dict"),
        Some(KType::Dict(Box::new(KType::Any), Box::new(KType::Any)))
    );
}

#[test]
fn join_distinct_concretes_yields_any() {
    assert_eq!(KType::join(&KType::Number, &KType::Str), KType::Any);
}

#[test]
fn join_same_yields_same() {
    assert_eq!(KType::join(&KType::Number, &KType::Number), KType::Number);
}

#[test]
fn join_lists_recurses_on_element() {
    let a = KType::List(Box::new(KType::Number));
    let b = KType::List(Box::new(KType::Str));
    assert_eq!(KType::join(&a, &b), KType::List(Box::new(KType::Any)));
}

#[test]
fn join_iter_empty_is_any() {
    let v: Vec<KType> = vec![];
    assert_eq!(KType::join_iter(v), KType::Any);
}

#[test]
fn join_iter_homogeneous() {
    let v = vec![KType::Number, KType::Number, KType::Number];
    assert_eq!(KType::join_iter(v), KType::Number);
}

#[test]
fn join_iter_mixed_yields_any() {
    let v = vec![KType::Number, KType::Str, KType::Bool];
    assert_eq!(KType::join_iter(v), KType::Any);
}

fn function(params: Vec<(&str, KType<'static>)>, ret: KType<'static>) -> KType<'static> {
    KType::KFunction {
        params: Record::from_pairs(params.into_iter().map(|(n, t)| (n.into(), t))),
        ret: Box::new(ret),
    }
}

fn functor(params: Vec<(&str, KType<'static>)>, ret: KType<'static>) -> KType<'static> {
    KType::KFunctor {
        params: Record::from_pairs(params.into_iter().map(|(n, t)| (n.into(), t))),
        ret: Box::new(ret),
        body: None,
    }
}

/// Two same-shape functions join to the shared `KFunction` (the established arm).
#[test]
fn join_same_shape_functions_yields_shared_function() {
    let f1 = function(vec![("x", KType::Number)], KType::Bool);
    let f2 = function(vec![("x", KType::Number)], KType::Bool);
    assert_eq!(KType::join(&f1, &f2), f1.clone());
}

/// Two same-shape functors join to the shared `KFunctor` — the parity the new functor
/// join arm restores, so `[f1, f2]` memoizes `List<:(FUNCTOR …)>` rather than
/// `List<Any>`.
#[test]
fn join_same_shape_functors_yields_shared_functor() {
    let g1 = functor(vec![("x", KType::Number)], KType::OfKind(KKind::Module));
    let g2 = functor(vec![("x", KType::Number)], KType::OfKind(KKind::Module));
    let joined = KType::join(&g1, &g2);
    assert_eq!(joined, g1.clone());
    // The element type a `[g1, g2]` list literal memoizes is the shared functor, not `Any`.
    assert_eq!(
        KType::List(Box::new(KType::join_iter(vec![g1.clone(), g2.clone()]))),
        KType::List(Box::new(g1)),
    );
}

/// Different-shape functors (mismatched key set) are incomparable, so the list join
/// coarsens to `Any` — same fall-through as functions.
#[test]
fn join_different_shape_functors_yields_any() {
    let g1 = functor(vec![("x", KType::Number)], KType::OfKind(KKind::Module));
    let g2 = functor(vec![("y", KType::Number)], KType::OfKind(KKind::Module));
    assert_eq!(KType::join(&g1, &g2), KType::Any);
    assert_eq!(KType::join_iter(vec![g1, g2]), KType::Any);
}

/// A function and a functor of identical shape never join to either family — the
/// variant-tag wall holds through join, falling through to `Any`.
#[test]
fn join_function_and_functor_yields_any() {
    let f = function(vec![("x", KType::Number)], KType::Bool);
    let g = functor(vec![("x", KType::Number)], KType::Bool);
    assert_eq!(KType::join(&f, &g), KType::Any);
}
