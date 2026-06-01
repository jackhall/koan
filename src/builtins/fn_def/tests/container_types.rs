//! Parameterized container types in FN parameter and return slots:
//! `List<T>`, `Dict<K, V>`, `Function<…>`, plus specificity tournaments.

use crate::builtins::test_support::{parse_one, run, run_one, run_root_silent};
use crate::machine::RuntimeArena;
use crate::machine::core::KErrorKind;
use crate::machine::execute::Scheduler;
use crate::machine::model::types::KType;
use crate::parse::parse;

use super::capture_program_output;

/// A `List<Number>` body returned through `:(LIST OF Any)` re-tags the carrier to
/// exactly the declared return type — coarsening — so the result's `ktype()` reports
/// `List<Any>`, the contract, not the body's incidental `List<Number>` precision.
#[test]
fn fn_return_coarsens_list_carrier_to_declared() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (NUMS) -> :(LIST OF Any) = ([1 2 3])");
    let result = run_one(scope, parse_one("NUMS"));
    assert_eq!(result.ktype(), KType::List(Box::new(KType::Any)));
}

/// Without an annotation, a list keeps its precise memoized join type.
#[test]
fn fn_return_keeps_precise_list_carrier_when_declared_precise() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (NUMS) -> :(LIST OF Number) = ([1 2 3])");
    let result = run_one(scope, parse_one("NUMS"));
    assert_eq!(result.ktype(), KType::List(Box::new(KType::Number)));
}

/// A heterogeneous literal carries `List<Any>` and does not satisfy a precise
/// declared element type.
#[test]
fn fn_return_heterogeneous_list_rejected_by_precise_declared() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (BAD) -> :(LIST OF Number) = ([2 \"hello\"])");
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(parse_one("BAD"), scope);
    sched.execute().expect("scheduler runs to completion");
    assert!(sched.read_result(id).is_err());
}

/// Empty container through an annotated return boundary: the vacuous `matches_value`
/// passes and the declared element type is stamped.
#[test]
fn fn_return_empty_list_stamps_declared_element_type() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (EMPTY) -> :(LIST OF Number) = ([])");
    let result = run_one(scope, parse_one("EMPTY"));
    assert_eq!(result.ktype(), KType::List(Box::new(KType::Number)));
}

#[test]
fn fn_with_typed_list_param_accepts_matching_list() {
    let bytes = capture_program_output(
        "FN (HEAD xs :(LIST OF Number)) -> Number = (1)\n\
         PRINT (HEAD [1 2 3])",
    );
    assert_eq!(bytes, b"1\n");
}

#[test]
fn fn_returning_typed_list_accepts_matching_value() {
    let bytes = capture_program_output(
        "FN (NUMS) -> :(LIST OF Number) = ([1 2 3])\n\
         PRINT (NUMS)",
    );
    assert_eq!(bytes, b"[1, 2, 3]\n");
}

/// The scheduler stores the return-type-check error in the result slot rather than
/// failing `execute`, so we read the slot via `read_result` to assert the failure.
#[test]
fn fn_returning_typed_list_rejects_wrong_element_type() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (BAD) -> :(LIST OF Number) = ([1 \"x\"])");
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(parse_one("BAD"), scope);
    sched.execute().expect("scheduler runs to completion");
    let res = sched.read_result(id);
    assert!(
        res.is_err(),
        "expected return-type mismatch when body produces :(LIST OF Any) for declared :(LIST OF Number)"
    );
}

#[test]
fn fn_with_invalid_list_arity_errors_at_definition() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let mut sched = Scheduler::new();
    let exprs = parse("FN (BAD xs :(List Number, Str)) -> Null = (xs)").expect("parse ok");
    let mut ids = Vec::new();
    for e in exprs {
        ids.push(sched.add_dispatch(e, scope));
    }
    sched.execute().expect("scheduler runs");
    assert!(
        ids.iter().any(|id| sched.read_result(*id).is_err()),
        "FN definition with `:(List Number, Str)` should fail with an arity error"
    );
}

#[test]
fn fn_with_typed_dict_param_accepts_matching_dict() {
    let bytes = capture_program_output(
        "FN (SIZE d :(MAP Str -> Number)) -> Number = (1)\n\
         PRINT (SIZE {\"a\": 1, \"b\": 2})",
    );
    assert_eq!(bytes, b"1\n");
}

/// Inline FN expression side-steps having to dereference an identifier-bound
/// function for the `function_compat` check.
#[test]
fn fn_with_typed_function_param_accepts_matching_function() {
    let bytes = capture_program_output(
        "FN (USE f :(FN (x :Number) -> Str)) -> Str = (\"got fn\")\n\
         PRINT (USE (FN (SHOW x :Number) -> Str = (\"hi\")))",
    );
    assert_eq!(bytes, b"got fn\n");
}

