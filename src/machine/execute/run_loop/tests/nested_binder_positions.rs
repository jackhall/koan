//! The nested-binder position rule, pinned end-to-end: a binder (any expression whose
//! cached `binder_installs` aggregate is non-empty — name-installing declaration forms
//! and named `FN` / `OP` definitions alike) may appear at statement position, in a
//! lazily-captured body, or in another binder's own declaration slot. Every other
//! eagerly-dispatched position pre-errors the slot with a TRY-catchable
//! [`KErrorKind::NestedBinder`]. Value positions take the anonymous `FN :{…}` form,
//! which installs nothing.

use crate::builtins::test_support::{parse_one, TestRun};
use crate::machine::core::run_root_storage;
use crate::machine::model::KObject;
use crate::machine::KErrorKind;

/// Assert `err` is `NestedBinder`, with a readable failure otherwise.
fn assert_nested_binder(err: crate::machine::KError, position: &str) {
    assert!(
        matches!(&err.kind, KErrorKind::NestedBinder { .. }),
        "expected NestedBinder for a binder in {position}, got {err}",
    );
}

/// `f (LET x = 1)` — a user-call argument is an eager value position.
#[test]
fn let_in_user_call_argument_is_nested_binder() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    test_run.run("FN (CALL n :Number) -> Number = (n)");
    let err = test_run.run_one_err(parse_one("CALL (LET x = 1)"));
    assert_nested_binder(err, "a user-call argument");
}

/// `(LET y = 1) + 2` — an operator-chain operand is an eager value position.
#[test]
fn let_in_operator_operand_is_nested_binder() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let err = test_run.run_one_err(parse_one("(LET y = 1) + 2"));
    assert_nested_binder(err, "an operator operand");
}

/// `{a = (LET v = 2)}` — a record-literal element is an eager value position.
#[test]
fn let_in_record_literal_element_is_nested_binder() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let err = test_run.run_one_err(parse_one("LET r = {a = (LET v = 2)}"));
    assert_nested_binder(err, "a record-literal element");
}

/// `(LET g = 5) (1)` — a deferred head is an eager value position.
#[test]
fn let_as_deferred_head_is_nested_binder() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let err = test_run.run_one_err(parse_one("(LET g = 5) (1)"));
    assert_nested_binder(err, "a deferred head");
}

/// A named `FN` definition is a binder wherever it appears: inline in a call
/// argument it is the same position error as a `LET`, not a function value.
#[test]
fn named_fn_in_user_call_argument_is_nested_binder() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    test_run.run("FN (USE f :(FN (x :Number) -> Str)) -> Str = (\"got fn\")");
    let err = test_run.run_one_err(parse_one("USE (FN (SHOW x :Number) -> Str = (\"hi\"))"));
    assert_nested_binder(err, "a user-call argument (named FN)");
}

/// A named `FN` in a list-literal element is likewise rejected.
#[test]
fn named_fn_in_list_element_is_nested_binder() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let err = test_run.run_one_err(parse_one(
        "LET xs = [(FN (ECHO x :Number) -> Number = (x))]",
    ));
    assert_nested_binder(err, "a list-literal element (named FN)");
}

/// A named `OP` definition in an eager argument position is likewise rejected.
#[test]
fn named_op_in_builtin_argument_is_nested_binder() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let err = test_run.run_one_err(parse_one("PRINT (OP #(⊕) OVER Number = (left + right))"));
    assert_nested_binder(err, "a builtin argument (named OP)");
}

/// The anonymous `FN :{…}` form installs nothing, so it stays legal in the same
/// list-element position that rejects the named form.
#[test]
fn anonymous_fn_in_list_element_is_legal() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    test_run.run("LET xs = [(FN :{x :Number} -> Number = (x))]");
    match test_run.scope.lookup("xs") {
        Some(KObject::List(items, _)) => {
            assert_eq!(items.len(), 1, "list should hold the anonymous closure");
        }
        other => panic!(
            "expected `xs` bound to a List, got {:?}",
            other.map(|o| o.ktype().name(test_run.types())),
        ),
    }
}

/// The error is slot-terminal and TRY-catchable like any structured error.
#[test]
fn nested_binder_error_is_try_catchable() {
    let region = run_root_storage();
    let (mut test_run, captured) = TestRun::with_buf(&region);
    test_run.run(
        "TRY (PRINT (LET x = 1)) -> :Str WITH (\
             NestedBinder -> (PRINT \"caught\")\
         )",
    );
    let bytes = captured.borrow().clone();
    assert_eq!(bytes, b"caught\n");
}
