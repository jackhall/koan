//! Typed parameters: dispatch routing on parameter types, overloads, shape errors.

use crate::builtins::test_support::{TestRun, fn_is_registered, lookup_fn, parse_one};
use crate::machine::KErrorKind;
use crate::machine::model::{Argument, KObject, KType, SignatureElement};
use crate::machine::{program_storage, run_root_storage};

use super::capture_program_output;
use crate::machine::model::Symbol;

#[test]
fn fn_typed_param_records_ktype_on_signature() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("FN (DOUBLE x :Number) -> Number = (x)");

    let f = lookup_fn(scope, "DOUBLE");
    match f.signature.elements() {
        [
            SignatureElement::Keyword(kw),
            SignatureElement::Argument(Argument { name, ktype }),
        ] => {
            assert_eq!(kw.text(), "DOUBLE");
            assert_eq!(name.symbol(), Symbol::of("x"));
            assert_eq!(*ktype, KType::NUMBER);
        }
        _ => panic!("expected signature shape [Keyword(\"DOUBLE\"), Argument(x :Number)]"),
    }
}

#[test]
fn fn_typed_param_dispatches_on_matching_call() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("FN (DOUBLE x :Number) -> Number = (x)");
    let result = test_run.run_one(parse_one(&program, "DOUBLE 7"));
    assert!(matches!(result, KObject::Number(n) if *n == 7.0));
}

/// Mismatched arg: per-slot type check filters out the only candidate, the scope chain
/// runs out, and the queue stalls — surfaces as `DispatchFailed` from `execute()` itself.
#[test]
fn fn_typed_param_rejects_mismatched_call() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("FN (DOUBLE x :Number) -> Number = (x)");
    let root = test_run.dispatch_in_scope(
        crate::machine::model::WorkingExpression::from_ast(
            scope.brand(),
            parse_one(&program, "DOUBLE \"hi\""),
        ),
        scope,
    );
    let edge = test_run.runtime.install_edge_for_test(root, scope);
    test_run
        .runtime
        .execute()
        .expect("a dispatch failure is slot-terminal, not a fatal execute error");
    let err = test_run
        .runtime
        .edge_result_error(edge)
        .expect_err("DOUBLE \"hi\" should fail dispatch");
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
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let id = test_run.dispatch_in_scope(
        crate::machine::model::WorkingExpression::from_ast(
            scope.brand(),
            parse_one(&program, "FN (DOUBLE x) -> Number = (x)"),
        ),
        scope,
    );
    let edge = test_run.runtime.install_edge_for_test(id, scope);
    test_run
        .runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let err = match test_run.runtime.edge_result_error(edge) {
        Err(e) => e,
        Ok(()) => panic!("untyped parameter should error"),
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
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let id = test_run.dispatch_in_scope(
        crate::machine::model::WorkingExpression::from_ast(
            scope.brand(),
            parse_one(&program, "FN (DOUBLE x :Bogus) -> Number = (x)"),
        ),
        scope,
    );
    let edge = test_run.runtime.install_edge_for_test(id, scope);
    test_run
        .runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let err = match test_run.runtime.edge_result_error(edge) {
        Err(e) => e,
        Ok(()) => panic!("unknown param type should error"),
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