/// When two overloads share the same untyped shape and both match, the more
/// specific one wins: `(xs: List<Number>)` over `(xs: List<Any>)` for a number-list
/// call.
#[test]
fn dispatch_picks_more_specific_list_overload() {
    let bytes = capture_program_output(
        "FN (PICK xs :(LIST OF Any)) -> Str = (\"any\")\n\
         FN (PICK xs :(LIST OF Number)) -> Str = (\"number\")\n\
         PRINT (PICK [1 2 3])",
    );
    assert_eq!(bytes, b"number\n");
}

/// Element-only-differing overloads (`:(LIST OF Number)` vs `:(LIST OF Str)`) tie
/// on shape at first dispatch and defer; once the literal evaluates, the
/// element-aware re-dispatch picks the satisfying overload.
#[test]
fn dispatch_disambiguates_element_only_overloads_on_literal() {
    let numbers = capture_program_output(
        "FN (DESCRIBE xs :(LIST OF Number)) -> Str = (\"numbers\")\n\
         FN (DESCRIBE xs :(LIST OF Str)) -> Str = (\"strings\")\n\
         PRINT (DESCRIBE [1 2 3])",
    );
    assert_eq!(numbers, b"numbers\n");
    let strings = capture_program_output(
        "FN (DESCRIBE xs :(LIST OF Number)) -> Str = (\"numbers\")\n\
         FN (DESCRIBE xs :(LIST OF Str)) -> Str = (\"strings\")\n\
         PRINT (DESCRIBE [\"a\" \"b\"])",
    );
    assert_eq!(strings, b"strings\n");
}

/// An already-evaluated container forced through parens arrives at re-dispatch as a
/// typed `Future` whose carried element type selects the overload — no literal-defer
/// needed.
#[test]
fn dispatch_disambiguates_element_only_overloads_on_evaluated_arg() {
    let bytes = capture_program_output(
        "FN (DESCRIBE xs :(LIST OF Number)) -> Str = (\"numbers\")\n\
         FN (DESCRIBE xs :(LIST OF Str)) -> Str = (\"strings\")\n\
         LET xs = [1 2 3]\n\
         PRINT (DESCRIBE (xs))",
    );
    assert_eq!(bytes, b"numbers\n");
}

/// A bare variable disambiguates in the first dispatch pass via the strict-pass
/// type peek: the bound value's carried element type admits exactly one overload.
#[test]
fn dispatch_disambiguates_element_only_overloads_on_bare_variable() {
    let numbers = capture_program_output(
        "FN (DESCRIBE xs :(LIST OF Number)) -> Str = (\"numbers\")\n\
         FN (DESCRIBE xs :(LIST OF Str)) -> Str = (\"strings\")\n\
         LET xs = [1 2 3]\n\
         PRINT (DESCRIBE xs)",
    );
    assert_eq!(numbers, b"numbers\n");
    let strings = capture_program_output(
        "FN (DESCRIBE xs :(LIST OF Number)) -> Str = (\"numbers\")\n\
         FN (DESCRIBE xs :(LIST OF Str)) -> Str = (\"strings\")\n\
         LET xs = [\"a\" \"b\"]\n\
         PRINT (DESCRIBE xs)",
    );
    assert_eq!(strings, b"strings\n");
}

/// A backward-referenced bare variable disambiguates via the strict-pass peek
/// through gated `resolve_with_chain`. Forward references don't apply under
/// index-gated resolution — a later-sibling `LET xs = …` is invisible to the
/// consumer (LET is value-style gated).
#[test]
fn dispatch_disambiguates_element_only_overloads_on_bound_variable() {
    let bytes = capture_program_output(
        "FN (DESCRIBE xs :(LIST OF Number)) -> Str = (\"numbers\")\n\
         FN (DESCRIBE xs :(LIST OF Str)) -> Str = (\"strings\")\n\
         LET xs = [1 2 3]\n\
         LET y = (DESCRIBE xs)\n\
         PRINT y",
    );
    assert_eq!(bytes, b"numbers\n");
}

/// A genuinely unbound name (no binder anywhere, so no placeholder to park on)
/// across a tentative tie surfaces as the precise `UnboundName` rather than a
/// generic `DispatchFailed`.
#[test]
fn dispatch_unbound_name_across_tied_overloads_is_unbound_error() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (DESCRIBE xs :(LIST OF Number)) -> Str = (\"numbers\")");
    run(scope, "FN (DESCRIBE xs :(LIST OF Str)) -> Str = (\"strings\")");
    let mut sched = Scheduler::new();
    sched.add_dispatch(parse_one("DESCRIBE nope"), scope);
    let error = sched
        .execute()
        .expect_err("an unbound name across tied overloads must error");
    assert!(
        matches!(error.kind, KErrorKind::UnboundName(ref n) if n == "nope"),
        "expected UnboundName(\"nope\"), got {error:?}",
    );
}

