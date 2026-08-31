//! Explicit laziness: a bare `(…)` argument evaluates before its parent dispatches everywhere
//! except a lazy slot of a fixed builtin form, so a user signature receives code only as a
//! `KExpression` *value*.

use crate::builtins::test_support::TestRun;
use crate::machine::KErrorKind;
use crate::machine::{program_storage, run_root_storage};

/// A program's `PRINT` output. Each line is one evaluation, so a body that prints counts its own
/// runs.
fn run_program(source: &str) -> Vec<u8> {
    let program = program_storage();
    let region = run_root_storage();
    let (mut test_run, captured) = TestRun::with_buf(&program, &region);
    test_run.run(source);
    captured.borrow().clone()
}

/// A `#(…)` literal satisfies a user `:KExpression` parameter, and the body receives the
/// expression itself — the quote is the spelling that hands code to a user form.
#[test]
fn a_quote_satisfies_a_user_kexpression_parameter() {
    let bytes = run_program(
        "FN (RUNLATER body :KExpression) -> Any = ($(body))\n\
         PRINT (RUNLATER #(1 + 2))",
    );
    assert_eq!(bytes, b"3\n");
}

/// A name bound to a quote satisfies it too: the slot is an ordinary eager value parameter, so any
/// `KExpression`-valued expression fills it — not just a literal quote.
#[test]
fn a_name_bound_to_a_quote_satisfies_a_kexpression_parameter() {
    let bytes = run_program(
        "FN (RUNLATER body :KExpression) -> Any = ($(body))\n\
         LET quoted = #(9 + 1)\n\
         PRINT (RUNLATER quoted)",
    );
    assert_eq!(bytes, b"10\n");
}

/// So does a bare group that *evaluates* to code — the slot classifies the landed value, not the
/// spelling that produced it.
#[test]
fn a_group_evaluating_to_code_satisfies_a_kexpression_parameter() {
    let bytes = run_program(
        "FN (SAME q :KExpression) -> Any = (q)\n\
         FN (RUNLATER body :KExpression) -> Any = ($(body))\n\
         PRINT (RUNLATER (SAME #(9 + 1)))",
    );
    assert_eq!(bytes, b"10\n");
}

/// A bare group in a `:KExpression` slot is not code: it evaluates — its side effects run — and
/// only then does dispatch miss. The error names the quote as the fix, because that failure mode
/// is worth pointing at.
#[test]
fn a_bare_group_in_a_kexpression_parameter_evaluates_then_misses() {
    let program = program_storage();
    let region = run_root_storage();
    let (mut test_run, captured) = TestRun::with_buf(&program, &region);
    test_run.run("FN (RUNLATER body :KExpression) -> Any = ($(body))");
    let error = test_run.run_one_err(test_run.parse_one("RUNLATER (PRINT \"side effect\")"));
    assert_eq!(
        captured.borrow().clone(),
        b"side effect\n",
        "the argument evaluates before dispatch, so its side effect happens exactly once",
    );
    let KErrorKind::DispatchFailed { reason, .. } = &error.kind else {
        panic!("expected a dispatch miss, got {error}");
    };
    assert!(
        reason.contains("#(…)"),
        "the miss names the quote as the likely fix, got {reason}",
    );
}

/// The same rule in an ordinary typed slot: the group evaluates exactly once, before its parent
/// dispatches. One `side` line, from the one evaluation.
#[test]
fn a_bare_group_argument_evaluates_exactly_once_before_dispatch() {
    let bytes = run_program(
        "FN (TAKE s :Str) -> Str = (\"done\")\n\
         PRINT (TAKE (PRINT \"side\"))",
    );
    assert_eq!(bytes, b"side\ndone\n");
}

/// A builtin's lazy slots are stamped at seal, so a quoted `MATCH` spliced back in by `EVAL` still
/// runs only its selected branch: the stamp travelled with the quoted node.
#[test]
fn an_eval_spliced_quote_keeps_its_builtin_lazy_branches() {
    let bytes = run_program(
        "UNION Maybe = (Some :Number None :Null)\n\
         LET m = (Maybe (Some 1))\n\
         LET branch = #(MATCH (m) -> :Str WITH \
             (Some -> (PRINT \"yes\") None -> (PRINT \"NO_SHOULD_NOT_APPEAR\")))\n\
         $(branch)",
    );
    assert_eq!(bytes, b"yes\n");
}
