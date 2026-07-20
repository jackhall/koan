use crate::builtins::test_support::{parse_one, TestRun};
use crate::machine::core::{run_root_storage, KErrorKind};
use crate::machine::model::KObject;

/// The tagged-union value-type check fires when the value-cell resolves to a
/// `KObject` that doesn't match the tag's expected type.
#[test]
fn ctor_fast_lane_rejects_value_of_wrong_type() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    test_run.run("UNION Maybe = (Some :Number None :Null)");
    let err = test_run.run_one_err(parse_one("Maybe (Some \"oops\")"));
    match &err.kind {
        KErrorKind::TypeMismatch { arg, expected, got } => {
            assert_eq!(arg, "value");
            assert_eq!(expected, "Number");
            assert_eq!(got, "Str");
        }
        _ => panic!("expected TypeMismatch on value, got {err}"),
    }
}

/// `TypeCall` fast lane (leaf-Type head) propagates the schema's tag check.
#[test]
fn ctor_fast_lane_propagates_tag_validation_error() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    test_run.run("UNION Maybe = (Some :Number None :Null)");
    let err = test_run.run_one_err(parse_one("Maybe (Other 42)"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("`Other`")),
        "expected ShapeError mentioning `Other`, got {err}",
    );
}

/// Value-cell sub-expression `(x)` rides the `BareIdentifier` fast lane to resolve
/// `x` before the newtype construction sees the value bind. A user-union variant value is an
/// ordinary `KObject::Wrapped` over the member `SetRef`, not a `KObject::Tagged`.
#[test]
fn ctor_fast_lane_with_sub_expression_value() {
    use crate::machine::model::KType;
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    test_run.run("UNION Maybe = (Some :Number None :Null)\nLET x = 7");
    let result = test_run.run_one(parse_one("Maybe (Some (x))"));
    match result {
        KObject::Wrapped { inner, type_id } => {
            assert!(matches!(inner.get(), KObject::Number(n) if *n == 7.0));
            match type_id {
                KType::SetRef { set, index } => assert_eq!(set.member(*index).name, "Some"),
                other => panic!("expected a member SetRef type_id, got {other:?}"),
            }
        }
        other => panic!("expected Wrapped, got {:?}", other.ktype()),
    }
}
