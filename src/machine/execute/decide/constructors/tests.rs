use crate::builtins::test_support::{TestRun, type_name};
use crate::machine::core::{KErrorKind, program_storage, run_root_storage};
use crate::machine::model::KObject;

/// The variant value-type check fires when the value-cell resolves to a
/// `KObject` that doesn't match the member's declared repr.
#[test]
fn ctor_fast_lane_rejects_value_of_wrong_type() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("UNION Maybe = (Some :Number None :Null)");
    let err = test_run.run_one_err(test_run.parse_one("Maybe.Some \"oops\""));
    match &err.kind {
        KErrorKind::TypeMismatch { arg, expected, got } => {
            assert_eq!(arg, "value");
            assert_eq!(expected, "Number");
            assert_eq!(got, "Str");
        }
        _ => panic!("expected TypeMismatch on value, got {err}"),
    }
}

/// Projecting a name the union does not declare is an error at the ATTR, listing the members.
#[test]
fn ctor_fast_lane_propagates_tag_validation_error() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("UNION Maybe = (Some :Number None :Null)");
    let err = test_run.run_one_err(test_run.parse_one("Maybe.Other 42"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("`Other`")),
        "expected ShapeError mentioning `Other`, got {err}",
    );
}

/// Value-cell sub-expression `(x)` rides the `BareIdentifier` fast lane to resolve
/// `x` before the variant construction sees the value bind. A user-union variant value is a
/// `KObject::Wrapped` — the shape every newtype produces — whose `type_id` is the member's own
/// sealed `SetMember` handle, since the projected member is what construction applies.
#[test]
fn ctor_fast_lane_with_sub_expression_value() {
    use crate::machine::model::TypeNode;
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("UNION Maybe = (Some :Number None :Null)\nLET x = 7");
    let result = test_run.run_one(test_run.parse_one("Maybe.Some (x)"));
    match result {
        KObject::Wrapped { inner, type_id } => {
            assert!(matches!(inner.payload(), KObject::Number(n) if *n == 7.0));
            match test_run.types().node(*type_id) {
                TypeNode::SetMember { name, .. } => {
                    assert_eq!(name, type_name("Some", test_run.registries()))
                }
                _ => panic!("expected a member SetMember identity, got {type_id:?}"),
            }
        }
        other => panic!("expected Wrapped, got {:?}", other.ktype()),
    }
}
