//! The combined statement form `LET <name> = FN <signature> -> <Return> = (<body>)`: one
//! statement whose single binder installs the value name and the signature's dispatch bucket, both
//! naming the one function it builds.

use crate::builtins::test_support::{TestRun, fn_is_registered};
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
    let by_keyword = test_run.run_one(test_run.parse_one("DOUBLE 5"));
    assert!(matches!(by_keyword, KObject::Number(n) if *n == 10.0));
    let by_name = test_run.run_one(test_run.parse_one("double {n = 7}"));
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

    let by_keyword = test_run.run_one(test_run.parse_one("OFFSET 1"));
    assert!(matches!(by_keyword, KObject::Number(n) if *n == 101.0));
    let by_name = test_run.run_one(test_run.parse_one("offset {n = 1}"));
    assert!(matches!(by_name, KObject::Number(n) if *n == 101.0));
}

/// A sibling reference parks on the combined statement through *either* channel: the
/// submission-time plan stamps a name placeholder and a pending-overload entry at one node, and —
/// because statement-at-a-time submission puts every statement in flight before any of them
/// executes — the referencing statement still parks on the declaring statement even though the
/// declaration precedes it, since the declaration is still finalizing when the reference steps.
#[test]
fn sibling_reference_parks_on_both_channels() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "LET triple = FN (TRIPLE n :Number) -> Number = (n * 3)\n\
         FN (CALLER) -> Number = (TRIPLE 3)",
    );
    let via_bucket = test_run.run_one(test_run.parse_one("CALLER"));
    assert!(matches!(via_bucket, KObject::Number(n) if *n == 9.0));

    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "LET quad = FN (QUAD n :Number) -> Number = (n * 4)\n\
         LET alias = quad",
    );
    let via_name = test_run.run_one(test_run.parse_one("alias"));
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
    let result = test_run.run_one(test_run.parse_one("PACK 3"));
    assert!(matches!(result, KObject::List(..)));
}

/// Inside a SIG body a value slot is declared with `VAL`, never bound — the combined form is a
/// binding, so it is rejected there exactly as plain `LET` is.
#[test]
fn combined_form_is_rejected_in_a_sig_body() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let err = test_run.run_one_err(
        test_run.parse_one("SIG Shape = (LET area = FN (AREA n :Number) -> Number = (n))"),
    );
    assert!(
        format!("{err}").contains("VAL"),
        "expected the VAL suggestion, got {err}",
    );
}

/// `FN :{…}` is anonymous — it registers no bucket, so there is no combined form for it: the
/// combined key's signature slot captures only code, so the `:{…}` there evaluates to a record type
/// no overload admits and the flat spelling fails dispatch. The parenthesized value-bind spelling
/// stays the working one.
#[test]
fn anonymous_signature_has_no_combined_form() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let err = test_run.run_one_err(test_run.parse_one("LET f = FN :{n :Number} -> Number = (n)"));
    assert!(
        matches!(err.kind, crate::machine::KErrorKind::DispatchFailed { .. }),
        "expected the flat spelling to match no combined overload, got {err}",
    );

    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("LET f = (FN :{n :Number} -> Number = (n))");
    let result = test_run.run_one(test_run.parse_one("f {n = 3}"));
    assert!(matches!(result, KObject::Number(n) if *n == 3.0));
}

/// A function is a value, so it binds under a value-classified identifier. A Type-classified
/// binder name gets the snake_case suggestion rather than a bare dispatch miss.
#[test]
fn type_classified_binder_name_is_a_diagnostic() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let err = test_run
        .run_one_err(test_run.parse_one("LET Doubler = FN (DOUBLER n :Number) -> Number = (n)"));
    let message = format!("{err}");
    assert!(
        message.contains("doubler"),
        "expected the snake_case suggestion, got {message}",
    );
}

/// The binder captures its token and never lowers it, so a name that happens to spell a builtin
/// type takes the same diagnostic naming the same token. Nothing on this path renders a type, so
/// no spelling can report a lowered node in place of what the user wrote.
#[test]
fn a_builtin_spelled_binder_name_is_diagnosed_as_written() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    for name in ["Str", "List", "Dict"] {
        let source = format!("LET {name} = FN (MAKE n :Number) -> Number = (n)");
        let err = test_run.run_one_err(test_run.parse_one(&source));
        let message = format!("{err}");
        assert!(
            message.contains(&format!("`{name}`")),
            "expected the diagnostic to name `{name}` as written, got {message}",
        );
        assert!(
            !message.contains("LIST OF") && !message.contains("MAP "),
            "the binder never lowers, so no diagnostic renders one: {message}",
        );
    }
}

/// The return slot names a type; an identifier there is the same mistake the bare form diagnoses,
/// and the combined twin reports it identically.
#[test]
fn value_named_return_is_diagnosed_in_the_combined_form() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let err =
        test_run.run_one_err(test_run.parse_one("LET f = FN (WIDEN n :Number) -> other = (n)"));
    assert!(
        format!("{err}").contains("TYPE OF"),
        "expected the `-> :(TYPE OF …)` suggestion, got {err}",
    );
}
