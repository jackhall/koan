//! Definition-time return-slot validation. Each case fires at the FUNCTOR
//! binder site; the diagnostic is a `ShapeError` mentioning
//! `FUNCTOR return-type slot`.

use crate::builtins::test_support::{parse_one, run, run_one_err, run_root_silent};
use crate::machine::KErrorKind;
use crate::machine::RuntimeArena;

#[test]
fn functor_return_slot_number_rejects() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
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
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
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

/// `MODULE_TYPE_OF` references a parameter, so the return carrier routes
/// through `ReturnTypeState::Deferred`; the head inspector surfaces the
/// diagnostic without waiting for per-call elaboration.
#[test]
fn functor_return_slot_module_type_of_rejects() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = ((LET Type = Number) (VAL compare :Number))",
    );
    let err = run_one_err(
        scope,
        parse_one("FUNCTOR (USE_TYPE Er :OrderedSig) -> (MODULE_TYPE_OF Er Type) = (1)"),
    );
    match &err.kind {
        KErrorKind::ShapeError(msg) => assert!(
            msg.contains("FUNCTOR return-type slot")
                && (msg.contains("MODULE_TYPE_OF") || msg.contains("abstract type")),
            "expected MODULE_TYPE_OF rejection, got {msg}",
        ),
        _ => panic!("expected ShapeError, got {err}"),
    }
}
