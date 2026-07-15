//! The `Pairwise` reducer's combiner fold
//! ([`operator_chain`](crate::machine::execute::dispatch::operator_chain)): a fixture-registered
//! pairwise group folds its pair results through its combiner — an **operator**, synthesized infix
//! and resolved at the use site — in either declared direction.
//!
//! The fixture's `%` bodies sum their operand pair and the combiner subtracts, so association is
//! observable: over `10 % 4 % 1 % 0` the three pairs are `14`, `5`, `1`, which fold left to
//! `(14 - 5) - 1` = 8 and right to `14 - (5 - 1)` = 10.

use std::collections::HashSet;

use crate::builtins::test_support::{parse_one, run, run_one, run_root_silent, run_root_with_buf};
use crate::machine::core::run_root_storage;
use crate::machine::model::{FoldDirection, OperatorGroup, ReductionMode};
use crate::machine::model::{KObject, Parseable};
use crate::machine::{BindingIndex, Scope};

/// Registers the `%` pairwise group in the given mode, the `%` pair body (a sum), and the `MINUS`
/// combiner the pair results fold through — declared with `OP`, the surface that gives a combiner
/// the infix keyword shape the reducer synthesizes. The `%` body is a plain `FN` rather than an
/// `OP`: `OP` would write its own singleton `%` registry entry, and a fold-left singleton conflicts
/// with the pairwise group the fixture registers by hand under the same probe.
fn register_pairwise_fixture<'a>(scope: &'a Scope<'a>, combiner: &str, direction: FoldDirection) {
    let members: HashSet<String> = ["%"].iter().map(|s| s.to_string()).collect();
    let group = scope.brand().alloc_operator_group(OperatorGroup::new(
        members,
        ReductionMode::Pairwise {
            combiner: combiner.to_string(),
            direction,
        },
    ));
    scope
        .register_operator_group("%".to_string(), group, BindingIndex::BUILTIN)
        .expect("register the pairwise operator group");
    run(scope, "FN (a :Number % b :Number) -> Number = (a + b)");
    run(scope, "OP #(MINUS) OVER Number = (left - right)");
}

/// The combiner fold: the reducer synthesizes `[pair, MINUS, pair]` over the pair results, an
/// infix keyworded expression that re-enters ordinary dispatch and binds the pair results
/// positionally against the `OP`-declared body.
#[test]
fn pairwise_combiner_folds_left() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    register_pairwise_fixture(scope, "MINUS", FoldDirection::Left);
    let result = run_one(scope, parse_one("10 % 4 % 1 % 0"));
    assert!(
        matches!(result, KObject::Number(n) if *n == 8.0),
        "a left fold nests ((p1 ⊙ p2) ⊙ p3) = (14 - 5) - 1 = 8; got {}",
        result.summarize(),
    );
}

/// The same run under a right-folding group nests the other way, so the two directions are
/// observably distinct through one non-associative combiner.
#[test]
fn pairwise_combiner_folds_right() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    register_pairwise_fixture(scope, "MINUS", FoldDirection::Right);
    let result = run_one(scope, parse_one("10 % 4 % 1 % 0"));
    assert!(
        matches!(result, KObject::Number(n) if *n == 10.0),
        "a right fold nests (p1 ⊙ (p2 ⊙ p3)) = 14 - (5 - 1) = 10; got {}",
        result.summarize(),
    );
}

/// Once-evaluation: the shared middle operand feeds both the `1 % 2` and `2 % 3` pairs, but it
/// dispatches — and its `PRINT` side effect runs — exactly once. `LOUD` is a `:Number`-typed
/// stand-in whose leading statement prints its argument and whose tail returns it unchanged, so the
/// chain still reduces (pairs `3` and `5`, folded through `MINUS` to `-2`) while leaving an
/// observable trace of how many times the operand actually ran.
#[test]
fn pairwise_combiner_evaluates_a_shared_operand_once() {
    let region = run_root_storage();
    let (scope, captured) = run_root_with_buf(&region);
    register_pairwise_fixture(scope, "MINUS", FoldDirection::Left);
    run(scope, "FN (LOUD x :Number) -> Number = ((PRINT x) (x))");
    let result = run_one(scope, parse_one("1 % (LOUD 2) % 3"));
    assert!(
        matches!(result, KObject::Number(n) if *n == -2.0),
        "the pairs are 1 + 2 = 3 and 2 + 3 = 5, folded through `MINUS` to 3 - 5 = -2; got {}",
        result.summarize(),
    );
    let bytes = captured.borrow().clone();
    assert_eq!(
        bytes,
        b"2\n",
        "the shared middle operand must dispatch exactly once (printing \"2\" exactly once); got {:?}",
        String::from_utf8_lossy(&bytes),
    );
}

/// The combiner is a *symbol*, resolved at the chain's use site through the ordinary dispatch walk
/// — the group record stores no resolved function. A combiner no scope declares therefore surfaces
/// as an ordinary dispatch failure at the chain, not at registration: group validation is deferred.
#[test]
fn pairwise_undeclared_combiner_errors_at_the_use_site() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    register_pairwise_fixture(scope, "NOWHERE", FoldDirection::Left);

    let error = crate::builtins::test_support::run_one_err(scope, parse_one("1 % 2 % 3"));
    let message = error.to_string();
    assert!(
        message.contains("NOWHERE"),
        "an undeclared combiner must surface as an ordinary use-site dispatch error; got: {message}",
    );
}

/// A fold-left run over *named* operands. The rewrite nests the run into `[(a ⊙ b), ⊙, c]`, whose
/// trailing bare name shares the expression with an eager sub-expression: no candidate strict-picks
/// against an unevaluated operand, so the pick — and with it the bare name's splice — happens on the
/// post-eager-subs re-resolve (`keyworded::finish`).
#[test]
fn fold_left_run_over_named_operands_resolves_the_trailing_name() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "LET x = 1\nLET y = 2\nLET z = 4");
    assert!(
        matches!(run_one(scope, parse_one("x + y + z")), KObject::Number(n) if *n == 7.0),
        "every operand of a named run reaches its binding",
    );
}
