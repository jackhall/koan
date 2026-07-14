//! The `Pairwise` reducer's combiner fold
//! ([`operator_chain`](crate::machine::execute::dispatch::operator_chain)): a fixture-registered
//! pairwise group folds its pair results through either combiner kind — a keyword (ordinary
//! keyworded dispatch) or a value name (the `FunctionValueCall` lane, resolved at the use site) —
//! in either declared direction.
//!
//! The fixture's `%` bodies sum their operand pair and both combiners subtract, so association is
//! observable: over `10 % 4 % 1 % 0` the three pairs are `14`, `5`, `1`, which fold left to
//! `(14 - 5) - 1` = 8 and right to `14 - (5 - 1)` = 10.

use std::collections::HashSet;

use crate::builtins::test_support::{parse_one, run, run_one, run_root_silent, run_root_with_buf};
use crate::machine::core::run_root_storage;
use crate::machine::model::operators::{Combiner, FoldDirection, OperatorGroup, ReductionMode};
use crate::machine::model::{KObject, Parseable};
use crate::machine::{BindingIndex, Scope};

/// Registers the `%` pairwise group in the given mode, the `%` pair body (a sum), and both
/// combiners the tests fold through: `mix`, a value-bound `FN` taking the `left` / `right`
/// arguments a [`Combiner::Name`] call binds, and `MINUS`, a keyworded body for a
/// [`Combiner::Keyword`] fold. Both subtract, so a left fold and a right fold disagree.
fn register_pairwise_fixture<'a>(
    scope: &'a Scope<'a>,
    combiner: Combiner,
    direction: FoldDirection,
) {
    let members: HashSet<String> = ["%"].iter().map(|s| s.to_string()).collect();
    let group = scope.brand().alloc_operator_group(OperatorGroup::new(
        members,
        ReductionMode::Pairwise {
            combiner,
            direction,
        },
    ));
    scope
        .register_operator_group("%".to_string(), group, BindingIndex::BUILTIN)
        .expect("register the pairwise operator group");
    run(scope, "FN (a :Number % b :Number) -> Number = (a + b)");
    run(
        scope,
        "LET mix = (FN (MIX left :Number right :Number) -> Number = (left - right))",
    );
    run(scope, "FN (a :Number MINUS b :Number) -> Number = (a - b)");
}

/// A `Combiner::Name` fold: the reducer synthesizes `mix {left = …, right = …}` over the pair
/// results, an Identifier-head call the `FunctionValueCall` lane resolves in the scope the chain
/// is written in.
#[test]
fn pairwise_name_combiner_folds_left() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    register_pairwise_fixture(
        scope,
        Combiner::Name("mix".to_string()),
        FoldDirection::Left,
    );
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
fn pairwise_name_combiner_folds_right() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    register_pairwise_fixture(
        scope,
        Combiner::Name("mix".to_string()),
        FoldDirection::Right,
    );
    let result = run_one(scope, parse_one("10 % 4 % 1 % 0"));
    assert!(
        matches!(result, KObject::Number(n) if *n == 10.0),
        "a right fold nests (p1 ⊙ (p2 ⊙ p3)) = 14 - (5 - 1) = 10; got {}",
        result.summarize(),
    );
}

/// A `Combiner::Keyword` fold over the same run: the synthesized shape is the 3-part keyworded
/// expression `[pair, MINUS, pair]`, which re-enters ordinary keyworded dispatch.
#[test]
fn pairwise_keyword_combiner_folds_left() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    register_pairwise_fixture(
        scope,
        Combiner::Keyword("MINUS".to_string()),
        FoldDirection::Left,
    );
    let result = run_one(scope, parse_one("10 % 4 % 1 % 0"));
    assert!(
        matches!(result, KObject::Number(n) if *n == 8.0),
        "a left fold nests ((p1 ⊙ p2) ⊙ p3) = (14 - 5) - 1 = 8; got {}",
        result.summarize(),
    );
}

/// The keyword combiner takes the fold direction too — the direction is a property of the group,
/// not of the combiner kind.
#[test]
fn pairwise_keyword_combiner_folds_right() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    register_pairwise_fixture(
        scope,
        Combiner::Keyword("MINUS".to_string()),
        FoldDirection::Right,
    );
    let result = run_one(scope, parse_one("10 % 4 % 1 % 0"));
    assert!(
        matches!(result, KObject::Number(n) if *n == 10.0),
        "a right fold nests (p1 ⊙ (p2 ⊙ p3)) = 14 - (5 - 1) = 10; got {}",
        result.summarize(),
    );
}

/// Once-evaluation under a name combiner: the shared middle operand feeds both the `1 % 2` and
/// `2 % 3` pairs, but it dispatches — and its `PRINT` side effect runs — exactly once. `LOUD` is a
/// `:Number`-typed stand-in whose leading statement prints its argument and whose tail returns it
/// unchanged, so the chain still reduces (pairs `3` and `5`, folded through `mix` to `-2`) while
/// leaving an observable trace of how many times the operand actually ran.
#[test]
fn pairwise_name_combiner_evaluates_a_shared_operand_once() {
    let region = run_root_storage();
    let (scope, captured) = run_root_with_buf(&region);
    register_pairwise_fixture(
        scope,
        Combiner::Name("mix".to_string()),
        FoldDirection::Left,
    );
    run(scope, "FN (LOUD x :Number) -> Number = ((PRINT x) (x))");
    let result = run_one(scope, parse_one("1 % (LOUD 2) % 3"));
    assert!(
        matches!(result, KObject::Number(n) if *n == -2.0),
        "the pairs are 1 + 2 = 3 and 2 + 3 = 5, folded through `mix` to 3 - 5 = -2; got {}",
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

/// The combiner is a *name*, resolved at the chain's use site through the ordinary scope walk —
/// the group record stores no resolved function. An unbound combiner name therefore surfaces as
/// an ordinary unbound-name error at the chain, not at registration: group validation is
/// deferred.
#[test]
fn pairwise_unbound_name_combiner_errors_at_the_use_site() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let members: HashSet<String> = ["%"].iter().map(|s| s.to_string()).collect();
    let group = scope.brand().alloc_operator_group(OperatorGroup::new(
        members,
        ReductionMode::Pairwise {
            combiner: Combiner::Name("nowhere".to_string()),
            direction: FoldDirection::Left,
        },
    ));
    scope
        .register_operator_group("%".to_string(), group, BindingIndex::BUILTIN)
        .expect("a group naming an unbound combiner registers fine");
    run(scope, "FN (a :Number % b :Number) -> Number = (a + b)");

    let error = crate::builtins::test_support::run_one_err(scope, parse_one("1 % 2 % 3"));
    let message = error.to_string();
    assert!(
        message.contains("nowhere"),
        "an unbound combiner must surface as an ordinary use-site name error; got: {message}",
    );
}
