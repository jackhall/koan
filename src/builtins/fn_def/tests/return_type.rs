//! Parsing the `-> Type` slot, and the runtime return-type check.

use crate::builtins::test_support::{fn_is_registered, lookup_fn, parse_one, TestRun};
use crate::machine::model::{KObject, KType, ReturnType};
use crate::machine::run_root_storage;
use crate::machine::KErrorKind;
use crate::parse::parse;

#[test]
fn fn_parses_declared_return_type_onto_signature() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run("FN (DOUBLE x :Number) -> Number = (x)");

    let f = lookup_fn(scope, "DOUBLE");
    let ReturnType::Resolved(kt) = &f.signature.return_type else {
        panic!("declared return type should land resolved on the signature");
    };
    assert_eq!(*kt, KType::Number);
}

/// Missing `-> Type`: the FN call doesn't match the registered signature, so no user-fn
/// gets bound. Sub-expression dispatch may error first depending on body shape — the
/// load-bearing assertion is that `DOUBLE` isn't registered.
#[test]
fn fn_without_return_type_annotation_does_not_register() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let exprs = parse("FN (DOUBLE x :Number) = (PRINT \"x\")").expect("parse should succeed");
    for expr in exprs {
        test_run.runtime.dispatch_in_scope(expr, scope);
    }
    let _ = test_run.runtime.execute();
    assert!(
        !fn_is_registered(scope, "DOUBLE"),
        "DOUBLE should not be registered without -> Type"
    );
}

/// Dispatch never selects on the return type, so two overloads whose shape and argument
/// types agree are indistinguishable at every call site no matter how their returns
/// differ — the second definition is a duplicate, not a new overload.
#[test]
fn return_type_only_difference_is_a_duplicate_overload() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run
        .runtime
        .dispatch_in_scope(parse_one("FN (DOUBLE x :Number) -> Number = (x)"), scope);
    let id = test_run
        .runtime
        .dispatch_in_scope(parse_one("FN (DOUBLE x :Number) -> Str = (\"a\")"), scope);
    test_run
        .runtime
        .execute()
        .expect("execute does not surface per-node errors");
    let err = match test_run.runtime.result_error(id) {
        Err(e) => e,
        Ok(()) => panic!("return-type-only overload should be rejected as a duplicate"),
    };
    assert!(
        matches!(err.kind, KErrorKind::DuplicateOverload { ref name, .. } if name == "DOUBLE"),
        "expected DuplicateOverload for DOUBLE, got {err}",
    );
}

#[test]
fn fn_with_unknown_return_type_name_errors() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let id = test_run
        .runtime
        .dispatch_in_scope(parse_one("FN (DOUBLE x :Number) -> Bogus = (x)"), scope);
    test_run
        .runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let err = match test_run.runtime.result_error(id) {
        Err(e) => e,
        Ok(()) => panic!("unknown type name should error"),
    };
    assert!(
        matches!(err.kind, KErrorKind::ShapeError(ref msg) if msg.contains("Bogus")),
        "expected ShapeError mentioning 'Bogus', got {err}",
    );
}

#[test]
fn user_fn_return_type_mismatch_surfaces_as_kerror() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run("FN (LIE) -> Number = (\"oops\")");
    let id = test_run.runtime.dispatch_in_scope(parse_one("LIE"), scope);
    test_run
        .runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let err = match test_run.runtime.result_error(id) {
        Err(e) => e,
        Ok(()) => panic!("LIE should fail return-type check"),
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

/// Forward reference: FN's body parks on `MyT`'s submit-time placeholder via dep-finish
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
/// see [ktype/slots-and-signatures.md § TypeNameRef](../../../../design/typing/ktype/slots-and-signatures.md#ktypeunresolved--surface-form-survives-bind).
#[test]
fn fn_return_type_surface_name_preserved_in_error() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let id = test_run
        .runtime
        .dispatch_in_scope(parse_one("FN (DOIT) -> SomeWeirdName = (1)"), scope);
    test_run
        .runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let err = match test_run.runtime.result_error(id) {
        Err(e) => e,
        Ok(()) => panic!("unknown type name should error"),
    };
    assert!(
        matches!(err.kind, KErrorKind::ShapeError(ref msg) if msg.contains("SomeWeirdName")),
        "expected ShapeError mentioning 'SomeWeirdName' verbatim, got {err}",
    );
}

#[test]
fn user_fn_with_any_return_type_accepts_anything() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    test_run.run("FN (PURE) -> Any = (\"a string\")");
    let result = test_run.run_one(parse_one("PURE"));
    assert!(matches!(result, KObject::KString(s) if s == "a string"));
}

