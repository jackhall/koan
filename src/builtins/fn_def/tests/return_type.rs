//! Parsing the `-> Type` slot, and the runtime return-type check.

use crate::builtins::test_support::{
    fn_is_registered, lookup_fn, parse_one, run, run_one, run_root_silent,
};
use crate::machine::execute::KoanHarness;
use crate::machine::model::{KObject, KType, ReturnType};
use crate::machine::{KErrorKind, RuntimeArena};
use crate::parse::parse;

#[test]
fn fn_parses_declared_return_type_onto_signature() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (DOUBLE x :Number) -> Number = (x)");

    let f = lookup_fn(scope, "DOUBLE");
    assert_eq!(f.signature.return_type, ReturnType::Resolved(KType::Number));
}

/// Missing `-> Type`: the FN call doesn't match the registered signature, so no user-fn
/// gets bound. Sub-expression dispatch may error first depending on body shape — the
/// load-bearing assertion is that `DOUBLE` isn't registered.
#[test]
fn fn_without_return_type_annotation_does_not_register() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let exprs = parse("FN (DOUBLE x :Number) = (PRINT \"x\")").expect("parse should succeed");
    let mut sched = KoanHarness::new();
    for expr in exprs {
        sched.add_dispatch(expr, scope);
    }
    let _ = sched.execute();
    assert!(
        !fn_is_registered(scope, "DOUBLE"),
        "DOUBLE should not be registered without -> Type"
    );
}

#[test]
fn fn_with_unknown_return_type_name_errors() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let mut sched = KoanHarness::new();
    let id = sched.add_dispatch(parse_one("FN (DOUBLE x :Number) -> Bogus = (x)"), scope);
    sched
        .execute()
        .expect("execute does not surface per-slot errors");
    let err = match sched.read_result(id) {
        Err(e) => e,
        Ok(_) => panic!("unknown type name should error"),
    };
    assert!(
        matches!(err.kind, KErrorKind::ShapeError(ref msg) if msg.contains("Bogus")),
        "expected ShapeError mentioning 'Bogus', got {err}",
    );
}

#[test]
fn user_fn_return_type_mismatch_surfaces_as_kerror() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (LIE) -> Number = (\"oops\")");
    let mut sched = KoanHarness::new();
    let id = sched.add_dispatch(parse_one("LIE"), scope);
    sched
        .execute()
        .expect("execute does not surface per-slot errors");
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

/// User-bound type alias as a FN return type: elaborates against the captured scope.
#[test]
fn fn_with_user_bound_return_type_works() {
    use super::capture_program_output;
    let bytes = capture_program_output(
        "LET MyT = Number\n\
         FN (DOIT xs :MyT) -> MyT = (xs)\n\
         PRINT (DOIT 7)",
    );
    assert_eq!(bytes, b"7\n");
}

/// Forward reference: FN's body parks on `MyT`'s submit-time placeholder via Combine
/// and re-elaborates against the final scope when the LET wakes.
#[test]
fn fn_with_forward_user_bound_return_type_works() {
    use super::capture_program_output;
    let bytes = capture_program_output(
        "FN (DOIT xs :MyT) -> MyT = (xs)\n\
         LET MyT = Number\n\
         PRINT (DOIT 7)",
    );
    assert_eq!(bytes, b"7\n");
}

/// Pins the surface-form-survives-bind guarantee on `KObject::TypeNameRef` —
/// see [ktype.md § TypeNameRef](../../../../design/typing/ktype.md#typenameref--surface-form-survives-bind).
#[test]
fn fn_return_type_surface_name_preserved_in_error() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let mut sched = KoanHarness::new();
    let id = sched.add_dispatch(parse_one("FN (DOIT) -> SomeWeirdName = (1)"), scope);
    sched
        .execute()
        .expect("execute does not surface per-slot errors");
    let err = match sched.read_result(id) {
        Err(e) => e,
        Ok(_) => panic!("unknown type name should error"),
    };
    assert!(
        matches!(err.kind, KErrorKind::ShapeError(ref msg) if msg.contains("SomeWeirdName")),
        "expected ShapeError mentioning 'SomeWeirdName' verbatim, got {err}",
    );
}

#[test]
fn user_fn_with_any_return_type_accepts_anything() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (PURE) -> Any = (\"a string\")");
    let result = run_one(scope, parse_one("PURE"));
    assert!(matches!(result, KObject::KString(s) if s == "a string"));
}
