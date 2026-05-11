//! Parsing the `-> Type` slot, and the runtime return-type check.

use crate::dispatch::builtins::test_support::{parse_one, run, run_one, run_root_silent};
use crate::dispatch::runtime::{KErrorKind, RuntimeArena};
use crate::dispatch::types::KType;
use crate::dispatch::values::KObject;
use crate::execute::scheduler::Scheduler;
use crate::parse::expression_tree::parse;

/// `FN` parses the declared return type from the `-> Type` slot and stores it on the
/// registered function's signature.
#[test]
fn fn_parses_declared_return_type_onto_signature() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (DOUBLE x: Number) -> Number = (x)");

    let data = scope.data.borrow();
    let entry = data.get("DOUBLE").expect("DOUBLE should be bound");
    let f = match entry {
        KObject::KFunction(f, _) => *f,
        _ => panic!("expected DOUBLE to bind a KFunction"),
    };
    assert_eq!(f.signature.return_type, KType::Number);
}

/// Missing `-> Type` annotation: the FN call doesn't match the registered signature, so
/// no user-fn gets bound. (Sub-expression dispatch may also error first depending on body
/// shape — the load-bearing assertion is that DOUBLE isn't bound.)
#[test]
fn fn_without_return_type_annotation_does_not_register() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let exprs = parse("FN (DOUBLE x: Number) = (PRINT \"x\")").expect("parse should succeed");
    let mut sched = Scheduler::new();
    for expr in exprs {
        sched.add_dispatch(expr, scope);
    }
    let _ = sched.execute(); // ignore: may or may not error depending on which sub fails first
    let data = scope.data.borrow();
    assert!(data.get("DOUBLE").is_none(), "DOUBLE should not be registered without -> Type");
}

/// Unknown type name in the return slot surfaces as a `ShapeError`.
#[test]
fn fn_with_unknown_return_type_name_errors() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(parse_one("FN (DOUBLE x: Number) -> Bogus = (x)"), scope);
    sched.execute().expect("execute does not surface per-slot errors");
    let err = match sched.read_result(id) {
        Err(e) => e,
        Ok(_) => panic!("unknown type name should error"),
    };
    assert!(
        matches!(err.kind, KErrorKind::ShapeError(ref msg) if msg.contains("Bogus")),
        "expected ShapeError mentioning 'Bogus', got {err}",
    );
}

/// Runtime return-type check fires when the body produces a value of the wrong type.
#[test]
fn user_fn_return_type_mismatch_surfaces_as_kerror() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (LIE) -> Number = (\"oops\")");
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(parse_one("LIE"), scope);
    sched.execute().expect("execute does not surface per-slot errors");
    let err = match sched.read_result(id) {
        Err(e) => e,
        Ok(_) => panic!("LIE should fail return-type check"),
    };
    match &err.kind {
        KErrorKind::TypeMismatch { arg, expected, got } => {
            assert_eq!(arg, "<return>");
            assert_eq!(expected, "Number");
            assert_eq!(got, "Str");
        }
        _ => panic!("expected TypeMismatch on <return>, got {err}"),
    }
    assert!(
        err.frames.iter().any(|f| f.function.contains("LIE")),
        "expected a frame mentioning LIE, got {:?}",
        err.frames.iter().map(|f| &f.function).collect::<Vec<_>>(),
    );
}

/// `Any` return type is the no-op fast path: any body value satisfies it.
#[test]
fn user_fn_with_any_return_type_accepts_anything() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (PURE) -> Any = (\"a string\")");
    let result = run_one(scope, parse_one("PURE"));
    assert!(matches!(result, KObject::KString(s) if s == "a string"));
}
