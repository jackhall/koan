//! Parameterized container types in FN parameter and return slots:
//! `List<T>`, `Dict<K, V>`, `Function<…>`, plus specificity tournaments.

use crate::builtins::test_support::{parse_one, run, run_one, run_root_silent};
use crate::machine::RuntimeArena;
use crate::machine::core::KErrorKind;
use crate::machine::execute::Scheduler;
use crate::machine::model::types::KType;
use crate::parse::parse;

use super::capture_program_output;

/// Phase 3 ascription stamping: a `List<Number>` body returned through `:(List Any)`
/// re-tags the carrier to *exactly* the declared return type — coarsening — so the
/// result's `ktype()` reports `List<Any>`, the contract, not the body's incidental
/// `List<Number>` precision.
#[test]
fn fn_return_coarsens_list_carrier_to_declared() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (NUMS) -> :(List Any) = ([1 2 3])");
    let result = run_one(scope, parse_one("NUMS"));
    assert_eq!(result.ktype(), KType::List(Box::new(KType::Any)));
}

/// Without an annotation, a list keeps its precise memoized join type.
#[test]
fn fn_return_keeps_precise_list_carrier_when_declared_precise() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (NUMS) -> :(List Number) = ([1 2 3])");
    let result = run_one(scope, parse_one("NUMS"));
    assert_eq!(result.ktype(), KType::List(Box::new(KType::Number)));
}

/// `[2, "hello"]` (a `List<Any>` value) returned through `:(List Number)` fails the
/// return-type check — a heterogeneous literal carries `List<Any>` and does not satisfy
/// the precise declared element type.
#[test]
fn fn_return_heterogeneous_list_rejected_by_precise_declared() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (BAD) -> :(List Number) = ([2 \"hello\"])");
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(parse_one("BAD"), scope);
    sched.execute().expect("scheduler runs to completion");
    assert!(sched.read_result(id).is_err());
}

/// Empty container through an annotated return boundary: the vacuous `matches_value`
/// passes and the declared element type is stamped, so `([]) -> :(List Number)` returns a
/// value whose `ktype()` is `List<Number>`.
#[test]
fn fn_return_empty_list_stamps_declared_element_type() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (EMPTY) -> :(List Number) = ([])");
    let result = run_one(scope, parse_one("EMPTY"));
    assert_eq!(result.ktype(), KType::List(Box::new(KType::Number)));
}

/// FN with a `List<Number>` parameter accepts a homogeneous number list and runs the body.
/// The signature parser routes `List<Number>` through `KType::List(Box::new(Number))`,
/// so per-element type checking is enforced at call time.
#[test]
fn fn_with_typed_list_param_accepts_matching_list() {
    let bytes = capture_program_output(
        "FN (HEAD xs :(List Number)) -> Number = (1)\n\
         PRINT (HEAD [1 2 3])",
    );
    assert_eq!(bytes, b"1\n");
}

/// A function declared `-> List<Number>` whose body returns a homogeneous number list
/// passes the scheduler's runtime return-type check.
#[test]
fn fn_returning_typed_list_accepts_matching_value() {
    let bytes = capture_program_output(
        "FN (NUMS) -> :(List Number) = ([1 2 3])\n\
         PRINT (NUMS)",
    );
    assert_eq!(bytes, b"[1, 2, 3]\n");
}

/// A function declared `-> List<Number>` whose body returns a list with a string
/// element fails the post-call return-type check (matches_value walks elements). The
/// scheduler stores the error in the result slot rather than failing `execute`, so we
/// read the slot via `read_result` to assert the failure.
#[test]
fn fn_returning_typed_list_rejects_wrong_element_type() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (BAD) -> :(List Number) = ([1 \"x\"])");
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(parse_one("BAD"), scope);
    sched.execute().expect("scheduler runs to completion");
    let res = sched.read_result(id);
    assert!(
        res.is_err(),
        "expected return-type mismatch when body produces :(List Any) for declared :(List Number)"
    );
}

/// FN-definition-time arity check: `List<A, B>` is invalid (List is unary). The error
/// surfaces at FN-construction time as a ShapeError, stored in the result slot.
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

/// FN with a `Dict<Str, Number>` parameter slot accepts a string-keyed number-valued dict.
#[test]
fn fn_with_typed_dict_param_accepts_matching_dict() {
    let bytes = capture_program_output(
        "FN (SIZE d :(Dict Str Number)) -> Number = (1)\n\
         PRINT (SIZE {\"a\": 1, \"b\": 2})",
    );
    assert_eq!(bytes, b"1\n");
}

/// FN with a `Function<(Number) -> Str>` parameter accepts a function value whose
/// signature matches structurally — the dispatch-time `function_compat` check. Pass an
/// inline FN expression as the argument to side-step having to dereference an
/// identifier-bound function.
#[test]
fn fn_with_typed_function_param_accepts_matching_function() {
    let bytes = capture_program_output(
        "FN (USE f :(Function (Number) -> Str)) -> Str = (\"got fn\")\n\
         PRINT (USE (FN (SHOW x :Number) -> Str = (\"hi\")))",
    );
    assert_eq!(bytes, b"got fn\n");
}

