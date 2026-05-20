use super::*;
use crate::machine::core::ScopeId;
use crate::machine::model::ast::TypeExpr;
use crate::machine::model::types::UserTypeKind;

fn params(names: &[&str]) -> HashSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}

fn list_te(inner: TypeExpr) -> TypeExpr {
    TypeExpr {
        name: "List".into(),
        params: TypeParams::List(vec![inner]),
        builtin_cache: std::cell::OnceCell::new(),
    }
}

fn dict_te(k: TypeExpr, v: TypeExpr) -> TypeExpr {
    TypeExpr {
        name: "Dict".into(),
        params: TypeParams::List(vec![k, v]),
        builtin_cache: std::cell::OnceCell::new(),
    }
}

fn ctor_te(name: &str, args: Vec<TypeExpr>) -> TypeExpr {
    TypeExpr {
        name: name.into(),
        params: TypeParams::List(args),
        builtin_cache: std::cell::OnceCell::new(),
    }
}

/// `:(List T)` against `List<Number>` binds `T = Number`.
#[test]
fn list_param_binds_element_type() {
    let declared = list_te(TypeExpr::leaf("T".into()));
    let actual = KType::List(Box::new(KType::Number));
    let result = unify_slot(&declared, &actual, &params(&["T"]));
    assert_eq!(result, UnifyResult::Bound(vec![("T".into(), KType::Number)]));
}

/// A concrete-leaf slot binds nothing and still unifies (the caller's `matches_value`
/// owns the agreement check).
#[test]
fn concrete_leaf_binds_nothing() {
    let declared = list_te(TypeExpr::leaf("Number".into()));
    let actual = KType::List(Box::new(KType::Number));
    assert_eq!(
        unify_slot(&declared, &actual, &params(&["T"])),
        UnifyResult::Bound(vec![])
    );
}

/// `:(Dict K V)` against `Dict<Str, Number>` binds both params position-wise.
#[test]
fn dict_params_bind_key_and_value() {
    let declared = dict_te(TypeExpr::leaf("K".into()), TypeExpr::leaf("V".into()));
    let actual = KType::Dict(Box::new(KType::Str), Box::new(KType::Number));
    match unify_slot(&declared, &actual, &params(&["K", "V"])) {
        UnifyResult::Bound(b) => {
            assert!(b.contains(&("K".into(), KType::Str)));
            assert!(b.contains(&("V".into(), KType::Number)));
        }
        other => panic!("expected Bound, got {other:?}"),
    }
}

/// `:(Result T E)` against `ConstructorApply { Result, [Number, MyErr] }` binds T and E.
#[test]
fn constructor_apply_params_bind_args() {
    let declared = ctor_te("Result", vec![
        TypeExpr::leaf("T".into()),
        TypeExpr::leaf("E".into()),
    ]);
    let myerr = KType::UserType {
        kind: UserTypeKind::Tagged,
        scope_id: ScopeId::from_raw(0, 0x1),
        name: "MyErr".into(),
    };
    let result_ctor = KType::UserType {
        kind: UserTypeKind::TypeConstructor { param_names: vec!["T".into(), "E".into()] },
        scope_id: ScopeId::from_raw(0, 0x2),
        name: "Result".into(),
    };
    let actual = KType::ConstructorApply {
        ctor: Box::new(result_ctor),
        args: vec![KType::Number, myerr.clone()],
    };
    match unify_slot(&declared, &actual, &params(&["T", "E"])) {
        UnifyResult::Bound(b) => {
            assert!(b.contains(&("T".into(), KType::Number)));
            assert!(b.contains(&("E".into(), myerr)));
        }
        other => panic!("expected Bound, got {other:?}"),
    }
}

/// Nested: `:(List (List T))` against `List<List<Number>>` binds `T = Number`.
#[test]
fn nested_list_param_binds() {
    let declared = list_te(list_te(TypeExpr::leaf("T".into())));
    let actual = KType::List(Box::new(KType::List(Box::new(KType::Number))));
    assert_eq!(
        unify_slot(&declared, &actual, &params(&["T"])),
        UnifyResult::Bound(vec![("T".into(), KType::Number)])
    );
}

/// A param that appears twice must bind consistently — conflicting occurrences mismatch.
#[test]
fn repeated_param_conflicting_binding_mismatches() {
    let declared = dict_te(TypeExpr::leaf("T".into()), TypeExpr::leaf("T".into()));
    let actual = KType::Dict(Box::new(KType::Str), Box::new(KType::Number));
    assert!(matches!(
        unify_slot(&declared, &actual, &params(&["T"])),
        UnifyResult::Mismatch(_)
    ));
}

/// A param that appears twice with the same concrete type binds once.
#[test]
fn repeated_param_consistent_binding_ok() {
    let declared = dict_te(TypeExpr::leaf("T".into()), TypeExpr::leaf("T".into()));
    let actual = KType::Dict(Box::new(KType::Number), Box::new(KType::Number));
    assert_eq!(
        unify_slot(&declared, &actual, &params(&["T"])),
        UnifyResult::Bound(vec![("T".into(), KType::Number)])
    );
}

/// Head-constructor / shape disagreement is a mismatch.
#[test]
fn shape_disagreement_mismatches() {
    let declared = list_te(TypeExpr::leaf("T".into()));
    let actual = KType::Dict(Box::new(KType::Str), Box::new(KType::Number));
    assert!(matches!(
        unify_slot(&declared, &actual, &params(&["T"])),
        UnifyResult::Mismatch(_)
    ));
}

/// Constructor arity disagreement is a mismatch.
#[test]
fn constructor_arity_mismatch() {
    let declared = ctor_te("Result", vec![TypeExpr::leaf("T".into())]);
    let result_ctor = KType::UserType {
        kind: UserTypeKind::TypeConstructor { param_names: vec!["T".into(), "E".into()] },
        scope_id: ScopeId::from_raw(0, 0x2),
        name: "Result".into(),
    };
    let actual = KType::ConstructorApply {
        ctor: Box::new(result_ctor),
        args: vec![KType::Number, KType::Str],
    };
    assert!(matches!(
        unify_slot(&declared, &actual, &params(&["T"])),
        UnifyResult::Mismatch(_)
    ));
}
