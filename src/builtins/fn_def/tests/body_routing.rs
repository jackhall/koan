//! Branch coverage for the FN-def `body()` routing matrix: each test forces a
//! specific (return-type-state × param-list-result) combination, exercises one
//! `ReturnTypeCapture` variant on the Combine-finish path, or trips a splice
//! rejection. Also covers the Stage B param-name scan utility arms
//! (`type_expr_references_any` Function-arrow recursion, `part_references_any`
//! Identifier / ListLiteral / DictLiteral arms).

use crate::builtins::test_support::{parse_one, run, run_root_silent};
use crate::machine::execute::Scheduler;
use crate::machine::model::{KObject, KType, ReturnType};
use crate::machine::{KErrorKind, RuntimeArena};

// ---------- Stage B param-name scan: utility arms in `type_expr_references_any` /
// `part_references_any`. Each test forces the scan to detect a parameter reference
// inside a return-type carrier and re-route the FN through `ReturnType::Deferred(_)`. ---

/// `type_expr_references_any` `TypeParams::Function { args, ret }` arm: a Function-arrow
/// return type whose `args` carry a parameter-name leaf must defer the return type so
/// per-call elaboration handles the parameter binding. Uses the Type-classified
/// parameter-name shape (`Er` per Stage A) since Function's `args` slot rejects lowercase
/// identifier-class tokens at parse time.
#[test]
fn fn_def_function_arrow_return_type_with_param_ref_defers() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         FN (USE Er :OrderedSig) -> :(Function (Number) -> Er) = (1)",
    );
    let data = scope.bindings().data();
    let f = match data.get("USE") {
        Some(KObject::KFunction(f, _)) => *f,
        other => panic!("USE should be a function, got {:?}", other.map(|o| o.ktype())),
    };
    assert!(
        matches!(f.signature.return_type, ReturnType::Deferred(_)),
        "USE return type should be Deferred (Function-arrow referencing param `Er`)",
    );
}

/// `part_references_any` `Identifier` arm: parens-form return type carrying a bare
/// lowercase identifier matching a parameter name must defer.
#[test]
fn fn_def_parens_return_type_with_identifier_param_ref_defers() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    // Parens-form return type `(somefn xs)` — `xs` parses as `Identifier("xs")` and
    // matches the FN's parameter name, routing to `Deferred(Expression(_))` without
    // any FN-def-time sub-Dispatch attempt against the outer scope.
    run(
        scope,
        "FN (USE xs :Number) -> (somefn xs) = (xs)",
    );
    let data = scope.bindings().data();
    let f = match data.get("USE") {
        Some(KObject::KFunction(f, _)) => *f,
        other => panic!("USE should be a function, got {:?}", other.map(|o| o.ktype())),
    };
    assert!(
        matches!(f.signature.return_type, ReturnType::Deferred(_)),
        "USE return type should be Deferred (parens-form Identifier referencing param)",
    );
}

/// `part_references_any` `ListLiteral` arm: a list literal inside a parens-form return
/// type carrying a parameter-name reference must defer. Exercises the recursion through
/// `items.iter().any(part_references_any, ...)`.
#[test]
fn fn_def_parens_return_type_with_list_literal_param_ref_defers() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "FN (USE xs :Number) -> ([xs]) = (xs)",
    );
    let data = scope.bindings().data();
    let f = match data.get("USE") {
        Some(KObject::KFunction(f, _)) => *f,
        other => panic!("USE should be a function, got {:?}", other.map(|o| o.ktype())),
    };
    assert!(
        matches!(f.signature.return_type, ReturnType::Deferred(_)),
        "USE return type should be Deferred (ListLiteral referencing param)",
    );
}

/// `part_references_any` `DictLiteral` arm: a dict literal inside a parens-form return
/// type carrying a parameter-name reference in a value position must defer. Exercises
/// the per-pair recursion through `pairs.iter().any(|(k, v)| ...)`.
#[test]
fn fn_def_parens_return_type_with_dict_literal_param_ref_defers() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "FN (USE xs :Number) -> ({\"k\": xs}) = (xs)",
    );
    let data = scope.bindings().data();
    let f = match data.get("USE") {
        Some(KObject::KFunction(f, _)) => *f,
        other => panic!("USE should be a function, got {:?}", other.map(|o| o.ktype())),
    };
    assert!(
        matches!(f.signature.return_type, ReturnType::Deferred(_)),
        "USE return type should be Deferred (DictLiteral value referencing param)",
    );
}

// ---------- (Deferred return type, Pending param list) routing arm + Combine-finish
// `ReturnTypeCapture::Deferred` arm. -----------------------------------------------------

/// (Deferred return type, Pending params) routing arm: an FN whose return type
/// references a parameter name *and* whose parameter type elaboration parks on a
/// SIG declared in the same batch routes through `defer_via_combine` carrying
/// `ReturnTypeCapture::Deferred(_)`. Pins that the Combine finish lifts that
/// carrier verbatim into `ReturnType::Deferred(_)` once the SIG terminalizes.
#[test]
fn fn_def_deferred_return_with_pending_param_routes_through_combine() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    // SIG and FN submitted in the same batch — the FN's param type elaboration parks
    // on `OrderedSig`'s placeholder; the return-type `Er` matches a parameter name
    // and routes through `ReturnTypeCapture::Deferred`.
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         FN (USE_ORD Er :OrderedSig) -> Er = (Er)",
    );
    let data = scope.bindings().data();
    let f = match data.get("USE_ORD") {
        Some(KObject::KFunction(f, _)) => *f,
        other => panic!("USE_ORD should be a function, got {:?}", other.map(|o| o.ktype())),
    };
    assert!(
        matches!(f.signature.return_type, ReturnType::Deferred(_)),
        "USE_ORD return type should be Deferred after Combine wake, got {:?}",
        f.signature.return_type,
    );
}

