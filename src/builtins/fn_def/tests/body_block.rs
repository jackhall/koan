//! Multi-statement FN body behavior. Bodies of the shape `((s_0) (s_1) ... (s_{N-1}))`
//! are split at `KFunction::invoke` time: the first N-1 statements run as siblings in
//! the per-call body scope, and the FN's slot tail-replaces into the last statement so
//! TCO is preserved on the terminal.

use crate::builtins::test_support::{parse_one, run, run_one, run_root_silent};
use crate::machine::model::KObject;
use crate::machine::RuntimeArena;

use super::capture_program_output;

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

/// Effect ordering between siblings is topological (sub-slot scheduling), not strict
/// source-order.
#[test]
fn multi_statement_fn_body_runs_each_statement() {
    let bytes = capture_program_output(
        "FN (FOO) -> Str = ((PRINT \"a\") (PRINT \"b\") (PRINT \"c\"))\nFOO",
    );
    assert!(bytes.windows(2).any(|w| w == b"a\n"), "missing 'a' in {:?}", String::from_utf8_lossy(&bytes));
    assert!(bytes.windows(2).any(|w| w == b"b\n"), "missing 'b' in {:?}", String::from_utf8_lossy(&bytes));
    assert!(bytes.windows(2).any(|w| w == b"c\n"), "missing 'c' in {:?}", String::from_utf8_lossy(&bytes));
}

/// Backward reference across siblings: the visibility predicate (`binder.idx < consumer`)
/// admits the read because `a` was submitted at a lower chain index.
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
