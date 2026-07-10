use crate::builtins::test_support::{parse_one, run_one_type, run_root_silent};
use crate::machine::core::run_root_storage;
use crate::machine::model::operators::ReductionMode;
use crate::machine::model::KType;

/// AC7: `|` is registered as a single-member `Unary`-mode operator group, so a `|` run reduces
/// through the unary reducer (`[Keyword("|"), ListLiteral(members)]`) into the constructor.
#[test]
fn pipe_is_a_unary_operator_group() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let group = scope
        .resolve_operator_group_with_chain("|", None)
        .expect("`|` must resolve to a registered operator group");
    assert!(
        matches!(group.mode(), ReductionMode::Unary),
        "`|` must reduce as a unary-mode group; got {:?}",
        group.mode(),
    );
}

/// The two-member keyworded form `:(Number | Str)` lowers to a canonical `Union`.
#[test]
fn two_member_union_lowers_to_union() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let result = run_one_type(scope, parse_one(":(Number | Str)"));
    assert_eq!(
        *result,
        KType::union_of(vec![KType::Number, KType::Str]),
        "two-member `|` builds the union of its members",
    );
}

/// A three-member run reduces through the `Unary` group and the body sees all members at once
/// (AC7): the result is the flat three-member union.
#[test]
fn three_member_run_builds_flat_union() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let result = run_one_type(scope, parse_one(":(Number | Str | Bool)"));
    assert_eq!(
        *result,
        KType::union_of(vec![KType::Number, KType::Str, KType::Bool]),
    );
}

/// `:(Number | Number)` collapses to `:Number` (AC2 idempotency).
#[test]
fn duplicate_member_collapses() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let result = run_one_type(scope, parse_one(":(Number | Number)"));
    assert_eq!(*result, KType::Number);
}

/// Member order does not matter (AC2): `:(Number | Str)` equals `:(Str | Number)`.
#[test]
fn member_order_blind() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let forward = run_one_type(scope, parse_one(":(Number | Str)"));
    let backward = run_one_type(scope, parse_one(":(Str | Number)"));
    assert_eq!(*forward, *backward);
}

/// The explicit prefix form `:(| [Number Str Bool])` reaches the same n-ary body as the infix
/// run — the "prefix and infix coincide" direction for the `|` unary group.
#[test]
fn prefix_form_builds_union() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let result = run_one_type(scope, parse_one(":(| [Number Str Bool])"));
    assert_eq!(
        *result,
        KType::union_of(vec![KType::Number, KType::Str, KType::Bool]),
    );
}

/// A parenthesized compound member `:((LIST OF Number) | Str)` resolves: the `:(...)` member
/// sub-dispatches to `List(Number)` before the union folds.
#[test]
fn parenthesized_compound_member() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let result = run_one_type(scope, parse_one(":((LIST OF Number) | Str)"));
    assert_eq!(
        *result,
        KType::union_of(vec![KType::List(Box::new(KType::Number)), KType::Str]),
    );
}
