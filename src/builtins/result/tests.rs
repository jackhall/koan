use crate::builtins::test_support::{parse_one, run, run_one, run_one_err, run_root_silent};
use crate::machine::model::types::UserTypeKind;
use crate::machine::model::{KObject, KType};
use crate::machine::{KErrorKind, RuntimeArena};

/// `Result` registers a `TypeConstructor` identity on the type side so `:(Result T E)`
/// resolves, and a `TaggedUnionType` carrier (schema `{ok, error}`) on the value side.
#[test]
fn result_registers_type_constructor_and_carrier() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);

    let identity = scope.resolve_type("Result").expect("Result type registered");
    assert!(
        matches!(
            identity,
            KType::UserType { kind: UserTypeKind::TypeConstructor { param_names }, .. }
                if param_names.len() == 2
        ),
        "expected arity-2 TypeConstructor, got {identity:?}",
    );

    let carrier = scope.lookup("Result").expect("Result carrier bound");
    match carrier {
        KObject::TaggedUnionType { schema, name, .. } => {
            assert_eq!(name, "Result");
            assert_eq!(schema.get("ok"), Some(&KType::Any));
            assert_eq!(schema.get("error"), Some(&KType::Any));
        }
        other => panic!("expected TaggedUnionType, got {:?}", other.ktype()),
    }
}

#[test]
fn result_constructs_ok_variant() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let result = run_one(scope, parse_one("Result (ok 1)"));
    match result {
        KObject::Tagged { tag, value, name, .. } => {
            assert_eq!(tag, "ok");
            assert_eq!(name, "Result");
            assert!(matches!(&**value, KObject::Number(n) if *n == 1.0));
        }
        other => panic!("expected Tagged, got {:?}", other.ktype()),
    }
}

#[test]
fn result_constructs_error_variant() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let result = run_one(scope, parse_one("Result (error \"x\")"));
    match result {
        KObject::Tagged { tag, value, name, .. } => {
            assert_eq!(tag, "error");
            assert_eq!(name, "Result");
            assert!(matches!(&**value, KObject::KString(s) if s == "x"));
        }
        other => panic!("expected Tagged, got {:?}", other.ktype()),
    }
}

#[test]
fn result_rejects_unknown_tag() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let err = run_one_err(scope, parse_one("Result (bogus 1)"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("`bogus`")),
        "expected ShapeError mentioning `bogus`, got {err}",
    );
}

/// The carrier flows through MATCH dispatch by tag like any other tagged union.
#[test]
fn result_matches_ok_branch() {
    let arena = RuntimeArena::new();
    let (scope, buf) = crate::builtins::test_support::run_root_with_buf(&arena);
    run(
        scope,
        "MATCH (Result (ok 1)) WITH (ok -> (PRINT it) error -> (PRINT \"no\"))",
    );
    assert_eq!(buf.borrow().as_slice(), b"1\n");
}

/// Redeclaring the builtin `Result` is rejected: the binder placeholder install at
/// dispatch time refuses a name already bound to a non-function value (the carrier),
/// raising `Rebind` before the union ever finalizes.
#[test]
fn redeclaring_result_errors() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let err = run_one_err(scope, parse_one("UNION Result = (ok :Str err :Str)"));
    assert!(
        matches!(&err.kind, KErrorKind::Rebind { name } if name == "Result"),
        "expected Rebind on Result, got {err}",
    );
}
