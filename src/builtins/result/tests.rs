use crate::builtins::test_support::{parse_one, run, run_one, run_one_err, run_root_silent};
use crate::machine::model::types::UserTypeKind;
use crate::machine::model::{KObject, KType};
use crate::machine::{KErrorKind, RuntimeArena};

#[test]
fn result_registers_type_constructor_with_schema() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);

    // Type-only: `Result`'s `TypeConstructor` identity carries both `param_names` and
    // the variant `schema` payload; no value-side carrier in `data`.
    let identity = scope
        .resolve_type("Result")
        .expect("Result type registered");
    match identity {
        KType::UserType {
            kind:
                UserTypeKind::TypeConstructor {
                    param_names,
                    schema,
                },
            name,
            ..
        } => {
            assert_eq!(name, "Result");
            assert_eq!(param_names.len(), 2);
            assert_eq!(schema.get("ok"), Some(&KType::Any));
            assert_eq!(schema.get("error"), Some(&KType::Any));
        }
        other => panic!("expected arity-2 TypeConstructor with schema, got {other:?}"),
    }
    assert!(
        scope.lookup("Result").is_none(),
        "Result must not write a value-side carrier into data",
    );
}

#[test]
fn result_constructs_ok_variant() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let result = run_one(scope, parse_one("Result (ok 1)"));
    match result {
        KObject::Tagged {
            tag, value, name, ..
        } => {
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
        KObject::Tagged {
            tag, value, name, ..
        } => {
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

/// Placeholder install at dispatch time refuses a name already bound to a
/// non-function value (the carrier), so the union errors before finalizing.
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