/// Keep-first across a cross-function tail chain: `OUTER`'s declared `-> Number` governs the whole
/// chain, so a violation introduced only by the chain's *final* tail value still errors against
/// `OUTER`'s contract — and the error's trace label names `OUTER` (the first call), not the inner
/// callee that produced the offending value. `MIDDLE` and `INNER` declare `-> Any` (FN registration
/// requires a `-> Type`), so their own contracts would *accept* the `Str`; the mismatch fires only
/// because keep-first keeps `OUTER`'s `-> Number` across both hops (`OUTER -> MIDDLE -> INNER`) and
/// carries its precomputed label. This exercises the invoke-continue/redispatch keep-first over a
/// two-deep cross-function chain, not self-recursion.
#[test]
fn keep_first_across_tail_chain_errors_against_outer_contract() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run("FN (INNER) -> Any = (\"nope\")");
    test_run.run("FN (MIDDLE) -> Any = (INNER)");
    test_run.run("FN (OUTER) -> Number = (MIDDLE)");
    let id = test_run
        .runtime
        .dispatch_in_scope(parse_one("OUTER"), scope);
    test_run
        .runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let err = match test_run.runtime.result_error(id) {
        Err(e) => e,
        Ok(()) => {
            panic!(
                "OUTER should fail: the chain's final tail returns a Str against OUTER's -> Number"
            )
        }
    };
    match &err.kind {
        KErrorKind::TypeMismatch { arg, expected, got } => {
            assert_eq!(arg, "<return>");
            assert_eq!(
                expected, "Number",
                "the kept-first contract is OUTER's -> Number, not the callees' -> Any",
            );
            assert_eq!(got, "Str");
        }
        _ => panic!("expected TypeMismatch on <return>, got {err}"),
    }
    assert!(
        err.frames.iter().any(|f| f.function.contains("OUTER")),
        "the kept-first contract's precomputed trace label names OUTER (the first call), got {:?}",
        err.frames.iter().map(|f| &f.function).collect::<Vec<_>>(),
    );
}

/// A tail-spliced declared-return obligation is discharged before any consumer reads the rehomed
/// terminal. `WRAP`'s body tail is a bare name (`x`) that forward-references a name defined lexically
/// later, so `x` is still a submit-time placeholder when the body decides: the slot splices out via
/// `Outcome::Forward` (an already-*bound* name would read as a plain `Done`, never a forward) rather
/// than parking a continuation. `WRAP`'s `-> Number` obligation rides the splice, so before the
/// forwarded terminal reaches the `out` consumer the checker discharges the declared return against
/// the producer's value — here through the parked-checker micro-step, since a forward-referenced
/// producer is unresolved when the consumer decides. A non-matching `Str` fires the mismatch at the
/// splice check; a matching `Number` forwards through intact.
#[test]
fn spliced_bare_name_tail_checks_declared_return() {
    // Non-matching: the bare-name tail forwards a Str; the splice check rejects it against -> Number.
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let bad_ids: Vec<_> = parse("FN (WRAP) -> Number = (x)\nLET out = (WRAP)\nLET x = \"nope\"")
        .expect("parse succeeds")
        .into_iter()
        .map(|e| test_run.runtime.dispatch_in_scope(e, scope))
        .collect();
    test_run
        .runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let err = match test_run.runtime.result_error(bad_ids[1]) {
        Err(e) => e,
        Ok(()) => panic!("the spliced Str tail must fail WRAP's -> Number check"),
    };
    match &err.kind {
        KErrorKind::TypeMismatch { arg, expected, got } => {
            assert_eq!(arg, "<return>");
            assert_eq!(expected, "Number");
            assert_eq!(got, "Str");
        }
        _ => panic!("expected TypeMismatch on <return> from the splice check, got {err}"),
    }
    assert!(
        err.frames.iter().any(|f| f.function.contains("WRAP")),
        "the splice check labels the mismatch with the obligation's FN (WRAP), got {:?}",
        err.frames.iter().map(|f| &f.function).collect::<Vec<_>>(),
    );

    // Matching: the bare-name tail forwards a Number; the splice check passes and the value arrives.
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let ok_ids: Vec<_> = parse("FN (WRAP) -> Number = (x)\nLET out = (WRAP)\nLET x = 7")
        .expect("parse succeeds")
        .into_iter()
        .map(|e| test_run.runtime.dispatch_in_scope(e, scope))
        .collect();
    test_run
        .runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    assert!(
        test_run.runtime.result_error(ok_ids[1]).is_ok(),
        "the matching spliced value passes the splice check: {:?}",
        test_run.runtime.result_error(ok_ids[1]).err(),
    );
    assert!(
        matches!(scope.lookup("out"), Some(KObject::Number(n)) if *n == 7.0),
        "the matching spliced value forwards through intact to out",
    );
}
