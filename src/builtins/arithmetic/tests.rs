//! Plain keyworded dispatch over the binary arithmetic / comparison / `AND` builtins —
//! no chain, no group, no reducer. `1 + (2 * 3)` exercises only the existing eager-subs
//! nesting (the parenthesized operand stages as its own sub-dispatch) plus these bodies.

use crate::builtins::test_support::{parse_one, run_one, run_one_err, run_root_silent};
use crate::machine::core::run_root_storage;
use crate::machine::model::KObject;
use crate::machine::KErrorKind;

#[test]
fn add_dispatches_to_number() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let result = run_one(scope, parse_one("1 + 2"));
    assert!(matches!(result, KObject::Number(n) if *n == 3.0));
}

#[test]
fn less_than_dispatches_to_bool() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let result = run_one(scope, parse_one("1 < 2"));
    assert!(matches!(result, KObject::Bool(true)));
}

#[test]
fn and_dispatches_to_bool() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    // koan's boolean literals are lowercase (`true` / `false` — see
    // `src/parse/tokens.rs::try_literal`); `AND` is the keyword.
    let result = run_one(scope, parse_one("true AND false"));
    assert!(matches!(result, KObject::Bool(false)));
}

/// `1 + (2 * 3)`: the parenthesized `(2 * 3)` operand stages as its own sub-dispatch via
/// the existing eager-subs track, splices back a `Number(6)`, and the outer `+` dispatches
/// over it — no chain/reducer involved, both `+` and `*` are plain 3-part `Keyworded`
/// expressions.
#[test]
fn nested_parenthesized_binary_evaluates() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let result = run_one(scope, parse_one("1 + (2 * 3)"));
    assert!(matches!(result, KObject::Number(n) if *n == 7.0));
}

#[test]
fn subtract_multiply_and_ordering_comparisons_dispatch() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    assert!(matches!(run_one(scope, parse_one("5 - 2")), KObject::Number(n) if *n == 3.0));
    assert!(matches!(run_one(scope, parse_one("3 * 4")), KObject::Number(n) if *n == 12.0));
    assert!(matches!(run_one(scope, parse_one("6 / 2")), KObject::Number(n) if *n == 3.0));
    assert!(matches!(
        run_one(scope, parse_one("2 <= 2")),
        KObject::Bool(true)
    ));
    assert!(matches!(
        run_one(scope, parse_one("3 > 2")),
        KObject::Bool(true)
    ));
    assert!(matches!(
        run_one(scope, parse_one("2 >= 3")),
        KObject::Bool(false)
    ));
}

/// `Number` is `f64` with no prior division operator in the codebase to match, so a zero
/// divisor raises a structured `KError` rather than an IEEE-754 infinity/NaN.
#[test]
fn divide_by_zero_raises_structured_error() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let err = run_one_err(scope, parse_one("1 / 0"));
    assert!(matches!(&err.kind, KErrorKind::User(msg) if msg.contains("division by zero")));
}

/// A non-Number operand is a dispatch non-match (the typed `:Number` slot), not a
/// bind-time type-check failure — it falls through to the ordinary "no matching
/// function" registry-miss diagnostic.
#[test]
fn add_over_non_number_is_dispatch_miss() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let err = run_one_err(scope, parse_one("true + 1"));
    assert!(matches!(&err.kind, KErrorKind::DispatchFailed { .. }));
}
