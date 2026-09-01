use crate::builtins::test_support::lookup_type;
use crate::builtins::test_support::{TestRun, type_name};
use crate::machine::KErrorKind;
use crate::machine::model::{KKind, NodeSchema, TypeNode};
use crate::machine::model::{KObject, KType};
use crate::machine::program_storage;
use crate::machine::run_root_storage;

/// Assert `identity` names a `SetMember` whose name is `expected`.
fn assert_member_named(
    registries: &crate::machine::model::RunRegistries,
    identity: KType,
    expected: &str,
) {
    match registries.types.node(identity) {
        TypeNode::SetMember { name, .. } => {
            assert_eq!(name, type_name(expected, registries))
        }
        _ => panic!("expected a SetMember identity named `{expected}`, got {identity:?}"),
    }
}

#[test]
fn result_registers_a_two_member_union() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;

    // Type-only: `Result` binds the anonymous union of its two sealed newtype members, exactly as
    // a user `UNION` does; no value-side carrier in `data`.
    let handle = lookup_type(scope, "Result").expect("Result type registered");
    let TypeNode::Union { members } = test_run.types().node(handle) else {
        panic!("expected Result to bind a Union, got {handle:?}")
    };
    assert_eq!(members.len(), 2, "one member per variant");
    for (member, expected) in members.iter().zip(["Ok", "Error"]) {
        match test_run.types().node(*member) {
            TypeNode::SetMember {
                name,
                kind: KKind::NewType,
                schema: NodeSchema::NewType(repr),
                ..
            } => {
                assert_eq!(name, type_name(expected, test_run.registries()));
                assert_eq!(
                    repr,
                    KType::ANY,
                    "a variant's payload type is supplied by application"
                );
            }
            _ => panic!("expected a newtype SetMember named `{expected}`, got {member:?}"),
        }
    }
    assert!(
        scope.lookup("Result").is_none(),
        "Result must not write a value-side carrier into data",
    );
}

#[test]
fn result_constructs_ok_variant() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let result = test_run.run_one(test_run.parse_one("Result.Ok 1"));
    match result {
        KObject::Wrapped { inner, type_id } => {
            assert_member_named(test_run.registries(), *type_id, "Ok");
            assert!(matches!(inner.payload(), KObject::Number(n) if *n == 1.0));
        }
        other => panic!("expected Wrapped, got {:?}", other.ktype()),
    }
}

#[test]
fn result_constructs_error_variant() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let result = test_run.run_one(test_run.parse_one("Result.Error \"x\""));
    match result {
        KObject::Wrapped { inner, type_id } => {
            assert_member_named(test_run.registries(), *type_id, "Error");
            assert!(matches!(inner.payload(), KObject::KString(s) if **s == *"x"));
        }
        other => panic!("expected Wrapped, got {:?}", other.ktype()),
    }
}

/// Projecting a name the union does not declare is the ordinary member miss, naming the members.
#[test]
fn result_rejects_unknown_variant() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let err = test_run.run_one_err(test_run.parse_one("Result.Bogus 1"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("Bogus")),
        "expected ShapeError mentioning Bogus, got {err}",
    );
}

/// `Result` is a union like any other, so the head itself constructs nothing: the application
/// raises the member-projection guidance a user union head gives.
#[test]
fn applying_the_result_head_directs_to_member_projection() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let err = test_run.run_one_err(test_run.parse_one("Result (Ok 1)"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg)
            if msg.contains("member projection") && msg.contains("Ok, Error")),
        "expected the member-projection guidance naming both members, got {err}",
    );
}

/// The carrier flows through the `MATCH … OVER` member walk, the one surface a union is
/// eliminated through.
#[test]
fn result_matches_ok_branch() {
    let program = program_storage();
    let region = run_root_storage();
    let (mut test_run, buf) = TestRun::with_buf(&program, &region);
    test_run.run(
        "MATCH (Result.Ok 1) OVER Result -> :Str WITH (Ok -> (PRINT it) Error -> (PRINT \"no\"))",
    );
    assert_eq!(buf.borrow().as_slice(), b"1\n");
}

/// `:(Result {Ok = …, Error = …})` is type application over the union head: it lowers to the union
/// of per-member applications, and the slot checks the inhabited member's payload against its
/// same-named argument.
#[test]
fn result_type_application_checks_the_inhabited_member() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("LET number_ok = (Result.Ok 1)");
    test_run.run("LET text_ok = (Result.Ok \"x\")");
    let slot = test_run.run_one_type(test_run.parse_one(":(Result {Ok = Number, Error = Str})"));
    let registries = test_run.registries();
    assert!(
        slot.matches_value(scope.expect_value("number_ok"), registries),
        "an `Ok` of a Number inhabits the slot binding `Ok` to Number",
    );
    assert!(
        !slot.matches_value(scope.expect_value("text_ok"), registries),
        "an `Ok` of a Str must not inhabit it",
    );
}

/// A type argument naming no member is reported against the member list.
#[test]
fn result_type_application_rejects_an_unknown_member_name() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let err = test_run.run_one_err(test_run.parse_one("LET x :(Result {Bogus = Number}) = 1"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg)
            if msg.contains("Bogus") && msg.contains("Ok, Error")),
        "expected a member-list miss naming Bogus, got {err}",
    );
}

/// Placeholder install at dispatch time refuses a name already bound to a
/// non-function value (the carrier), so the union errors before finalizing.
#[test]
fn redeclaring_result_errors() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let err = test_run.run_one_err(test_run.parse_one("UNION Result = (Ok :Str Err :Str)"));
    assert!(
        matches!(&err.kind, KErrorKind::Rebind { name } if name == "Result"),
        "expected Rebind on Result, got {err}",
    );
}
