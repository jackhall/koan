//! `GROUP` surface tests: what a group registers, how a mixed-member run reduces (inside the body
//! and through a `USING` window), the pairwise combiner fold, and the errors the surface rejects.
//!
//! - [`functor`] — a `GROUP` in the body of a module-returning `FN`, instantiated explicitly.

mod functor;

use crate::builtins::test_support::{
    binds_module, parse_one, run, run_one, run_one_err, run_root_silent, run_root_with_buf,
};
use crate::machine::core::run_root_storage;
use crate::machine::model::values::Held;
use crate::machine::model::{KObject, Parseable};
use crate::machine::KErrorKind;

/// The numbers of a `KObject::List` — the member bodies below return one of their two list
/// operands, so association is observable in which list comes back.
fn list_numbers(object: &KObject<'_>) -> Vec<f64> {
    match object {
        KObject::List(items, _) => items
            .iter()
            .map(|item| match item {
                Held::Object(KObject::Number(n)) => *n,
                other => panic!("expected a Number element, got {}", other.summarize()),
            })
            .collect(),
        other => panic!("expected a list, got {}", other.ktype().name()),
    }
}

/// The three list operands every `vec_ops` run below chains.
const LISTS: &str = "LET xs = [1]\nLET ys = [2]\nLET zs = [3]\n";

/// AC2: a `GROUP` declares two operators, and a run mixing them reduces fold-left through both
/// member bodies — inside the group's own body (the member powerset is registered before the body
/// runs) and inside a `USING` window, which surfaces the registry entries and the function buckets
/// together.
///
/// `+` returns its left operand and `-` its right, so a fold-left `xs + ys - zs` = `(xs + ys) - zs`
/// = `xs - zs` = `zs`. Both members run: neither one alone could produce `zs`.
#[test]
fn group_mixed_run_reduces_fold_left_inside_the_body_and_through_using() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        &format!(
            "{LISTS}\
             GROUP vec_ops FOLD LEFT = (\
               (OP #(+) OVER :(LIST OF Number) = (left))\
               (OP #(-) OVER :(LIST OF Number) = (right))\
               (LET inside = (xs + ys - zs)))\n\
             LET outside = (USING vec_ops SCOPE (xs + ys - zs))",
        ),
    );
    assert_eq!(
        list_numbers(run_one(scope, parse_one("vec_ops.inside"))),
        vec![3.0],
        "the mixed run reduces fold-left through both member bodies inside the group body",
    );
    assert_eq!(
        list_numbers(run_one(scope, parse_one("outside"))),
        vec![3.0],
        "a USING window surfaces the group's registry entries alongside its operator bodies",
    );
}

/// The same members under `FOLD RIGHT` nest the other way — `xs + (ys - zs)` = `xs + zs` = `xs` —
/// which also pins the innermost-wins registry walk: the root's builtin `+ -` group is fold-left,
/// and the group's own record overrides it for the operand types the group declares.
#[test]
fn group_fold_right_nests_right_associated() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        &format!(
            "{LISTS}\
             GROUP vec_ops FOLD RIGHT = (\
               (OP #(+) OVER :(LIST OF Number) = (left))\
               (OP #(-) OVER :(LIST OF Number) = (right))\
               (LET inside = (xs + ys - zs)))",
        ),
    );
    assert_eq!(
        list_numbers(run_one(scope, parse_one("vec_ops.inside"))),
        vec![1.0],
        "a fold-right run nests `xs + (ys - zs)`, which returns `xs`",
    );
}

/// A `PAIRWISE` group with **heterogeneous** members (`Number -> Bool`, admissible only here) and a
/// combiner declared as an `OP` over the pair-result type. Each adjacent pair dispatches through its
/// own member's body and the two `Bool` results fold through the combiner, so the chain answers
/// whether the whole run is ordered.
#[test]
fn pairwise_group_folds_heterogeneous_pairs_through_the_combiner() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "GROUP num_compare PAIRWISE FOLD #(BOTH) LEFT = (\
           (OP #(BOTH) OVER Bool = (left AND right))\
           (OP #(≺) OVER Number -> Bool = (left < right))\
           (OP #(≼) OVER Number -> Bool = (left <= right))\
           (LET ordered = (1 ≺ 2 ≼ 3))\
           (LET unordered = (3 ≺ 2 ≼ 3)))",
    );
    assert!(
        matches!(run_one(scope, parse_one("num_compare.ordered")), KObject::Bool(b) if *b),
        "1 ≺ 2 and 2 ≼ 3 both hold, so the combiner folds them to true",
    );
    assert!(
        matches!(run_one(scope, parse_one("num_compare.unordered")), KObject::Bool(b) if !*b),
        "3 ≺ 2 fails, so the combiner folds the pair results to false",
    );
}

/// The pairwise combiner takes the group's fold direction. The `%` bodies sum their operand pair and
/// the `⊖` combiner subtracts, so over `10 % 4 % 1 % 0` the pairs `14`, `5`, `1` fold left to
/// `(14 - 5) - 1` = 8 and right to `14 - (5 - 1)` = 10.
#[test]
fn pairwise_group_folds_pair_results_in_the_declared_direction() {
    for (direction, expected) in [("LEFT", 8.0), ("RIGHT", 10.0)] {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(
            scope,
            &format!(
                "GROUP tally PAIRWISE FOLD #(⊖) {direction} = (\
                   (OP #(%) OVER Number = (left + right))\
                   (OP #(⊖) OVER Number = (left - right))\
                   (LET folded = (10 % 4 % 1 % 0)))",
            ),
        );
        let result = run_one(scope, parse_one("tally.folded"));
        assert!(
            matches!(result, KObject::Number(n) if *n == expected),
            "a {direction} fold of the pair results must give {expected}; got {}",
            result.summarize(),
        );
    }
}

