//! Typed parameters: dispatch routing on parameter types, overloads, shape errors.

use crate::builtins::test_support::{
    fn_is_registered, lookup_fn, parse_one, run, run_one, run_root_silent,
};
use crate::machine::execute::KoanRuntime;
use crate::machine::model::{Argument, KObject, KType, SignatureElement};
use crate::machine::{KErrorKind, RuntimeArena};

use super::capture_program_output;

#[test]
fn fn_typed_param_records_ktype_on_signature() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (DOUBLE x :Number) -> Number = (x)");

    let f = lookup_fn(scope, "DOUBLE");
    match f.signature.elements.as_slice() {
        [SignatureElement::Keyword(kw), SignatureElement::Argument(Argument { name, ktype })] => {
            assert_eq!(kw, "DOUBLE");
            assert_eq!(name, "x");
            assert_eq!(*ktype, KType::Number);
        }
        _ => panic!("expected signature shape [Keyword(\"DOUBLE\"), Argument(x :Number)]"),
    }
}

#[test]
fn fn_typed_param_dispatches_on_matching_call() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (DOUBLE x :Number) -> Number = (x)");
    let result = run_one(scope, parse_one("DOUBLE 7"));
    assert!(matches!(result, KObject::Number(n) if *n == 7.0));
}

/// Mismatched arg: per-slot type check filters out the only candidate, the scope chain
/// runs out, and the queue stalls — surfaces as `DispatchFailed` from `execute()` itself.
#[test]
fn fn_typed_param_rejects_mismatched_call() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (DOUBLE x :Number) -> Number = (x)");
    let mut sched = KoanRuntime::new();
    let root = sched.add_dispatch(parse_one("DOUBLE \"hi\""), scope);
    sched
        .execute()
        .expect("a dispatch failure is slot-terminal, not a fatal execute error");
    let err = sched
        .read_result(root)
        .err()
        .expect("DOUBLE \"hi\" should fail dispatch");
    assert!(
        matches!(&err.kind, KErrorKind::DispatchFailed { .. }),
        "expected DispatchFailed for type-mismatched DOUBLE call, got {err}",
    );
}

/// Two FNs sharing a shape but differing on parameter type both register, and dispatch
/// routes each call to the body whose type signature matches.
#[test]
fn fn_overloads_dispatch_by_param_type() {
    let bytes = capture_program_output(
        "FN (DESCRIBE x :Number) -> Null = (PRINT \"number\")\n\
         FN (DESCRIBE x :Str) -> Null = (PRINT \"string\")\n\
         DESCRIBE 7\n\
         DESCRIBE \"hi\"",
    );
    assert_eq!(bytes, b"number\nstring\n");
}

#[test]
fn fn_param_without_annotation_is_rejected() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let mut sched = KoanRuntime::new();
    let id = sched.add_dispatch(parse_one("FN (DOUBLE x) -> Number = (x)"), scope);
    sched
        .execute()
        .expect("execute does not surface per-slot errors");
    let err = match sched.read_result(id) {
        Err(e) => e,
        Ok(_) => panic!("untyped parameter should error"),
    };
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("`x`")),
        "expected ShapeError mentioning `x`, got {err}",
    );
    assert!(
        !fn_is_registered(scope, "DOUBLE"),
        "DOUBLE should not register"
    );
}

#[test]
fn fn_param_with_unknown_type_name_is_rejected() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let mut sched = KoanRuntime::new();
    let id = sched.add_dispatch(parse_one("FN (DOUBLE x :Bogus) -> Number = (x)"), scope);
    sched
        .execute()
        .expect("execute does not surface per-slot errors");
    let err = match sched.read_result(id) {
        Err(e) => e,
        Ok(_) => panic!("unknown param type should error"),
    };
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("Bogus")),
        "expected ShapeError mentioning `Bogus`, got {err}",
    );
}

/// Commas inside expression frames are stripped, so comma- and whitespace-separated
/// parameter triples are interchangeable.
#[test]
fn fn_comma_separated_typed_params_register() {
    let bytes = capture_program_output(
        "FN (FIRST x :Number, y :Number) -> Number = (x)\n\
         PRINT (FIRST 1 2)",
    );
    assert_eq!(bytes, b"1\n");
}
