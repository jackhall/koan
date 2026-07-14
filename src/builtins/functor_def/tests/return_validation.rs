//! Definition-time return-slot validation. Each case fires at the FUNCTOR
//! binder site; the diagnostic is a `ShapeError` mentioning
//! `FUNCTOR return-type slot`.

use crate::builtins::test_support::{parse_one, run, run_one_err, run_root_silent};
use crate::machine::core::run_root_storage;
use crate::machine::KErrorKind;

#[test]
fn functor_return_slot_number_rejects() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let err = run_one_err(
        scope,
        parse_one("FUNCTOR (MAKEN x :Number) -> Number = (1)"),
    );
    match &err.kind {
        KErrorKind::ShapeError(msg) => assert!(
            msg.contains("FUNCTOR return-type slot"),
            "expected FUNCTOR return-type diagnostic, got {msg}",
        ),
        _ => panic!("expected ShapeError, got {err}"),
    }
}

#[test]
fn functor_return_slot_function_type_rejects() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let err = run_one_err(
        scope,
        parse_one(
            "FUNCTOR (MAKEFN x :Number) -> :(FN (y :Number) -> Number) = \
                (FN (INNER y :Number) -> Number = (y))",
        ),
    );
    match &err.kind {
        KErrorKind::ShapeError(msg) => assert!(
            msg.contains("FUNCTOR return-type slot"),
            "expected FUNCTOR return-type diagnostic, got {msg}",
        ),
        _ => panic!("expected ShapeError, got {err}"),
    }
}

/// the dotted `er.Type` access references a parameter, so the return carrier routes
/// through `ReturnTypeState::Deferred`; the head inspector surfaces the
/// diagnostic without waiting for per-call elaboration.
#[test]
fn functor_return_slot_dotted_type_member_rejects() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG Ordered = ((TYPE Carrier) (VAL compare :Number))",
    );
    let err = run_one_err(
        scope,
        parse_one("FUNCTOR (USE_TYPE er :Ordered) -> er.Type = (1)"),
    );
    match &err.kind {
        KErrorKind::ShapeError(msg) => assert!(
            msg.contains("FUNCTOR return-type slot") && msg.contains("abstract type"),
            "expected dotted-type-member rejection, got {msg}",
        ),
        _ => panic!("expected ShapeError, got {err}"),
    }
}
