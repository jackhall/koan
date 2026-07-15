//! Anonymous functions: the keyword-less `FN :{<record schema>} -> T = (body)`
//! binder. The record schema resolves to a `KType::Record` before the FN body
//! fires; each field becomes a keyword-less `Argument`, so the function
//! registers no dispatch keyword and is reachable only through its value —
//! bound by `LET` or dropped into a function-typed slot, and called by record
//! (`f {x = 1}`).

use crate::builtins::test_support::{parse_one, run, run_one, run_one_err, run_root_silent};
use crate::machine::model::KObject;
use crate::machine::run_root_storage;
use crate::machine::KErrorKind;

use super::capture_program_output;

/// A record-schema binder produces a callable value with no keyword; calling it
/// by record runs the body against the named field.
#[test]
fn anonymous_fn_call_by_record_runs_body() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "LET f = (FN :{x :Number} -> Number = (x))");
    let result = run_one(scope, parse_one("f {x = 7}"));
    assert!(
        matches!(result, KObject::Number(n) if *n == 7.0),
        "f {{x = 7}} should run the body and return 7",
    );
}

/// The bound value is a `KFunction` — the only handle to an anonymous function.
#[test]
fn anonymous_fn_binds_a_function_value() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "LET f = (FN :{x :Number} -> Number = (x))");
    let result = run_one(scope, parse_one("f"));
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
/// (`:(FN (x :Number) -> Str)`) via the same `function_compat` check a keyworded
/// inline FN uses — its keyword-less signature projects the same
/// `KType::KFunction`.
#[test]
fn anonymous_fn_fills_function_typed_slot() {
    let bytes = capture_program_output(
        "FN (USE f :(FN (x :Number) -> Str)) -> Str = (\"got fn\")\n\
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
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "LET f = (FN :{x :Number} -> Number = (x))");
    let error = run_one_err(scope, parse_one("f (1)"));
    assert!(
        matches!(error.kind, KErrorKind::DispatchFailed { .. }),
        "a positional call on an anonymous FN should fail dispatch, got {error:?}",
    );
}

/// A non-record signature operand (`:Number`) is a shape error: the anonymous
/// binder demands a record schema.
#[test]
fn anonymous_fn_non_record_signature_is_shape_error() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let error = run_one_err(scope, parse_one("FN :Number -> Number = (1)"));
    assert!(
        matches!(error.kind, KErrorKind::ShapeError(_)),
        "a non-record `:T` signature should be a shape error, got {error:?}",
    );
}
