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
/// `x` before the variant construction sees the value bind. A user-union variant value is a
/// `KObject::Tagged` — the same shape builtin `Result` produces — carrying its variant tag and,
/// as `identity`, the member's own sealed `SetMember` handle.
#[test]
fn ctor_fast_lane_with_sub_expression_value() {
    use crate::machine::model::TypeNode;
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    test_run.run("UNION Maybe = (Some :Number None :Null)\nLET x = 7");
    let result = test_run.run_one(parse_one("Maybe (Some (x))"));
    match result {
        KObject::Tagged {
            tag,
            value,
            identity,
        } => {
            assert_eq!(tag, "Some");
            assert!(matches!(value.payload(), KObject::Number(n) if *n == 7.0));
            match test_run.types.node(*identity) {
                TypeNode::SetMember { name, .. } => assert_eq!(name, "Some"),
                _ => panic!("expected a member SetMember identity, got {identity:?}"),
            }
        }
        other => panic!("expected Tagged, got {:?}", other.ktype()),
    }
}
