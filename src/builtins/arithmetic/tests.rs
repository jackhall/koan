//! Plain keyworded dispatch over the binary arithmetic / comparison / `AND` builtins —
//! no chain, no group, no reducer. `1 + (2 * 3)` exercises only the existing eager-subs
//! nesting (the parenthesized operand stages as its own sub-dispatch) plus these bodies.

use crate::builtins::test_support::{parse_one, TestRun};
use crate::machine::model::KObject;
use crate::machine::run_root_storage;
use crate::machine::KErrorKind;

#[test]
fn add_dispatches_to_number() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let result = test_run.run_one(parse_one("1 + 2"));
    assert!(matches!(result, KObject::Number(n) if *n == 3.0));
}

#[test]
fn less_than_dispatches_to_bool() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let result = test_run.run_one(parse_one("1 < 2"));
    assert!(matches!(result, KObject::Bool(true)));
}

#[test]
fn and_dispatches_to_bool() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    // koan's boolean literals are lowercase (`true` / `false` — see
    // `src/parse/tokens.rs::try_literal`); `AND` is the keyword.
    let result = test_run.run_one(parse_one("true AND false"));
    assert!(matches!(result, KObject::Bool(false)));
}

/// `1 + (2 * 3)`: the parenthesized `(2 * 3)` operand stages as its own sub-dispatch via
/// the existing eager-subs track, splices back a `Number(6)`, and the outer `+` dispatches
/// over it — no chain/reducer involved, both `+` and `*` are plain 3-part `Keyworded`
/// expressions.
#[test]
fn nested_parenthesized_binary_evaluates() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let result = test_run.run_one(parse_one("1 + (2 * 3)"));
    assert!(matches!(result, KObject::Number(n) if *n == 7.0));
}

#[test]
fn subtract_multiply_and_ordering_comparisons_dispatch() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    assert!(matches!(test_run.run_one(parse_one("5 - 2")), KObject::Number(n) if *n == 3.0));
    assert!(matches!(test_run.run_one(parse_one("3 * 4")), KObject::Number(n) if *n == 12.0));
    assert!(matches!(test_run.run_one(parse_one("6 / 2")), KObject::Number(n) if *n == 3.0));
    assert!(matches!(
        test_run.run_one(parse_one("2 <= 2")),
        KObject::Bool(true)
    ));
    assert!(matches!(
        test_run.run_one(parse_one("3 > 2")),
        KObject::Bool(true)
    ));
    assert!(matches!(
        test_run.run_one(parse_one("2 >= 3")),
        KObject::Bool(false)
    ));
}

/// `Number` is `f64` with no prior division operator in the codebase to match, so a zero
/// divisor raises a structured `KError` rather than an IEEE-754 infinity/NaN.
#[test]
fn divide_by_zero_raises_structured_error() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let err = test_run.run_one_err(parse_one("1 / 0"));
    assert!(matches!(&err.kind, KErrorKind::User(msg) if msg.contains("division by zero")));
}

/// A non-Number operand is a dispatch non-match (the typed `:Number` slot), not a
/// bind-time type-check failure — it falls through to the ordinary "no matching
/// function" registry-miss diagnostic.
#[test]
fn add_over_non_number_is_dispatch_miss() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let err = test_run.run_one_err(parse_one("true + 1"));
    assert!(matches!(&err.kind, KErrorKind::DispatchFailed { .. }));
}

// =====================================================================
// Builtin-seeded operator groups reaching the `FoldLeft` reducer — no test-local group
// registration, only the seeded run root's own `register_builtin_operator_groups` seeding.
// =====================================================================

/// `1 + 2 + 3` — the additive group is seeded `FoldLeft`, so the chain rewrites to
/// `[ Expression([1, +, 2]), +, 3 ]` and evaluates left-to-right.
#[test]
fn additive_chain_folds_left_through_seeded_group() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let result = test_run.run_one(parse_one("1 + 2 + 3"));
    assert!(matches!(result, KObject::Number(n) if *n == 6.0));
}

