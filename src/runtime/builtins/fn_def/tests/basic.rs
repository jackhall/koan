//! Basic FN registration, dispatch, and parameter substitution.

use crate::runtime::builtins::test_support::{parse_one, run, run_one, run_root_silent};
use crate::runtime::model::{KObject, SignatureElement};
use crate::runtime::machine::RuntimeArena;

use super::capture_program_output;

/// Smoke test for FN's pre_run extractor: pulls the first Keyword out of the
/// signature Expression at `parts[1]` (FN's name slot is *inside* the signature,
/// not at `parts[1]` directly).
#[test]
fn pre_run_extracts_first_keyword_of_signature() {
    let expr = parse_one("FN (DOUBLE x: Number) -> Number = (x)");
    let name = crate::runtime::builtins::fn_def::pre_run(&expr);
    assert_eq!(name.as_deref(), Some("DOUBLE"));
}

#[test]
fn fn_registers_user_function_under_keyword_signature() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (GREET) -> Null = (PRINT \"hi\")");

    let data = scope.data.borrow();
    let entry = data.get("GREET").expect("GREET should be bound");
    let f = match entry {
        KObject::KFunction(f, _) => *f,
        _ => panic!("expected GREET to bind a KFunction"),
    };
    match f.signature.elements.as_slice() {
        [SignatureElement::Keyword(s)] => assert_eq!(s, "GREET"),
        _ => panic!("expected single-Keyword signature [Keyword(\"GREET\")]"),
    }
}

#[test]
fn fn_call_dispatches_body_at_call_time() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "LET x = 42\nFN (GETX) -> Number = (x)");

    let result = run_one(scope, parse_one("GETX"));
    assert!(matches!(result, KObject::Number(n) if *n == 42.0),
        "GETX should return the value bound to x at call time");
}

#[test]
fn fn_rejects_non_keyword_name() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (greet) -> Null = (PRINT \"hi\")");
    let data = scope.data.borrow();
    assert!(data.get("greet").is_none());
    assert!(data.get("GREET").is_none());
}

#[test]
fn fn_call_runs_body_each_time() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "LET x = 7\nFN (GETX) -> Number = (x)");

    for _ in 0..2 {
        let result = run_one(scope, parse_one("GETX"));
        assert!(matches!(result, KObject::Number(n) if *n == 7.0));
    }
}

#[test]
fn fn_body_with_nested_expression_evaluates() {
    let bytes = capture_program_output(
        "LET msg = \"from outer scope\"\n\
         FN (SAY) -> Null = (PRINT (msg))\n\
         SAY",
    );
    assert_eq!(bytes, b"from outer scope\n");
}

#[test]
fn user_fn_calls_user_fn_transitively() {
    let bytes = capture_program_output(
        "FN (BAR) -> Null = (PRINT \"ok\")\n\
         FN (FOO) -> Null = (BAR)\n\
         FOO",
    );
    assert_eq!(bytes, b"ok\n");
}

#[test]
fn calling_user_fn_repeatedly_runs_body_each_time() {
    let bytes = capture_program_output(
        "FN (GREET) -> Null = (PRINT \"hello world\")\n\
         GREET\n\
         GREET",
    );
    assert_eq!(bytes, b"hello world\nhello world\n");
}

#[test]
fn fn_with_single_param_substitutes_at_call_site() {
    let bytes = capture_program_output(
        "FN (SAY x: Str) -> Null = (PRINT x)\n\
         SAY \"hello\"",
    );
    assert_eq!(bytes, b"hello\n");
}

#[test]
fn fn_with_two_params_binds_each_by_name() {
    let bytes = capture_program_output(
        "FN (FIRST x: Str y: Str) -> Null = (PRINT x)\n\
         FIRST \"one\" \"two\"",
    );
    assert_eq!(bytes, b"one\n");
}

#[test]
fn fn_with_infix_shape_dispatches_on_keyword_position() {
    let bytes = capture_program_output(
        "FN (a: Str SAID) -> Null = (PRINT a)\n\
         \"hi\" SAID",
    );
    assert_eq!(bytes, b"hi\n");
}

#[test]
fn fn_param_shadows_outer_binding_at_call_site() {
    let bytes = capture_program_output(
        "LET msg = \"outer\"\n\
         FN (SAY msg: Str) -> Null = (PRINT msg)\n\
         SAY \"param wins\"",
    );
    assert_eq!(bytes, b"param wins\n");
}

#[test]
fn fn_param_substitutes_inside_nested_subexpression() {
    let bytes = capture_program_output(
        "FN (WRAP x: Str) -> Null = (PRINT (x))\n\
         WRAP \"wrapped\"",
    );
    assert_eq!(bytes, b"wrapped\n");
}

#[test]
fn fn_returns_param_value_directly() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (ECHO v: Number) -> Number = (v)");

    let result = run_one(scope, parse_one("ECHO 7"));
    assert!(matches!(result, KObject::Number(n) if *n == 7.0));
}

#[test]
fn fn_signature_with_no_keyword_is_rejected() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (x: Number) -> Null = (PRINT \"oops\")");
    let data = scope.data.borrow();
    assert!(data.get("x").is_none());
}

/// `FN` returns the `KObject::KFunction` it just registered, so callers can capture a
/// callable handle via `LET f = (FN ...)`. Calling the captured handle is tested in
/// [`call_by_name`](crate::runtime::builtins::call_by_name).
#[test]
fn fn_def_returns_the_registered_kfunction() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let result = run_one(scope, parse_one("FN (DOUBLE x: Number) -> Number = (x)"));
    assert!(
        matches!(result, KObject::KFunction(_, _)),
        "FN should return its registered KFunction",
    );
}
