//! The binder position rule, pinned end-to-end: binding is a statement-level act, so a binder (any
//! expression carrying a `binder_plan` — name-installing declaration forms and named `FN` / `OP`
//! definitions alike) is legal in exactly two places, statement position and a lazily-captured
//! body. Every other eagerly-dispatched position — including another binder's own declaration slot
//! — pre-errors the slot with a TRY-catchable [`KErrorKind::NestedBinder`]. A value position takes
//! the anonymous `FN :{…}` form, which installs nothing; a definition that must also bind a name is
//! one statement in the combined `LET <name> = FN …` spelling.

use crate::builtins::test_support::{TestRun, parse_one};
use crate::machine::KErrorKind;
use crate::machine::core::{program_storage, run_root_storage};
use crate::machine::model::KObject;

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
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("FN (CALL n :Number) -> Number = (n)");
    let err = test_run.run_one_err(parse_one(&program, "CALL (LET x = 1)"));
    assert_nested_binder(err, "a user-call argument");
}

/// `(LET y = 1) + 2` — an operator-chain operand is an eager value position.
#[test]
fn let_in_operator_operand_is_nested_binder() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let err = test_run.run_one_err(parse_one(&program, "(LET y = 1) + 2"));
    assert_nested_binder(err, "an operator operand");
}

/// `{a = (LET v = 2)}` — a record-literal element is an eager value position.
#[test]
fn let_in_record_literal_element_is_nested_binder() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let err = test_run.run_one_err(parse_one(&program, "LET r = {a = (LET v = 2)}"));
    assert_nested_binder(err, "a record-literal element");
}

/// `(LET g = 5) (1)` — a deferred head is an eager value position.
#[test]
fn let_as_deferred_head_is_nested_binder() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let err = test_run.run_one_err(parse_one(&program, "(LET g = 5) (1)"));
    assert_nested_binder(err, "a deferred head");
}

/// A named `FN` definition is a binder wherever it appears: inline in a call
/// argument it is the same position error as a `LET`, not a function value.
#[test]
fn named_fn_in_user_call_argument_is_nested_binder() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("FN (USE f :(FN (x :Number) -> Str)) -> Str = (\"got fn\")");
    let err = test_run.run_one_err(parse_one(
        &program,
        "USE (FN (SHOW x :Number) -> Str = (\"hi\"))",
    ));
    assert_nested_binder(err, "a user-call argument (named FN)");
}

/// A named `FN` in a list-literal element is likewise rejected.
#[test]
fn named_fn_in_list_element_is_nested_binder() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let err = test_run.run_one_err(parse_one(
        &program,
        "LET xs = [(FN (ECHO x :Number) -> Number = (x))]",
    ));
    assert_nested_binder(err, "a list-literal element (named FN)");
}

/// A named `OP` definition in an eager argument position is likewise rejected.
#[test]
fn named_op_in_builtin_argument_is_nested_binder() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let err = test_run.run_one_err(parse_one(
        &program,
        "PRINT (OP #(⊕) OVER Number = (left + right))",
    ));
    assert_nested_binder(err, "a builtin argument (named OP)");
}

/// The anonymous `FN :{…}` form installs nothing, so it stays legal in the same
/// list-element position that rejects the named form.
#[test]
fn anonymous_fn_in_list_element_is_legal() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("LET xs = [(FN :{x :Number} -> Number = (x))]");
    match test_run.scope.lookup("xs") {
        Some(KObject::List(items, _)) => {
            assert_eq!(
                items.elements().len(),
                1,
                "list should hold the anonymous closure"
            );
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
    let program = program_storage();
    let region = run_root_storage();
    let (mut test_run, captured) = TestRun::with_buf(&program, &region);
    test_run.run(
        "TRY (PRINT (LET x = 1)) -> :Str WITH (\
             NestedBinder -> (PRINT \"caught\")\
         )",
    );
    let bytes = captured.borrow().clone();
    assert_eq!(bytes, b"caught\n");
}

/// A definition in another binder's declaration slot is an eager position like any other:
/// `LET f = (FN …)` errors, and the message names the one-statement spelling that does both.
#[test]
fn definition_in_a_declaration_slot_suggests_the_flat_spelling() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let err = test_run.run_one_err(parse_one(
        &program,
        "LET f = (FN (DOUBLE x :Number) -> Number = (x))",
    ));
    let message = format!("{err}");
    assert!(
        matches!(&err.kind, KErrorKind::NestedBinder { .. }),
        "expected NestedBinder for a binder's declaration slot, got {message}",
    );
    assert!(
        message.contains("LET <name> = FN <signature>"),
        "a rejected definition should name the flat spelling, got {message}",
    );
}

/// A plain `LET` in a declaration slot registers no overload, so it has no flat spelling to
/// suggest: `LET z = (LET a = 3)` errors with the bare position rule.
#[test]
fn plain_let_in_a_declaration_slot_gets_the_plain_message() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let err = test_run.run_one_err(parse_one(&program, "LET z = (LET a = 3)"));
    let message = format!("{err}");
    assert!(
        matches!(&err.kind, KErrorKind::NestedBinder { .. }),
        "expected NestedBinder for a nested LET, got {message}",
    );
    assert!(
        !message.contains("write it flat"),
        "a plain LET registers nothing, so there is no flat spelling to suggest: {message}",
    );
}

/// The combined statement forms are the legal spelling of the same intent, and they are statements
/// — so the same declarations that error above run when written flat.
#[test]
fn the_combined_forms_are_legal_at_statement_position() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("LET double = FN (DOUBLE x :Number) -> Number = (x * 2)");
    let result = test_run.run_one(parse_one(&program, "DOUBLE 4"));
    assert!(matches!(result, KObject::Number(n) if *n == 8.0));
}
