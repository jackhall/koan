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

/// Element-only-differing overloads (`:(List Number)` vs `:(List Str)`) no longer tie on a
/// literal argument. The literal admits both shape-only at first dispatch, so the strict
/// tie defers; once `[1 2 3]` evaluates to a `List<Number>`, the element-aware re-dispatch
/// rejects `:(List Str)` (carried `List<Number>` doesn't satisfy it) and routes to
/// `:(List Number)`. The string literal routes symmetrically.
#[test]
fn dispatch_disambiguates_element_only_overloads_on_literal() {
    let numbers = capture_program_output(
        "FN (DESCRIBE xs :(List Number)) -> Str = (\"numbers\")\n\
         FN (DESCRIBE xs :(List Str)) -> Str = (\"strings\")\n\
         PRINT (DESCRIBE [1 2 3])",
    );
    assert_eq!(numbers, b"numbers\n");
    let strings = capture_program_output(
        "FN (DESCRIBE xs :(List Number)) -> Str = (\"numbers\")\n\
         FN (DESCRIBE xs :(List Str)) -> Str = (\"strings\")\n\
         PRINT (DESCRIBE [\"a\" \"b\"])",
    );
    assert_eq!(strings, b"strings\n");
}

/// The same disambiguation works through an *already-evaluated* container argument: a
/// `LET`-bound list forced through parens arrives at re-dispatch as a typed `Future` whose
/// carried element type selects the overload — no literal-defer needed.
#[test]
fn dispatch_disambiguates_element_only_overloads_on_evaluated_arg() {
    let bytes = capture_program_output(
        "FN (DESCRIBE xs :(List Number)) -> Str = (\"numbers\")\n\
         FN (DESCRIBE xs :(List Str)) -> Str = (\"strings\")\n\
         LET xs = [1 2 3]\n\
         PRINT (DESCRIBE (xs))",
    );
    assert_eq!(bytes, b"numbers\n");
}

/// A *bare* variable in a container slot disambiguates in the first dispatch pass via the
/// strict-pass type peek: `xs` resolves to its bound `List<Number>` value, whose carried
/// element type admits `:(List Number)` and rejects `:(List Str)` — no defer, no parens
/// needed. The string-bound variable routes symmetrically.
#[test]
fn dispatch_disambiguates_element_only_overloads_on_bare_variable() {
    let numbers = capture_program_output(
        "FN (DESCRIBE xs :(List Number)) -> Str = (\"numbers\")\n\
         FN (DESCRIBE xs :(List Str)) -> Str = (\"strings\")\n\
         LET xs = [1 2 3]\n\
         PRINT (DESCRIBE xs)",
    );
    assert_eq!(numbers, b"numbers\n");
    let strings = capture_program_output(
        "FN (DESCRIBE xs :(List Number)) -> Str = (\"numbers\")\n\
         FN (DESCRIBE xs :(List Str)) -> Str = (\"strings\")\n\
         LET xs = [\"a\" \"b\"]\n\
         PRINT (DESCRIBE xs)",
    );
    assert_eq!(strings, b"strings\n");
}

/// A backward-referenced bare variable disambiguates element-only overloads via
/// the strict-pass peek: `LET xs = [1 2 3]` lands `xs` with a `List<Number>`
/// carried type, then `(DESCRIBE xs)`'s strict pass reads that type through the
/// gated `resolve_with_chain` and routes to `:(List Number)`. Forward references
/// no longer drive this case under index-gated resolution — a later-sibling
/// `LET xs = …` is invisible to the consumer (LET is value-style gated).
#[test]
fn dispatch_disambiguates_element_only_overloads_on_bound_variable() {
    let bytes = capture_program_output(
        "FN (DESCRIBE xs :(List Number)) -> Str = (\"numbers\")\n\
         FN (DESCRIBE xs :(List Str)) -> Str = (\"strings\")\n\
         LET xs = [1 2 3]\n\
         LET y = (DESCRIBE xs)\n\
         PRINT y",
    );
    assert_eq!(bytes, b"numbers\n");
}

/// A genuinely *unbound* name (no binder anywhere, so no placeholder to park on) across a
/// tentative tie surfaces as the precise `UnboundName` — the name names nothing — rather
/// than a generic `DispatchFailed`. Mirrors what the single-overload path already reports
/// when the auto-wrapped name evaluates.
#[test]
fn dispatch_unbound_name_across_tied_overloads_is_unbound_error() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (DESCRIBE xs :(List Number)) -> Str = (\"numbers\")");
    run(scope, "FN (DESCRIBE xs :(List Str)) -> Str = (\"strings\")");
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

/// A heterogeneous literal memoizes `List<Any>`, which satisfies neither `:(List Number)`
/// nor `:(List Str)` — the post-eval re-dispatch admits neither overload, so the call finds
/// no match (`DispatchFailed`) rather than tying as ambiguous.
#[test]
fn dispatch_heterogeneous_literal_matches_no_concrete_element_overload() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (DESCRIBE xs :(List Number)) -> Str = (\"numbers\")");
    run(scope, "FN (DESCRIBE xs :(List Str)) -> Str = (\"strings\")");
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

/// A `:(List Any)` overload catches the heterogeneous list the concrete-element overloads
/// reject: `[1 "a"]` (`List<Any>`) satisfies only the `Any` slot.
#[test]
fn dispatch_list_any_overload_catches_heterogeneous_literal() {
    let bytes = capture_program_output(
        "FN (DESCRIBE xs :(List Number)) -> Str = (\"numbers\")\n\
         FN (DESCRIBE xs :(List Any)) -> Str = (\"any\")\n\
         PRINT (DESCRIBE [1 \"a\"])",
    );
    assert_eq!(bytes, b"any\n");
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

/// A wrong-element-type call finds no matching overload. The `["a"]` literal admits the
/// lone `:(List Number)` slot shape-only at first dispatch, but once it evaluates to a
/// `List<Str>` value the element-aware re-dispatch (`accepts_part`) rejects it — a
/// `List<Str>` carried type does not satisfy `:(List Number)`. With no other overload to
/// fall through to, this surfaces as `DispatchFailed`, not a bind-time `TypeMismatch`:
/// element type is part of what an overload matches, so a non-satisfying container is a
/// non-match rather than a committed-then-failed bind.
#[test]
fn fn_typed_list_param_wrong_element_type_finds_no_match() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (HEAD xs :(List Number)) -> Number = (1)");
    let mut sched = Scheduler::new();
    sched.add_dispatch(parse_one("HEAD [\"a\"]"), scope);
    // `Unmatched` propagates from `execute` itself (not as a node result), matching the
    // scheduler's `Ambiguous`/`Unmatched` → `Err` contract.
    let error = sched.execute().expect_err(
        "List<Str> against a :(List Number)-only overload must fail to dispatch",
    );
    assert!(
        matches!(error.kind, KErrorKind::DispatchFailed { .. }),
        "expected DispatchFailed (no matching overload), got {error:?}",
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
