//! Anonymous functions: the keyword-less `FN :{<record schema>} -> T = (body)`
//! binder. The record schema resolves to a record `KType` before the FN body
//! fires; each field becomes a keyword-less `Argument`, so the function
//! registers no dispatch keyword and is reachable only through its value —
//! bound by `LET` or dropped into a function-typed slot, and called by record
//! (`f {x = 1}`).

use crate::builtins::test_support::TestRun;
use crate::machine::KErrorKind;
use crate::machine::model::KObject;
use crate::machine::{program_storage, run_root_storage};

use super::capture_program_output;

/// A record-schema binder produces a callable value with no keyword; calling it
/// by record runs the body against the named field.
#[test]
fn anonymous_fn_call_by_record_runs_body() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("LET f = (FN :{x :Number} -> Number = (x))");
    let result = test_run.run_one(test_run.parse_one("f {x = 7}"));
    assert!(
        matches!(result, KObject::Number(n) if *n == 7.0),
        "f {{x = 7}} should run the body and return 7",
    );
}

/// An anonymous `FN` binds a `KFunction` value.
#[test]
fn anonymous_fn_binds_a_function_value() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("LET f = (FN :{x :Number} -> Number = (x))");
    let result = test_run.run_one(test_run.parse_one("f"));
    assert!(
        matches!(result, KObject::KFunction(..)),
        "an anonymous FN binds a callable value",
    );
}

/// Empty schema `:{}` is a no-parameter thunk, called with the empty record.
#[test]
fn anonymous_fn_empty_thunk() {
    let bytes = capture_program_output(
        "LET g = (FN :{} -> Str = (\"hi\"))\n\
         PRINT (g {})",
    );
    assert_eq!(bytes, b"hi\n");
}

/// Multiple fields bind by name regardless of call-site field order.
#[test]
fn anonymous_fn_multi_param_binds_by_name() {
    let bytes = capture_program_output(
        "LET f = (FN :{x :Number, y :Str} -> Str = (y))\n\
         PRINT (f {y = \"a\", x = 1})",
    );
    assert_eq!(bytes, b"a\n");
}

/// An anonymous FN value fills a function-typed parameter slot
/// (`:(FN :{x :Number} -> Str)`) via the same `function_compat` check a keyworded
/// inline FN uses — its keyword-less signature projects the same
/// `KType::KFunction`.
#[test]
fn anonymous_fn_fills_function_typed_slot() {
    let bytes = capture_program_output(
        "FN (USE f :(FN :{x :Number} -> Str)) -> Str = (\"got fn\")\n\
         PRINT (USE (FN :{x :Number} -> Str = (\"hi\")))",
    );
    assert_eq!(bytes, b"got fn\n");
}

/// A field whose type needs its own sub-dispatch (`:(LIST OF Number)`) resolves
/// during operand resolution, so the FN body still receives a fully-resolved
/// record schema.
#[test]
fn anonymous_fn_with_sub_dispatched_field_type() {
    let bytes = capture_program_output(
        "LET f = (FN :{xs :(LIST OF Number)} -> Number = (1))\n\
         PRINT (f {xs = [1, 2, 3]})",
    );
    assert_eq!(bytes, b"1\n");
}

/// Functions are called by record, never positionally: a positional `f (1)`
/// surfaces the `NAMED_ONLY` dispatch failure rather than binding.
#[test]
fn anonymous_fn_rejects_positional_call() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("LET f = (FN :{x :Number} -> Number = (x))");
    let error = test_run.run_one_err(test_run.parse_one("f (1)"));
    assert!(
        matches!(error.kind, KErrorKind::DispatchFailed { .. }),
        "a positional call on an anonymous FN should fail dispatch, got {error:?}",
    );
}

/// A non-record signature operand (`:Number`) is a shape error: the anonymous
/// binder demands a record schema.
#[test]
fn anonymous_fn_non_record_signature_is_shape_error() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let error = test_run.run_one_err(test_run.parse_one("FN :Number -> Number = (1)"));
    assert!(
        matches!(error.kind, KErrorKind::ShapeError(_)),
        "a non-record `:T` signature should be a shape error, got {error:?}",
    );
}

/// The `signature` slot stays a pure kind expectation rather than a raw-capture union: an
/// anonymous FN whose parameter record names a *later*-announced sibling still elaborates, because
/// the eager consumer sub-dispatch parks until the module body's window seals. The same holds when
/// the parameter is function-typed over the recursive pair.
#[test]
fn an_anonymous_signature_over_a_later_announced_sibling_elaborates() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "MODULE pair = (\n  NEWTYPE Aa = Number\n  \
         LET plain = (FN :{b :Bb} -> Number = (1))\n  \
         LET higher = (FN :{g :(FN :{a :Aa} -> Bb)} -> Number = (2))\n  \
         NEWTYPE Bb = Number\n)",
    );
    assert!(
        crate::builtins::test_support::binds_module(scope, "pair"),
        "the announced pair seals with both anonymous FNs elaborated",
    );
}

/// The keyworded return slot's raw capture carries the same guarantee from the other side: a
/// return naming a later-announced sibling parks and resolves at seal.
#[test]
fn a_keyworded_return_naming_a_later_announced_sibling_elaborates() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "MODULE pair = (\n  NEWTYPE Aa = Number\n  \
         FN (GETB a :Aa) -> Bb = (1)\n  \
         NEWTYPE Bb = Number\n)",
    );
    assert!(
        crate::builtins::test_support::binds_module(scope, "pair"),
        "the announced pair seals with the forward-naming return resolved",
    );
}

/// AC5: the `signature` slot is a pure kind expectation, so a bare `Type` token naming a record
/// alias auto-wraps and resolves like any other — an alias and a `:{…}` literal reach the same
/// body read. Parity with the shipped `:(FN Params -> Bool)` constructor surface.
#[test]
fn an_anonymous_signature_takes_a_record_alias() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "LET Params = :{x :Number}\n\
         LET f = (FN Params -> Number = (x * 2))\n\
         LET out = (f {x = 5})",
    );
    assert!(
        matches!(scope.lookup("out"), Some(KObject::Number(n)) if *n == 10.0),
        "the aliased signature defines a function that runs",
    );
}
