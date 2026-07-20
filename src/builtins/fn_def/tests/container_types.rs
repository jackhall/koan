//! Parameterized container types in FN parameter and return slots:
//! `List<T>`, `Dict<K, V>`, `Function<…>`, plus specificity tournaments.

use crate::builtins::test_support::{parse_one, TestRun};
use crate::machine::model::KType;
use crate::machine::run_root_storage;
use crate::machine::KErrorKind;

use super::capture_program_output;

/// A `List<Number>` body returned through `:(LIST OF Any)` re-tags the carrier to
/// exactly the declared return type — coarsening — so the result's `ktype()` reports
/// `List<Any>`, the contract, not the body's incidental `List<Number>` precision.
#[test]
fn fn_return_coarsens_list_carrier_to_declared() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    test_run.run("FN (NUMS) -> :(LIST OF Any) = ([1 2 3])");
    let result = test_run.run_one(parse_one("NUMS"));
    assert_eq!(result.ktype(), KType::list(Box::new(KType::Any)));
}

/// Without an annotation, a list keeps its precise memoized join type.
#[test]
fn fn_return_keeps_precise_list_carrier_when_declared_precise() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    test_run.run("FN (NUMS) -> :(LIST OF Number) = ([1 2 3])");
    let result = test_run.run_one(parse_one("NUMS"));
    assert_eq!(result.ktype(), KType::list(Box::new(KType::Number)));
}

/// A heterogeneous literal carries `List<Any>` and does not satisfy a precise
/// declared element type.
#[test]
fn fn_return_heterogeneous_list_rejected_by_precise_declared() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run("FN (BAD) -> :(LIST OF Number) = ([2 \"hello\"])");
    let runtime = &mut test_run.runtime;
    let id = runtime.dispatch_in_scope(parse_one("BAD"), scope);
    runtime.execute().expect("scheduler runs to completion");
    assert!(runtime.result_error(id).is_err());
}

/// Empty container through an annotated return boundary: the vacuous `matches_value`
/// passes and the declared element type is stamped.
#[test]
fn fn_return_empty_list_stamps_declared_element_type() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    test_run.run("FN (EMPTY) -> :(LIST OF Number) = ([])");
    let result = test_run.run_one(parse_one("EMPTY"));
    assert_eq!(result.ktype(), KType::list(Box::new(KType::Number)));
}

#[test]
fn fn_with_typed_list_param_accepts_matching_list() {
    let bytes = capture_program_output(
        "FN (HEAD xs :(LIST OF Number)) -> Number = (1)\n\
         PRINT (HEAD [1 2 3])",
    );
    assert_eq!(bytes, b"1\n");
}

/// A signature is a type value, so it rides container type expressions: `:(LIST OF S)`
/// elaborates, a list of modules satisfying `S` dispatches into the slot (each element's
/// memoized self-sig digest-equals or refines `S`), and a list of non-satisfying modules
/// falls through to the generic overload.
#[test]
fn fn_with_signature_element_list_param_dispatches_on_satisfaction() {
    let bytes = capture_program_output(
        "SIG HasLabel = (VAL label :Str)\n\
         MODULE widget = (LET label = \"button\")\n\
         MODULE plain = (LET count = 3)\n\
         FN (DESCRIBE xs :(LIST OF HasLabel)) -> Str = (\"labelled\")\n\
         FN (DESCRIBE xs :Any) -> Str = (\"generic\")\n\
         PRINT (DESCRIBE [widget, widget])\n\
         PRINT (DESCRIBE [plain, plain])",
    );
    assert_eq!(bytes, b"labelled\ngeneric\n");
}

/// `:(MAP Str -> S)` elaborates with a signature value type and admits a dict of satisfying
/// module values.
#[test]
fn fn_with_signature_value_map_param_dispatches() {
    let bytes = capture_program_output(
        "SIG HasLabel = (VAL label :Str)\n\
         MODULE widget = (LET label = \"button\")\n\
         FN (LOOKUP d :(MAP Str -> HasLabel)) -> Str = (\"map-of-labelled\")\n\
         PRINT (LOOKUP {\"a\": widget})",
    );
    assert_eq!(bytes, b"map-of-labelled\n");
}

