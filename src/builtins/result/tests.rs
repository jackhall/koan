use crate::builtins::test_support::{parse_one, run, run_one, run_one_err, run_root_silent};
use crate::machine::core::run_root_storage;
use crate::machine::model::types::{KKind, ProjectedSchema, RecursiveSet};
use crate::machine::model::{KObject, KType};
use crate::machine::KErrorKind;

#[test]
fn result_registers_type_constructor_with_schema() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);

    // Type-only: `Result`'s `TypeConstructor` member carries both `param_names` and the
    // variant `schema`; no value-side carrier in `data`.
    let identity = scope
        .resolve_type("Result")
        .expect("Result type registered");
    match identity {
        KType::SetRef { set, index } if set.member(*index).kind == KKind::TypeConstructor => {
            assert_eq!(set.member(*index).name, "Result");
            match RecursiveSet::projected_schema(set, *index) {
                ProjectedSchema::TypeConstructor {
                    param_names,
                    schema,
                } => {
                    assert_eq!(param_names.len(), 2);
                    assert_eq!(schema.get("Ok"), Some(&KType::Any));
                    assert_eq!(schema.get("Error"), Some(&KType::Any));
                }
                _ => panic!("expected a TypeConstructor schema"),
            }
        }
        other => panic!("expected arity-2 TypeConstructor SetRef, got {other:?}"),
    }
    assert!(
        scope.lookup("Result").is_none(),
        "Result must not write a value-side carrier into data",
    );
}

#[test]
fn result_constructs_ok_variant() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let result = run_one(scope, parse_one("Result (Ok 1)"));
    match result {
        KObject::Tagged {
            tag,
            value,
            set,
            index,
            ..
        } => {
            assert_eq!(tag, "Ok");
            assert_eq!(set.member(*index).name, "Result");
            assert!(matches!(&**value, KObject::Number(n) if *n == 1.0));
        }
        other => panic!("expected Tagged, got {:?}", other.ktype()),
    }
}

#[test]
fn result_constructs_error_variant() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let result = run_one(scope, parse_one("Result (Error \"x\")"));
    match result {
        KObject::Tagged {
            tag,
            value,
            set,
            index,
            ..
        } => {
            assert_eq!(tag, "Error");
            assert_eq!(set.member(*index).name, "Result");
            assert!(matches!(&**value, KObject::KString(s) if s == "x"));
        }
        other => panic!("expected Tagged, got {:?}", other.ktype()),
    }
}

#[test]
fn result_rejects_unknown_tag() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let err = run_one_err(scope, parse_one("Result (Bogus 1)"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("`Bogus`")),
        "expected ShapeError mentioning `Bogus`, got {err}",
    );
}

/// The carrier flows through MATCH dispatch by tag like any other tagged union.
#[test]
fn result_matches_ok_branch() {
    let region = run_root_storage();
    let (scope, buf) = crate::builtins::test_support::run_root_with_buf(&region);
    run(
        scope,
        "MATCH (Result (Ok 1)) -> :Str WITH (Ok -> (PRINT it) Error -> (PRINT \"no\"))",
    );
    assert_eq!(buf.borrow().as_slice(), b"1\n");
}

/// Placeholder install at dispatch time refuses a name already bound to a
/// non-function value (the carrier), so the union errors before finalizing.
#[test]
fn redeclaring_result_errors() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let err = run_one_err(scope, parse_one("UNION Result = (Ok :Str Err :Str)"));
    assert!(
        matches!(&err.kind, KErrorKind::Rebind { name } if name == "Result"),
        "expected Rebind on Result, got {err}",
    );
}