// ---------- (ExprSubDispatched return type, Pending params) routing arm: parens-form
// return type with no parameter reference combined with a parking parameter slot. ------

/// (ExprSubDispatched, Pending) routing arm: a parens-form return type that
/// sub-dispatches at FN-def (no parameter reference) and a parameter slot that parks
/// on a forward-LET binding must both join the same Combine. The return-type
/// sub-dispatch is appended after the park producers; its `results_pos` says where the
/// closure picks the lifted `KTypeValue` out of `&[&KObject]`.
#[test]
fn fn_def_expr_sub_dispatched_return_with_pending_param_routes_through_combine() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "FN (USE xs :MyT) -> (LIST_OF Number) = ([1])\n\
         LET MyT = Number",
    );
    let data = scope.bindings().data();
    let f = match data.get("USE") {
        Some(KObject::KFunction(f, _)) => *f,
        other => panic!("USE should be a function, got {:?}", other.map(|o| o.ktype())),
    };
    assert_eq!(
        f.signature.return_type,
        ReturnType::Resolved(KType::List(Box::new(KType::Number))),
        "USE return type should resolve to List<Number> after Combine wake",
    );
}

// ---------- (Pending return type, Done params) routing arm + `make_capture`'s two
// variants. ----------------------------------------------------------------------------

/// (Pending bare-leaf return type, Done params) routing arm with `make_capture`'s
/// `TypeParams::None` branch: a bare forward-LET return type with no parameters parks
/// on the LET's placeholder and routes through `ReturnTypeCapture::Unresolved(name)`.
#[test]
fn fn_def_forward_let_bare_return_type_resolves_after_wake() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "FN (NOP) -> MyT = (1)\n\
         LET MyT = Number",
    );
    let data = scope.bindings().data();
    let f = match data.get("NOP") {
        Some(KObject::KFunction(f, _)) => *f,
        other => panic!("NOP should be a function, got {:?}", other.map(|o| o.ktype())),
    };
    assert_eq!(
        f.signature.return_type,
        ReturnType::Resolved(KType::Number),
        "NOP return type should resolve to Number after LET wakes",
    );
}

/// `make_capture`'s `TypeParams::List | Function` arm: a parameterized forward-LET
/// return type (`:(List MyT)`) routes through `ReturnTypeCapture::TypeExpr(te)` so the
/// parser-preserved structure survives the Combine boundary.
#[test]
fn fn_def_forward_let_parameterized_return_type_resolves_after_wake() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "FN (NUMS) -> :(List MyT) = ([1])\n\
         LET MyT = Number",
    );
    let data = scope.bindings().data();
    let f = match data.get("NUMS") {
        Some(KObject::KFunction(f, _)) => *f,
        other => panic!("NUMS should be a function, got {:?}", other.map(|o| o.ktype())),
    };
    assert_eq!(
        f.signature.return_type,
        ReturnType::Resolved(KType::List(Box::new(KType::Number))),
        "NUMS return type should resolve to List<Number> after LET wakes",
    );
}

// ---------- Combine-finish error paths: a sub-Dispatched param or return-type slot
// that resolves to a non-`KTypeValue` result. -------------------------------------------

/// Combine-finish parameter-slot splice check: a parens-form parameter type that
/// sub-dispatches to a non-`KTypeValue` (`(1)` → `Number(1)`) must surface a
/// `ShapeError` naming the offending slot's part-index. Pins the per-slot diagnostic
/// path so the rejection is attributed to the right signature slot rather than
/// surfacing later as an opaque elaborator failure.
#[test]
fn fn_def_parens_param_type_non_type_value_errors() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(parse_one("FN (USE xs (1)) -> Null = (xs)"), scope);
    sched.execute().expect("execute does not surface per-slot errors");
    let err = match sched.read_result(id) {
        Err(e) => e,
        Ok(_) => panic!("non-type param type expression should error"),
    };
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("expected a type expression")),
        "expected ShapeError mentioning 'expected a type expression', got {err}",
    );
    let data = scope.bindings().data();
    assert!(data.get("USE").is_none(), "USE should not register");
}

/// Combine-finish return-type splice check: a parens-form return type that
/// sub-dispatches to a non-`KTypeValue` (`(1)` → `Number(1)`) must surface a
/// `ShapeError` naming the return-type slot. Mirrors the parameter-slot check but on
/// the `ReturnTypeCapture::ReturnTypeExpr` arm of the Combine finish.
#[test]
fn fn_def_parens_return_type_non_type_value_errors() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(parse_one("FN (NOP) -> (1) = (1)"), scope);
    sched.execute().expect("execute does not surface per-slot errors");
    let err = match sched.read_result(id) {
        Err(e) => e,
        Ok(_) => panic!("non-type return-type expression should error"),
    };
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("return-type slot sub-Dispatch")),
        "expected ShapeError mentioning 'return-type slot sub-Dispatch', got {err}",
    );
    let data = scope.bindings().data();
    assert!(data.get("NOP").is_none(), "NOP should not register");
}
