//! Typed parameters: dispatch routing on parameter types, overloads, shape errors.

use crate::dispatch::builtins::test_support::{parse_one, run, run_one, run_root_silent};
use crate::dispatch::{Argument, KErrorKind, KObject, KType, RuntimeArena, SignatureElement};
use crate::execute::scheduler::Scheduler;

use super::capture_program_output;

/// A typed parameter records its declared `KType` on the registered signature, rather
/// than collapsing to `Any` as it did before per-param annotations existed.
#[test]
fn fn_typed_param_records_ktype_on_signature() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (DOUBLE x: Number) -> Number = (x)");

    let data = scope.data.borrow();
    let entry = data.get("DOUBLE").expect("DOUBLE should be bound");
    let f = match entry {
        KObject::KFunction(f, _) => *f,
        _ => panic!("expected DOUBLE to bind a KFunction"),
    };
    match f.signature.elements.as_slice() {
        [SignatureElement::Keyword(kw), SignatureElement::Argument(Argument { name, ktype })] => {
            assert_eq!(kw, "DOUBLE");
            assert_eq!(name, "x");
            assert_eq!(*ktype, KType::Number);
        }
        _ => panic!("expected signature shape [Keyword(\"DOUBLE\"), Argument(x: Number)]"),
    }
}

/// A call whose argument satisfies the parameter type dispatches into the body.
#[test]
fn fn_typed_param_dispatches_on_matching_call() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (DOUBLE x: Number) -> Number = (x)");
    let result = run_one(scope, parse_one("DOUBLE 7"));
    assert!(matches!(result, KObject::Number(n) if *n == 7.0));
}

/// A call whose argument doesn't satisfy the parameter type fails at dispatch with
/// `DispatchFailed` (the per-slot type check filters out the only candidate, so the
/// scope chain runs out without a match). Same path as builtins.
#[test]
fn fn_typed_param_rejects_mismatched_call() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (DOUBLE x: Number) -> Number = (x)");
    let mut sched = Scheduler::new();
    let _ = sched.add_dispatch(parse_one("DOUBLE \"hi\""), scope);
    // The dispatch failure surfaces via `execute()` here (the queue can't make
    // progress past the unmatchable call). The other shape — `execute() -> Ok` plus
    // a per-slot Err — is what return-type mismatches use; this case is different.
    let err = sched.execute().expect_err("DOUBLE \"hi\" should fail dispatch");
    assert!(
        matches!(&err.kind, KErrorKind::DispatchFailed { .. }),
        "expected DispatchFailed for type-mismatched DOUBLE call, got {err}",
    );
}

/// Two FNs sharing a shape but differing on parameter type both register, and dispatch
/// routes each call to the body whose type signature matches. Exercises the existing
/// slot-specificity path now that user-fns can carry concrete types.
#[test]
fn fn_overloads_dispatch_by_param_type() {
    let bytes = capture_program_output(
        "FN (DESCRIBE x: Number) -> Null = (PRINT \"number\")\n\
         FN (DESCRIBE x: Str) -> Null = (PRINT \"string\")\n\
         DESCRIBE 7\n\
         DESCRIBE \"hi\"",
    );
    assert_eq!(bytes, b"number\nstring\n");
}

/// A bare identifier without `: Type` in a parameter slot is rejected with a
/// `ShapeError` naming the offending parameter.
#[test]
fn fn_param_without_annotation_is_rejected() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(parse_one("FN (DOUBLE x) -> Number = (x)"), scope);
    sched.execute().expect("execute does not surface per-slot errors");
    let err = match sched.read_result(id) {
        Err(e) => e,
        Ok(_) => panic!("untyped parameter should error"),
    };
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("`x`")),
        "expected ShapeError mentioning `x`, got {err}",
    );
    let data = scope.data.borrow();
    assert!(data.get("DOUBLE").is_none(), "DOUBLE should not register");
}

/// An unknown type name in a parameter slot surfaces as a `ShapeError` mentioning the
/// bad name, mirroring the return-type case.
#[test]
fn fn_param_with_unknown_type_name_is_rejected() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(parse_one("FN (DOUBLE x: Bogus) -> Number = (x)"), scope);
    sched.execute().expect("execute does not surface per-slot errors");
    let err = match sched.read_result(id) {
        Err(e) => e,
        Ok(_) => panic!("unknown param type should error"),
    };
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("Bogus")),
        "expected ShapeError mentioning `Bogus`, got {err}",
    );
}

/// Comma-separated parameter triples parse the same as whitespace-separated ones —
/// the parser strips commas inside expression frames, so `(x: Number, y: Number)`
/// and `(x: Number y: Number)` are interchangeable.
#[test]
fn fn_comma_separated_typed_params_register() {
    let bytes = capture_program_output(
        "FN (FIRST x: Number, y: Number) -> Number = (x)\n\
         PRINT (FIRST 1 2)",
    );
    assert_eq!(bytes, b"1\n");
}