/// Bound-identifier elements name-resolve, so a list built from them memoizes the resolved
/// values' element type and dispatches into the matching typed slot — both as a LET-bound
/// value and as an inline literal.
#[test]
fn fn_with_typed_list_param_accepts_bound_identifier_elements() {
    let bytes = capture_program_output(
        "FN (HEAD xs :(LIST OF Number)) -> Number = (1)\n\
         LET n = 5\n\
         LET ns = [n, n]\n\
         PRINT (HEAD ns)\n\
         PRINT (HEAD [n, n])",
    );
    assert_eq!(bytes, b"1\n1\n");
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
/// failing `execute`, so we read the slot via `result_error` to assert the failure.
#[test]
fn fn_returning_typed_list_rejects_wrong_element_type() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run("FN (BAD) -> :(LIST OF Number) = ([1 \"x\"])");
    let runtime = &mut test_run.runtime;
    let id = runtime.dispatch_in_scope(parse_one("BAD"), scope);
    runtime.execute().expect("scheduler runs to completion");
    let res = runtime.result_error(id);
    assert!(
        res.is_err(),
        "expected return-type mismatch when body produces :(LIST OF Any) for declared :(LIST OF Number)"
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

/// `function_compat` is name-keyed: a function whose parameter name differs from the
/// slot's (`n` vs `x`) does not fill the slot. With no other overload, the call surfaces
/// `DispatchFailed` rather than binding the structurally-similar function.
#[test]
fn fn_with_typed_function_param_rejects_name_mismatch() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run("FN (USE f :(FN (x :Number) -> Str)) -> Str = (\"got fn\")");
    let runtime = &mut test_run.runtime;
    let root = runtime.dispatch_in_scope(
        parse_one("USE (FN (SHOW n :Number) -> Str = (\"hi\"))"),
        scope,
    );
    runtime
        .execute()
        .expect("a dispatch failure is slot-terminal, not a fatal execute error");
    let error = runtime
        .result_error(root)
        .expect_err("a function with param name `n` must not fill a `(x :Number)` slot");
    assert!(
        matches!(error.kind, KErrorKind::DispatchFailed { .. }),
        "expected DispatchFailed on parameter-name mismatch, got {error:?}",
    );
}

/// Depth-contravariant admit: a value whose param accepts `Any` fills a slot promising
/// only a `Number` param. Under call-by-name, a `Number` argument is a valid `Any`, so
/// the more-general value param subsumes the slot's narrower promise.
#[test]
fn fn_with_typed_function_param_admits_contravariant_param() {
    let bytes = capture_program_output(
        "FN (USE f :(FN (x :Number) -> Str)) -> Str = (\"got fn\")\n\
         PRINT (USE (FN (SHOW x :Any) -> Str = (\"hi\")))",
    );
    assert_eq!(bytes, b"got fn\n");
}

/// Covariant-return admit: a value returning a `Number` subtype fills a slot promising
/// an `Any` return. The slot's caller only relies on the wider `Any`, which the
/// narrower `Number` return satisfies.
#[test]
fn fn_with_typed_function_param_admits_covariant_return() {
    let bytes = capture_program_output(
        "FN (USE f :(FN (x :Number) -> Any)) -> Str = (\"got fn\")\n\
         PRINT (USE (FN (SHOW x :Number) -> Number = (1)))",
    );
    assert_eq!(bytes, b"got fn\n");
}

/// Width-drop admit + callable: a unary value fills a binary slot. The extra slot
/// param (`y`) is unbound under call-by-name, and the bound function still runs.
#[test]
fn fn_with_typed_function_param_admits_width_drop() {
    let bytes = capture_program_output(
        "FN (USE f :(FN (x :Number, y :Str) -> Str)) -> Str = (\"got fn\")\n\
         PRINT (USE (FN (SHOW x :Number) -> Str = (\"hi\")))",
    );
    assert_eq!(bytes, b"got fn\n");
}

/// Width-extra reject: a value declaring a param (`y`) the slot doesn't promise fails
/// to fill the slot — the value requires an argument call-by-name can't supply. With no
/// other overload the call surfaces `DispatchFailed`.
#[test]
fn fn_with_typed_function_param_rejects_width_extra() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run("FN (USE f :(FN (x :Number) -> Str)) -> Str = (\"got fn\")");
    let runtime = &mut test_run.runtime;
    let root = runtime.dispatch_in_scope(
        parse_one("USE (FN (SHOW x :Number, y :Str) -> Str = (\"hi\"))"),
        scope,
    );
    runtime
        .execute()
        .expect("a dispatch failure is slot-terminal, not a fatal execute error");
    let error = runtime
        .result_error(root)
        .expect_err("a value declaring an extra param `y` must not fill a `(x :Number)` slot");
    assert!(
        matches!(error.kind, KErrorKind::DispatchFailed { .. }),
        "expected DispatchFailed on width-extra value param, got {error:?}",
    );
}

/// Contravariant specificity tie-break: with overloads keyed on `(x :Number)` and
/// `(x :Any)` function slots, a value param of `Any` admits both but picks the
/// `(x :Any)` overload (the value's `Any` param is contravariantly most specific for
/// the `(x :Any)` slot), and a value param of `Number` picks the `(x :Number)` overload.
#[test]
fn fn_typed_function_param_contravariant_tiebreak() {
    let any_value = capture_program_output(
        "FN (USE f :(FN (x :Number) -> Str)) -> Str = (\"narrow\")\n\
         FN (USE f :(FN (x :Any) -> Str)) -> Str = (\"wide\")\n\
         PRINT (USE (FN (GET x :Any) -> Str = (\"v\")))",
    );
    assert_eq!(any_value, b"wide\n");
    let number_value = capture_program_output(
        "FN (USE f :(FN (x :Number) -> Str)) -> Str = (\"narrow\")\n\
         FN (USE f :(FN (x :Any) -> Str)) -> Str = (\"wide\")\n\
         PRINT (USE (FN (GET x :Number) -> Str = (\"v\")))",
    );
    assert_eq!(number_value, b"narrow\n");
}

