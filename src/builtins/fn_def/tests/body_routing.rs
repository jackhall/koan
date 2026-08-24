//! Branch coverage for the FN-def `body()` routing matrix and `ReturnTypeCapture`
//! variants on the dep-finish path, plus the Stage B param-name scan utility arms.

use crate::builtins::test_support::{TestRun, fn_is_registered, lookup_fn};
use crate::machine::KErrorKind;
use crate::machine::model::{KType, ReturnType};
use crate::machine::{program_storage, run_root_storage};

/// Parens-form return type carrying a bare lowercase identifier matching a parameter
/// name must defer.
#[test]
fn fn_def_sigil_return_type_with_identifier_param_ref_defers() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("FN (USE xs :Number) -> :(somefn xs) = (xs)");
    let f = lookup_fn(scope, "USE");
    assert!(
        matches!(f.signature.return_type(), ReturnType::Deferred(_)),
        "USE return type should be Deferred (sigil-form Identifier referencing param)",
    );
}

/// List literal inside a sigil-form return type carrying a parameter-name reference
/// must defer.
#[test]
fn fn_def_sigil_return_type_with_list_literal_param_ref_defers() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("FN (USE xs :Number) -> :([xs]) = (xs)");
    let f = lookup_fn(scope, "USE");
    assert!(
        matches!(f.signature.return_type(), ReturnType::Deferred(_)),
        "USE return type should be Deferred (ListLiteral referencing param)",
    );
}

/// Dict literal inside a sigil-form return type carrying a parameter-name reference
/// in a value position must defer.
#[test]
fn fn_def_sigil_return_type_with_dict_literal_param_ref_defers() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("FN (USE xs :Number) -> :({\"k\": xs}) = (xs)");
    let f = lookup_fn(scope, "USE");
    assert!(
        matches!(f.signature.return_type(), ReturnType::Deferred(_)),
        "USE return type should be Deferred (DictLiteral value referencing param)",
    );
}

/// An FN whose return type references a parameter name and whose parameter type
/// elaboration parks on a same-batch SIG routes through `defer` carrying
/// `ReturnTypeCapture::Deferred(_)`; the dep-finish lifts that carrier verbatim
/// into `ReturnType::Deferred(_)` once the SIG terminalizes.
#[test]
fn fn_def_deferred_return_with_pending_param_routes_through_combine() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "SIG Ordered = (VAL compare :Number)\n\
         FN (USE_ORD er :Ordered) -> :(TYPE OF er) = (er)",
    );
    let f = lookup_fn(scope, "USE_ORD");
    assert!(
        matches!(f.signature.return_type(), ReturnType::Deferred(_)),
        "USE_ORD return type should be Deferred after dep-finish wake, got {:?}",
        f.signature.return_type(),
    );
}

/// A sigil-form return type that sub-dispatches at FN-def (no parameter reference)
/// and a parameter slot that parks on the still-in-flight `LET MyT`'s placeholder
/// both join the same dep-finish; the finish picks each sub-dispatch's `Carried::Type`
/// terminal out of the dep results by the index the builder handed back when it
/// appended the request. `LET MyT` is submitted first (declaration order), but
/// statement-at-a-time submission puts both statements in flight together, so the
/// FN's param slot still parks on `MyT`'s placeholder when the FN statement steps
/// ahead of the LET's finalize.
#[test]
fn fn_def_expr_sub_dispatched_return_with_pending_param_routes_through_combine() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "LET MyT = Number\n\
         FN (USE xs :MyT) -> :(LIST OF Number) = ([1])",
    );
    let f = lookup_fn(scope, "USE");
    let ReturnType::Resolved(kt) = f.signature.return_type() else {
        panic!("USE return type should resolve to List<Number> after dep-finish wake");
    };
    assert_eq!(kt, test_run.types().list(KType::NUMBER));
}

/// A bare return type with no parameters parks on the still-in-flight `LET MyT`'s
/// placeholder and routes through `ReturnTypeCapture::Unresolved(name)`
/// (`make_capture`). `LET MyT` is submitted first (declaration order), but
/// statement-at-a-time submission puts both statements in flight together, so the
/// FN's return-type capture still parks on `MyT`'s placeholder when the FN pops
/// before the LET's finalize, and resolves once the LET wakes it.
#[test]
fn fn_def_bare_return_type_resolves_after_wake() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "LET MyT = Number\n\
         FN (NOP) -> MyT = (1)",
    );
    let f = lookup_fn(scope, "NOP");
    let ReturnType::Resolved(kt) = f.signature.return_type() else {
        panic!("NOP return type should resolve to Number after LET wakes");
    };
    assert_eq!(kt, KType::NUMBER);
}

/// A parens-form parameter type that sub-dispatches to a non-type value must
/// surface a `ShapeError` naming the offending slot's part-index, attributing the
/// rejection to the right signature slot rather than an opaque elaborator failure.
#[test]
fn fn_def_parens_param_type_non_type_value_errors() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let id = test_run.dispatch_in_scope(
        crate::machine::model::WorkingExpression::from_ast(
            scope.brand(),
            test_run.parse_one("FN (USE xs (1)) -> Null = (xs)"),
        ),
        scope,
    );
    let runtime = &mut test_run.runtime;
    let edge = runtime.install_edge_for_test(id, scope);
    runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let err = match runtime.edge_result_error(edge) {
        Err(e) => e,
        Ok(()) => panic!("non-type param type expression should error"),
    };
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("expected a type expression")),
        "expected ShapeError mentioning 'expected a type expression', got {err}",
    );
    assert!(!fn_is_registered(scope, "USE"), "USE should not register");
}

/// A sigil-form return type that sub-dispatches to a non-type value must surface
/// a `ShapeError` naming the return-type slot (the
/// `ReturnTypeCapture::ReturnTypeExpr` arm of the dep-finish).
#[test]
fn fn_def_sigil_return_type_non_type_value_errors() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let id = test_run.dispatch_in_scope(
        crate::machine::model::WorkingExpression::from_ast(
            scope.brand(),
            test_run.parse_one("FN (NOP) -> :(1) = (1)"),
        ),
        scope,
    );
    let runtime = &mut test_run.runtime;
    let edge = runtime.install_edge_for_test(id, scope);
    runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let err = match runtime.edge_result_error(edge) {
        Err(e) => e,
        Ok(()) => panic!("non-type return-type expression should error"),
    };
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("return-type slot sub-dispatch resolved to a non-type")),
        "expected ShapeError mentioning 'return-type slot sub-dispatch resolved to a non-type', got {err}",
    );
    assert!(!fn_is_registered(scope, "NOP"), "NOP should not register");
}