/// A heterogeneous literal memoizes `List<Any>`, which satisfies neither concrete
/// overload — the post-eval re-dispatch finds no match (`DispatchFailed`) rather
/// than tying as ambiguous.
#[test]
fn dispatch_heterogeneous_literal_matches_no_concrete_element_overload() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (DESCRIBE xs :(LIST OF Number)) -> Str = (\"numbers\")");
    run(scope, "FN (DESCRIBE xs :(LIST OF Str)) -> Str = (\"strings\")");
    let mut sched = Scheduler::new();
    sched.add_dispatch(parse_one("DESCRIBE [1 \"a\"]"), scope);
    let error = sched
        .execute()
        .expect_err("heterogeneous List<Any> must match no concrete-element overload");
    assert!(
        matches!(error.kind, KErrorKind::DispatchFailed { .. }),
        "expected DispatchFailed, got {error:?}",
    );
}

/// A `:(LIST OF Any)` overload catches the heterogeneous list the concrete-element
/// overloads reject.
#[test]
fn dispatch_list_any_overload_catches_heterogeneous_literal() {
    let bytes = capture_program_output(
        "FN (DESCRIBE xs :(LIST OF Number)) -> Str = (\"numbers\")\n\
         FN (DESCRIBE xs :(LIST OF Any)) -> Str = (\"any\")\n\
         PRINT (DESCRIBE [1 \"a\"])",
    );
    assert_eq!(bytes, b"any\n");
}

/// Parens-wrapped FN parameter type schedules the inner expression as a
/// sub-Dispatch from `parse_fn_param_list`, splices the resulting `KTypeValue` back
/// into the signature, and finalizes the FN with the elaborated type.
#[test]
fn fn_with_parens_wrapped_list_of_param_accepts_matching_list() {
    let bytes = capture_program_output(
        "FN (HEAD xs (LIST_OF Number)) -> Number = (1)\n\
         PRINT (HEAD [1 2 3])",
    );
    assert_eq!(bytes, b"1\n");
}

#[test]
fn fn_with_nested_parens_wrapped_type_param_dispatches() {
    let bytes = capture_program_output(
        "FN (HEAD xs (LIST_OF (LIST_OF Number))) -> Number = (1)\n\
         PRINT (HEAD [[1 2] [3]])",
    );
    assert_eq!(bytes, b"1\n");
}

/// Two-type-arg shape of `parse_fn_param_list`'s `Future(_)` re-walk arm.
#[test]
fn fn_with_parens_wrapped_dict_of_param_accepts_matching_dict() {
    let bytes = capture_program_output(
        "FN (SIZE d (DICT_OF Str Number)) -> Number = (1)\n\
         PRINT (SIZE {\"a\": 1, \"b\": 2})",
    );
    assert_eq!(bytes, b"1\n");
}

/// Element type is part of what an overload matches, so a non-satisfying container
/// is a non-match (`DispatchFailed`) rather than a committed-then-failed bind. With
/// no other overload to fall through to, the call surfaces no match.
#[test]
fn fn_typed_list_param_wrong_element_type_finds_no_match() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (HEAD xs :(LIST OF Number)) -> Number = (1)");
    let mut sched = Scheduler::new();
    sched.add_dispatch(parse_one("HEAD [\"a\"]"), scope);
    let error = sched.execute().expect_err(
        "List<Str> against a :(LIST OF Number)-only overload must fail to dispatch",
    );
    assert!(
        matches!(error.kind, KErrorKind::DispatchFailed { .. }),
        "expected DispatchFailed (no matching overload), got {error:?}",
    );
}

/// The bound parameter is coarsened to exactly the declared slot element type: `[1]`
/// (a `List<Number>`) bound into a `:(LIST OF Any)` slot reports `List<Any>`, the
/// contract.
#[test]
fn fn_typed_list_param_stamps_bound_arg_to_declared_element() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (ECHO xs :(LIST OF Any)) -> :(LIST OF Any) = (xs)");
    let result = run_one(scope, parse_one("ECHO [1]"));
    assert_eq!(result.ktype(), KType::List(Box::new(KType::Any)));
}

/// A correct-element call into a precise slot keeps the precise element type.
#[test]
fn fn_typed_list_param_accepts_matching_element_at_call() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (ECHO xs :(LIST OF Number)) -> :(LIST OF Number) = (xs)");
    let result = run_one(scope, parse_one("ECHO [1]"));
    assert_eq!(result.ktype(), KType::List(Box::new(KType::Number)));
}