/// A shared middle operand of a pairwise run feeds both adjacent pairs but evaluates **once**:
/// `LOUD` prints its argument and returns it unchanged, and the chain's pairs (`3` and `5`, folded
/// through `⊖` to `-2`) leave exactly one line of output.
#[test]
fn pairwise_group_evaluates_a_shared_operand_once() {
    let region = run_root_storage();
    let (scope, captured) = run_root_with_buf(&region);
    run(
        scope,
        "FN (LOUD x :Number) -> Number = ((PRINT x) (x))\n\
         GROUP tally PAIRWISE FOLD #(⊖) LEFT = (\
           (OP #(%) OVER Number = (left + right))\
           (OP #(⊖) OVER Number = (left - right))\
           (LET once = (1 % (LOUD 2) % 3)))",
    );
    assert!(
        matches!(run_one(scope, parse_one("tally.once")), KObject::Number(n) if *n == -2.0),
        "the pairs are 1 + 2 = 3 and 2 + 3 = 5, folded through `⊖` to 3 - 5 = -2",
    );
    let bytes = captured.borrow().clone();
    assert_eq!(
        bytes,
        b"2\n",
        "the shared middle operand must dispatch exactly once; got {:?}",
        String::from_utf8_lossy(&bytes),
    );
}

/// Non-`OP` body statements are ordinary module content — the member scan ignores them, and they
/// bind as members of the group's module value like any other module body statement.
#[test]
fn group_body_holds_ordinary_module_statements() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "GROUP shifts FOLD LEFT = (\
           (LET bump = 10)\
           (OP #(⊕) OVER Number = ((left + right) + bump))\
           (LET total = (1 ⊕ 2 ⊕ 3)))",
    );
    assert!(
        matches!(run_one(scope, parse_one("shifts.bump")), KObject::Number(n) if *n == 10.0),
        "a LET in a group body binds a member of the group's module value",
    );
    // fold-left: (1 ⊕ 2) = 13, (13 ⊕ 3) = 26 — the body reads its sibling `bump` through the scope
    // it captures, exactly as a module-level `OP` does.
    assert!(matches!(run_one(scope, parse_one("shifts.total")), KObject::Number(n) if *n == 26.0),);
}

/// A unary operator takes the whole run as one list, so it chains with nothing: the member scan
/// refuses it before the group is allocated.
#[test]
fn unary_op_in_a_group_body_errors() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let error = run_one_err(
        scope,
        parse_one(
            "GROUP gather FOLD LEFT = (\
               (UNARY OP #(~) OVER Number -> :(LIST OF Number) = (operands)))",
        ),
    );
    assert!(
        matches!(&error.kind, KErrorKind::ShapeError(msg) if msg.contains("chains with nothing")),
        "expected the unary-in-a-group diagnostic, got {error}",
    );
    assert!(
        !binds_module(scope, "gather"),
        "the refused group binds nothing",
    );
}

/// A heterogeneous member is admissible only where a combiner folds the pair results — inside a
/// `PAIRWISE` group. In a `FOLD` group the member's explicit `-> Result` is the error `OP` reports
/// against its group context, and the failing body statement short-circuits the group's finalize.
#[test]
fn heterogeneous_member_in_a_fold_group_errors() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let error = run_one_err(
        scope,
        parse_one(
            "GROUP bad_fold FOLD LEFT = (\
               (OP #(≺) OVER Number -> Bool = (left < right))\
               (OP #(≼) OVER Number -> Bool = (left <= right)))",
        ),
    );
    assert!(
        error.to_string().contains("PAIRWISE"),
        "expected the member's PAIRWISE diagnostic to surface at the group, got {error}",
    );
    assert!(
        !binds_module(scope, "bad_fold"),
        "a member declaring `-> Result` outside a PAIRWISE group fails the group's body",
    );
}

/// A group's members come from its body: a body declaring no operator declares no group.
#[test]
fn empty_group_errors() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let error = run_one_err(scope, parse_one("GROUP empty FOLD LEFT = (LET x = 1)"));
    assert!(
        matches!(&error.kind, KErrorKind::ShapeError(msg) if msg.contains("at least one")),
        "expected the empty-group diagnostic, got {error}",
    );
}

/// A group is a module and a module is a value, so a Type-token group name takes the same
/// respelling diagnostic MODULE's Type-named overload reports.
#[test]
fn type_token_group_name_errors_with_the_snake_case_respelling() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let error = run_one_err(
        scope,
        parse_one("GROUP VecOps FOLD LEFT = ((OP #(⊕) OVER Number = (left)))"),
    );
    assert!(
        matches!(&error.kind, KErrorKind::ShapeError(msg)
            if msg.contains("a module is a value") && msg.contains("`vec_ops`")),
        "expected the snake_case respelling diagnostic, got {error}",
    );
}