/// Specificity tournament: when two overloads share the same untyped shape and both
/// match, the more specific one wins. `(xs: List<Number>)` is strictly more specific
/// than `(xs: List<Any>)`, so a number-list call routes to the former.
#[test]
fn dispatch_picks_more_specific_list_overload() {
    let bytes = capture_program_output(
        "FN (PICK xs :(List Any)) -> Str = (\"any\")\n\
         FN (PICK xs :(List Number)) -> Str = (\"number\")\n\
         PRINT (PICK [1 2 3])",
    );
    assert_eq!(bytes, b"number\n");
}

/// Parens-wrapped FN parameter type: `xs: (LIST_OF Number)` schedules `(LIST_OF Number)`
/// as a sub-Dispatch from `parse_fn_param_list`, splices the resulting `KTypeValue` back
/// into the signature, and finalizes the FN with the elaborated `KType::List(Number)`.
/// Pins the per-roadmap "parens-wrapped type expressions sub-dispatch" Direction.
#[test]
fn fn_with_parens_wrapped_list_of_param_accepts_matching_list() {
    let bytes = capture_program_output(
        "FN (HEAD xs (LIST_OF Number)) -> Number = (1)\n\
         PRINT (HEAD [1 2 3])",
    );
    assert_eq!(bytes, b"1\n");
}

/// Nested parens-wrapped type expression in a FN parameter slot: `xs (LIST_OF (LIST_OF
/// Number))` exercises the same scheduler path the standalone `(LIST_OF (LIST_OF
/// Number))` test in `type_ops` exercises, but via the FN-def Combine.
#[test]
fn fn_with_nested_parens_wrapped_type_param_dispatches() {
    let bytes = capture_program_output(
        "FN (HEAD xs (LIST_OF (LIST_OF Number))) -> Number = (1)\n\
         PRINT (HEAD [[1 2] [3]])",
    );
    assert_eq!(bytes, b"1\n");
}

/// `d (DICT_OF Str Number)` walks the same parens-wrapped sub-Dispatch path as the
/// LIST_OF case but with two type args, exercising the multi-arg shape of
/// `parse_fn_param_list`'s `Future(_)` re-walk arm.
#[test]
fn fn_with_parens_wrapped_dict_of_param_accepts_matching_dict() {
    let bytes = capture_program_output(
        "FN (SIZE d (DICT_OF Str Number)) -> Number = (1)\n\
         PRINT (SIZE {\"a\": 1, \"b\": 2})",
    );
    assert_eq!(bytes, b"1\n");
}

/// Mixed list dispatches to the `List<Any>` overload (the only one that matches by
/// post-evaluation `matches_value`); the `List<Number>` overload is filtered out.
/// Note: dispatch-time matching is shape-only for containers (`Argument::matches`),
/// so both overloads pass the initial filter; specificity then picks `List<Number>`,
/// which fails at runtime element-check. Acceptable trade-off — caller gets the
/// type-mismatch error from the more-specific overload, which is informative.
#[test]
fn fn_typed_list_param_rejects_wrong_element_type_at_call() {
    // Single overload typed List<Number> — wrong-element-type call must error.
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (HEAD xs :(List Number)) -> Number = (1)");
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(parse_one("HEAD [\"a\"]"), scope);
    // Dispatch-time matching is shape-only and admits the call. The full
    // content-recursive element check now runs at the splice-time bind loop in
    // `KFunction::invoke`, where `bundle.args` holds evaluated values — so a
    // `List<Str>` value bound into a `:(List Number)` slot errors here, no longer
    // deferred to a later phase. The error lands on the node result.
    sched.execute().expect("scheduler runs to completion");
    let error = sched.read_result(id).err().expect(
        "List<Str> into :(List Number) slot must error at splice",
    );
    assert!(
        matches!(error.kind, KErrorKind::TypeMismatch { ref arg, .. } if arg == "xs"),
        "expected a TypeMismatch naming arg `xs`, got {error:?}",
    );
}

/// Splice-time argument stamping (the positive companion to the rejection above): a
/// correct-element call still succeeds, and the bound parameter is coarsened to exactly
/// the declared slot element type. The FN body returns the param so the call result's
/// `ktype()` reflects the stamped carrier — `[1]` is a `List<Number>` value bound into a
/// `:(List Any)` slot, so the bound `xs` reports `List<Any>`, the contract.
#[test]
fn fn_typed_list_param_stamps_bound_arg_to_declared_element() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (ECHO xs :(List Any)) -> :(List Any) = (xs)");
    let result = run_one(scope, parse_one("ECHO [1]"));
    assert_eq!(result.ktype(), KType::List(Box::new(KType::Any)));
}

/// A correct-element call into a *precise* slot succeeds and keeps the precise element
/// type: `[1]` (`List<Number>`) into `:(List Number)` binds and reports `List<Number>`.
#[test]
fn fn_typed_list_param_accepts_matching_element_at_call() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (ECHO xs :(List Number)) -> :(List Number) = (xs)");
    let result = run_one(scope, parse_one("ECHO [1]"));
    assert_eq!(result.ktype(), KType::List(Box::new(KType::Number)));
}