/// Incomparable overloads tie as ambiguous: a value param of `Any` fills both the
/// `(x :Number)` and `(x :Str)` function slots (contravariantly), but the two slots
/// are mutually incomparable, so neither wins → `AmbiguousDispatch`.
#[test]
fn fn_typed_function_param_incomparable_is_ambiguous() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run("FN (USE f :(FN (x :Number) -> Str)) -> Str = (\"num\")");
    test_run.run("FN (USE f :(FN (x :Str) -> Str)) -> Str = (\"str\")");
    let runtime = &mut test_run.runtime;
    let root =
        runtime.dispatch_in_scope(parse_one("USE (FN (GET x :Any) -> Str = (\"v\"))"), scope);
    runtime
        .execute()
        .expect("a dispatch failure is slot-terminal, not a fatal execute error");
    let error = runtime
        .result_error(root)
        .expect_err("an `Any`-param value matching two incomparable slots must be ambiguous");
    assert!(
        matches!(error.kind, KErrorKind::AmbiguousDispatch { .. }),
        "expected AmbiguousDispatch across incomparable function slots, got {error:?}",
    );
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
/// typed `Spliced` whose carried element type selects the overload — no literal-defer
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
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run("FN (DESCRIBE xs :(LIST OF Number)) -> Str = (\"numbers\")");
    test_run.run("FN (DESCRIBE xs :(LIST OF Str)) -> Str = (\"strings\")");
    let runtime = &mut test_run.runtime;
    let root = runtime.dispatch_in_scope(parse_one("DESCRIBE nope"), scope);
    runtime
        .execute()
        .expect("a dispatch failure is slot-terminal, not a fatal execute error");
    let error = runtime
        .result_error(root)
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
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run("FN (DESCRIBE xs :(LIST OF Number)) -> Str = (\"numbers\")");
    test_run.run("FN (DESCRIBE xs :(LIST OF Str)) -> Str = (\"strings\")");
    let runtime = &mut test_run.runtime;
    let root = runtime.dispatch_in_scope(parse_one("DESCRIBE [1 \"a\"]"), scope);
    runtime
        .execute()
        .expect("a dispatch failure is slot-terminal, not a fatal execute error");
    let error = runtime
        .result_error(root)
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
        "FN (HEAD xs :(LIST OF Number)) -> Number = (1)\n\
         PRINT (HEAD [1 2 3])",
    );
    assert_eq!(bytes, b"1\n");
}

#[test]
fn fn_with_nested_parens_wrapped_type_param_dispatches() {
    let bytes = capture_program_output(
        "FN (HEAD xs :(LIST OF :(LIST OF Number))) -> Number = (1)\n\
         PRINT (HEAD [[1 2] [3]])",
    );
    assert_eq!(bytes, b"1\n");
}

/// Two-type-arg shape of `parse_fn_param_list`'s `Spliced(_)` re-walk arm.
#[test]
fn fn_with_parens_wrapped_dict_of_param_accepts_matching_dict() {
    let bytes = capture_program_output(
        "FN (SIZE d :(MAP Str -> Number)) -> Number = (1)\n\
         PRINT (SIZE {\"a\": 1, \"b\": 2})",
    );
    assert_eq!(bytes, b"1\n");
}

/// Element type is part of what an overload matches, so a non-satisfying container
/// is a non-match (`DispatchFailed`) rather than a committed-then-failed bind. With
/// no other overload to fall through to, the call surfaces no match.
#[test]
fn fn_typed_list_param_wrong_element_type_finds_no_match() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run("FN (HEAD xs :(LIST OF Number)) -> Number = (1)");
    let runtime = &mut test_run.runtime;
    let root = runtime.dispatch_in_scope(parse_one("HEAD [\"a\"]"), scope);
    runtime
        .execute()
        .expect("a dispatch failure is slot-terminal, not a fatal execute error");
    let error = runtime
        .result_error(root)
        .expect_err("List<Str> against a :(LIST OF Number)-only overload must fail to dispatch");
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
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    test_run.run("FN (ECHO xs :(LIST OF Any)) -> :(LIST OF Any) = (xs)");
    let result = test_run.run_one(parse_one("ECHO [1]"));
    assert_eq!(result.ktype(), KType::list(Box::new(KType::Any)));
}

/// A correct-element call into a precise slot keeps the precise element type.
#[test]
fn fn_typed_list_param_accepts_matching_element_at_call() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    test_run.run("FN (ECHO xs :(LIST OF Number)) -> :(LIST OF Number) = (xs)");
    let result = test_run.run_one(parse_one("ECHO [1]"));
    assert_eq!(result.ktype(), KType::list(Box::new(KType::Number)));
}
