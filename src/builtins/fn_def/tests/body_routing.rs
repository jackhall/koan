//! Branch coverage for the FN-def `body()` routing matrix and `ReturnTypeCapture`
//! variants on the dep-finish path, plus the Stage B param-name scan utility arms.

use crate::builtins::test_support::{fn_is_registered, lookup_fn, parse_one, run, run_root_silent};
use crate::machine::core::run_root_storage;
use crate::machine::execute::KoanRuntime;
use crate::machine::model::{KType, ReturnType};
use crate::machine::KErrorKind;

/// Parens-form return type carrying a bare lowercase identifier matching a parameter
/// name must defer.
#[test]
fn fn_def_sigil_return_type_with_identifier_param_ref_defers() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "FN (USE xs :Number) -> :(somefn xs) = (xs)");
    let f = lookup_fn(scope, "USE");
    assert!(
        matches!(f.signature.return_type, ReturnType::Deferred(_)),
        "USE return type should be Deferred (sigil-form Identifier referencing param)",
    );
}

/// List literal inside a sigil-form return type carrying a parameter-name reference
/// must defer.
#[test]
fn fn_def_sigil_return_type_with_list_literal_param_ref_defers() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "FN (USE xs :Number) -> :([xs]) = (xs)");
    let f = lookup_fn(scope, "USE");
    assert!(
        matches!(f.signature.return_type, ReturnType::Deferred(_)),
        "USE return type should be Deferred (ListLiteral referencing param)",
    );
}

/// Dict literal inside a sigil-form return type carrying a parameter-name reference
/// in a value position must defer.
#[test]
fn fn_def_sigil_return_type_with_dict_literal_param_ref_defers() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "FN (USE xs :Number) -> :({\"k\": xs}) = (xs)");
    let f = lookup_fn(scope, "USE");
    assert!(
        matches!(f.signature.return_type, ReturnType::Deferred(_)),
        "USE return type should be Deferred (DictLiteral value referencing param)",
    );
}

/// An FN whose return type references a parameter name and whose parameter type
/// elaboration parks on a same-batch SIG routes through `defer` carrying
/// `ReturnTypeCapture::Deferred(_)`; the dep-finish lifts that carrier verbatim
/// into `ReturnType::Deferred(_)` once the SIG terminalizes.
#[test]
fn fn_def_deferred_return_with_pending_param_routes_through_combine() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         FN (USE_ORD Er :OrderedSig) -> Er = (Er)",
    );
    let f = lookup_fn(scope, "USE_ORD");
    assert!(
        matches!(f.signature.return_type, ReturnType::Deferred(_)),
        "USE_ORD return type should be Deferred after dep-finish wake, got {:?}",
        f.signature.return_type,
    );
}

/// A sigil-form return type that sub-dispatches at FN-def (no parameter reference)
/// and a parameter slot that parks on a forward-LET binding both join the same
/// dep-finish; the return-type sub-dispatch's `results_pos` says where the closure picks
/// the lifted `KTypeValue` out of `&[&KObject]`.
#[test]
fn fn_def_expr_sub_dispatched_return_with_pending_param_routes_through_combine() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "FN (USE xs :MyT) -> :(LIST OF Number) = ([1])\n\
         LET MyT = Number",
    );
    let f = lookup_fn(scope, "USE");
    assert_eq!(
        f.signature.return_type,
        ReturnType::Resolved(KType::list(Box::new(KType::Number))),
        "USE return type should resolve to List<Number> after dep-finish wake",
    );
}

/// A bare forward-LET return type with no parameters parks on the LET's placeholder
/// and routes through `ReturnTypeCapture::Unresolved(name)` (`make_capture`).
#[test]
fn fn_def_forward_let_bare_return_type_resolves_after_wake() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "FN (NOP) -> MyT = (1)\n\
         LET MyT = Number",
    );
    let f = lookup_fn(scope, "NOP");
    assert_eq!(
        f.signature.return_type,
        ReturnType::Resolved(KType::Number),
        "NOP return type should resolve to Number after LET wakes",
    );
}

/// A parens-form parameter type that sub-dispatches to a non-`KTypeValue` must
/// surface a `ShapeError` naming the offending slot's part-index, attributing the
/// rejection to the right signature slot rather than an opaque elaborator failure.
#[test]
fn fn_def_parens_param_type_non_type_value_errors() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let mut runtime = KoanRuntime::new();
    let id = runtime.dispatch_in_scope(parse_one("FN (USE xs (1)) -> Null = (xs)"), scope);
    runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let err = match runtime.result_error(id) {
        Err(e) => e,
        Ok(()) => panic!("non-type param type expression should error"),
    };
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("expected a type expression")),
        "expected ShapeError mentioning 'expected a type expression', got {err}",
    );
    assert!(!fn_is_registered(scope, "USE"), "USE should not register");
}

/// A sigil-form return type that sub-dispatches to a non-`KTypeValue` must surface
/// a `ShapeError` naming the return-type slot (the
/// `ReturnTypeCapture::ReturnTypeExpr` arm of the dep-finish).
#[test]
fn fn_def_sigil_return_type_non_type_value_errors() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let mut runtime = KoanRuntime::new();
    let id = runtime.dispatch_in_scope(parse_one("FN (NOP) -> :(1) = (1)"), scope);
    runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let err = match runtime.result_error(id) {
        Err(e) => e,
        Ok(()) => panic!("non-type return-type expression should error"),
    };
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("return-type slot sub-dispatch resolved to a non-type")),
        "expected ShapeError mentioning 'return-type slot sub-dispatch resolved to a non-type', got {err}",
    );
    assert!(!fn_is_registered(scope, "NOP"), "NOP should not register");
}