/// `10 - 3 - 2` — left-association is observable here: a right-fold would give
/// `10 - (3 - 2)` = 9, but `FoldLeft` gives `(10 - 3) - 2` = 5.
#[test]
fn subtractive_chain_left_associates_through_seeded_group() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let result = test_run.run_one(parse_one("10 - 3 - 2"));
    assert!(matches!(result, KObject::Number(n) if *n == 5.0));
}

/// `1 + 2 * 3` mixes the additive and multiplicative groups — two separate seeded groups,
/// so the chain's probe (`"* +"`) is a genuine cross-group miss (nothing registers that
/// key), surfacing the ordinary registry-miss `DispatchFailed` rather than evaluating.
/// The parenthesized form (`nested_parenthesized_binary_evaluates`, above) is how this
/// mix is written when the multiplication should bind first.
#[test]
fn additive_multiplicative_mix_is_registry_miss() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let err = test_run.run_one_err(parse_one("1 + 2 * 3"));
    assert!(matches!(&err.kind, KErrorKind::DispatchFailed { .. }));
}

// =====================================================================
// Builtin-seeded comparison group reaching the `Pairwise` reducer — no test-local group
// registration, only the seeded run root's own `register_builtin_operator_groups` seeding.
// =====================================================================

/// `1 < 2 < 3` — the comparison group is seeded `Pairwise` with the `AND` keyword combiner
/// folded left, so the chain
/// stages `1`, `2`, and `3` as three independent dispatches, splices `2`'s resolved cell into
/// both the `1 < 2` and `2 < 3` pairs, and folds the two Bool results through `AND`.
#[test]
fn comparison_chain_reduces_pairwise_through_seeded_group() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let result = test_run.run_one(parse_one("1 < 2 < 3"));
    assert!(matches!(result, KObject::Bool(true)));
}

/// `1 <= 1 < 10` mixes two operators from the same comparison group in one pairwise run — the
/// probe `"< <="` still resolves to the single seeded group (seeding registers every nonempty
/// subset of the group's members, not just singletons).
#[test]
fn mixed_comparison_operators_reduce_pairwise_through_seeded_group() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let result = test_run.run_one(parse_one("1 <= 1 < 10"));
    assert!(matches!(result, KObject::Bool(true)));
}

/// `1 < 5 < 2` — the second pair (`5 < 2`) is false, so the strict `AND` fold is false even
/// though the first pair (`1 < 5`) is true; both pairs evaluate (no short-circuit).
#[test]
fn comparison_chain_pairwise_false_when_any_pair_fails() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let result = test_run.run_one(parse_one("1 < 5 < 2"));
    assert!(matches!(result, KObject::Bool(false)));
}

/// Once-evaluation proof: the shared middle operand feeds both the `1 < 2` and `2 < 3` pairs,
/// but it must dispatch — and its `PRINT` side effect must run — exactly once. `PRINT` returns
/// its rendered argument as a `Str` (see `src/builtins/print.rs`), which would fail the
/// surrounding `:Number` pairs' dispatch, so the shared operand here is `LOUD`, a tiny
/// test-registered `FN` whose leading (non-tail) statement prints its argument and whose tail
/// returns the same `Number` unchanged — a `:Number`-typed stand-in for a plain operand that
/// still leaves an observable trace of how many times it actually ran. The pairwise result is
/// unaffected (still `1 < 2 < 3` = true); the load-bearing assertion is on the captured output,
/// which must contain exactly one `"2\n"` line rather than two.
#[test]
fn pairwise_shared_middle_operand_evaluates_exactly_once() {
    let region = run_root_storage();
    let (mut test_run, captured) = TestRun::with_buf(&region);
    test_run.run("FN (LOUD x :Number) -> Number = ((PRINT x) (x))");
    let result = test_run.run_one(parse_one("1 < (LOUD 2) < 3"));
    assert!(matches!(result, KObject::Bool(true)));
    let bytes = captured.borrow().clone();
    assert_eq!(
        bytes, b"2\n",
        "the shared middle operand must dispatch exactly once (printing \"2\" exactly once); got {:?}",
        String::from_utf8_lossy(&bytes)
    );
}
