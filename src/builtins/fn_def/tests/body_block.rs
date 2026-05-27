//! Multi-statement FN body behavior. Bodies of the shape `((s_0) (s_1) ... (s_{N-1}))`
//! are split at `KFunction::invoke` time: the first N-1 statements run as sibling
//! sub-slots in the per-call body scope (chain indices `1..N-1`), and the FN's
//! slot tail-replaces into the last statement at index `N`. The last statement's
//! value is the FN's terminal; TCO is preserved on the last statement.

use crate::builtins::test_support::{parse_one, run, run_one, run_root_silent};
use crate::machine::model::KObject;
use crate::machine::RuntimeArena;

use super::capture_program_output;

/// Three-statement FN body returns the last statement's value.
#[test]
fn multi_statement_fn_body_returns_last_value() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "FN (FOO) -> Number = ((LET x = 1) (LET y = 2) (y))",
    );
    let v = run_one(scope, parse_one("FOO"));
    assert!(matches!(v, KObject::Number(n) if *n == 2.0));
}

/// Each statement in a multi-statement body runs; effect ordering between siblings
/// is topological (sub-slot scheduling), not strict source-order.
#[test]
fn multi_statement_fn_body_runs_each_statement() {
    let bytes = capture_program_output(
        "FN (FOO) -> Str = ((PRINT \"a\") (PRINT \"b\") (PRINT \"c\"))\nFOO",
    );
    assert!(bytes.windows(2).any(|w| w == b"a\n"), "missing 'a' in {:?}", String::from_utf8_lossy(&bytes));
    assert!(bytes.windows(2).any(|w| w == b"b\n"), "missing 'b' in {:?}", String::from_utf8_lossy(&bytes));
    assert!(bytes.windows(2).any(|w| w == b"c\n"), "missing 'c' in {:?}", String::from_utf8_lossy(&bytes));
}

/// Backward reference across body statements: `b` reads `a` bound by an earlier
/// sibling. The visibility predicate (`b.idx < c`) admits the read because `a`
/// was submitted at a lower chain index than the consumer.
#[test]
fn backward_reference_across_statements_works() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "FN (FOO) -> Number = ((LET a = 10) (LET b = (a)) (b))",
    );
    let v = run_one(scope, parse_one("FOO"));
    assert!(matches!(v, KObject::Number(n) if *n == 10.0));
}
