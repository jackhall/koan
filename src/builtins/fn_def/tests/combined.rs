//! The combined statement form `LET <name> = FN <signature> -> <Return> = (<body>)`: one
//! statement whose single binder installs the value name and the signature's dispatch bucket, both
//! naming the one function it builds.

use crate::builtins::test_support::{fn_is_registered, parse_one, TestRun};
use crate::machine::model::KObject;
use crate::machine::{program_storage, run_root_storage};

/// Both channels install: the keyworded call dispatches, and the bound name holds the callable.
#[test]
fn combined_form_installs_name_and_bucket() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("LET double = FN (DOUBLE n :Number) -> Number = (n * 2)");

    assert!(fn_is_registered(scope, "DOUBLE"));
    let by_keyword = test_run.run_one(parse_one(&program, "DOUBLE 5"));
    assert!(matches!(by_keyword, KObject::Number(n) if *n == 10.0));
    let by_name = test_run.run_one(parse_one(&program, "double {n = 7}"));
    assert!(
        matches!(by_name, KObject::Number(n) if *n == 14.0),
        "the bound name reaches the same function through the call-by-name lane"
    );
}

/// The bound value and the registered overload are the same function: calling through either
/// spelling reaches one body, so a closure over a sibling binding reads the same capture.
#[test]
fn bound_value_and_overload_are_one_function() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("LET base = 100\nLET offset = FN (OFFSET n :Number) -> Number = (n + base)");

    let by_keyword = test_run.run_one(parse_one(&program, "OFFSET 1"));
    assert!(matches!(by_keyword, KObject::Number(n) if *n == 101.0));
    let by_name = test_run.run_one(parse_one(&program, "offset {n = 1}"));
    assert!(matches!(by_name, KObject::Number(n) if *n == 101.0));
}

/// A forward sibling reference parks on the combined statement through *either* channel: the
/// submission-time plan stamps a name placeholder and a pending-overload entry at one node.
#[test]
fn forward_reference_parks_on_both_channels() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "FN (CALLER) -> Number = (TRIPLE 3)\n\
         LET triple = FN (TRIPLE n :Number) -> Number = (n * 3)",
    );
    let via_bucket = test_run.run_one(parse_one(&program, "CALLER"));
    assert!(matches!(via_bucket, KObject::Number(n) if *n == 9.0));

    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "LET alias = quad\n\
         LET quad = FN (QUAD n :Number) -> Number = (n * 4)",
    );
    let via_name = test_run.run_one(parse_one(&program, "alias"));
    assert!(
        matches!(via_name, KObject::KFunction(..)),
        "the name channel's placeholder resolves to the declared callable"
    );
}

/// A `:(…)` return carrier reaches the combined form too — the sigiled-return overload is
/// registered alongside the proper-type one, as it is for the bare `FN`.
#[test]
fn combined_form_takes_a_sigiled_return_carrier() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("LET pack = FN (PACK n :Number) -> :(LIST OF Number) = ([n])");
    let result = test_run.run_one(parse_one(&program, "PACK 3"));
    assert!(matches!(result, KObject::List(..)));
}

/// Inside a SIG body a value slot is declared with `VAL`, never bound — the combined form is a
/// binding, so it is rejected there exactly as plain `LET` is.
#[test]
fn combined_form_is_rejected_in_a_sig_body() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let err = test_run.run_one_err(parse_one(
        &program,
        "SIG Shape = (LET area = FN (AREA n :Number) -> Number = (n))",
    ));
    assert!(
        format!("{err}").contains("VAL"),
        "expected the VAL suggestion, got {err}",
    );
}

/// `FN :{…}` is anonymous — it registers no bucket, so there is no combined form for it: the flat
/// spelling matches no overload, and the parenthesized value bind stays the spelling.
#[test]
fn anonymous_signature_has_no_combined_form() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let err = test_run.run_one_err(parse_one(
        &program,
        "LET f = FN :{n :Number} -> Number = (n)",
    ));
    assert!(
        matches!(err.kind, crate::machine::KErrorKind::DispatchFailed { .. }),
        "expected a dispatch miss, got {err}",
    );

    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("LET f = (FN :{n :Number} -> Number = (n))");
    let result = test_run.run_one(parse_one(&program, "f {n = 3}"));
    assert!(matches!(result, KObject::Number(n) if *n == 3.0));
}

/// A function is a value, so it binds under a value-classified identifier. A Type-classified
/// binder name gets the snake_case suggestion rather than a bare dispatch miss.
#[test]
fn type_classified_binder_name_is_a_diagnostic() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let err = test_run.run_one_err(parse_one(
        &program,
        "LET Doubler = FN (DOUBLER n :Number) -> Number = (n)",
    ));
    let message = format!("{err}");
    assert!(
        message.contains("doubler"),
        "expected the snake_case suggestion, got {message}",
    );
}

/// The return slot names a type; an identifier there is the same mistake the bare form diagnoses,
/// and the combined twin reports it identically.
#[test]
fn value_named_return_is_diagnosed_in_the_combined_form() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let err = test_run.run_one_err(parse_one(
        &program,
        "LET f = FN (WIDEN n :Number) -> other = (n)",
    ));
    assert!(
        format!("{err}").contains("TYPE OF"),
        "expected the `-> :(TYPE OF …)` suggestion, got {err}",
    );
}
