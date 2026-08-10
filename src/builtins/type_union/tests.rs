use crate::builtins::test_support::{TestRun, parse_one};
use crate::machine::model::ReductionMode;
use crate::machine::model::{KType, TypeNode};
use crate::machine::{program_storage, run_root_storage};

/// AC7: `|` is registered as a single-member `Unary`-mode operator group, so a `|` run reduces
/// through the unary reducer (`[Keyword("|"), ListLiteral(members)]`) into the constructor.
#[test]
fn pipe_is_a_unary_operator_group() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let group = scope
        .resolve_operator_group_delivered("|", None)
        .expect("`|` must resolve to a registered operator group");
    let mode = group.open(|group| format!("{:?}", group.mode()));
    assert!(
        group.open(|group| matches!(group.mode(), ReductionMode::Unary)),
        "`|` must reduce as a unary-mode group; got {mode}",
    );
}

/// The two-member keyworded form `:(Number | Str)` lowers to a canonical `Union`.
#[test]
fn two_member_union_lowers_to_union() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let result = test_run.run_one_type(parse_one(&program, ":(Number | Str)"));
    let types = test_run.types();
    assert_eq!(
        result,
        types.union_of(vec![KType::NUMBER, KType::STR]),
        "two-member `|` builds the union of its members",
    );
}

/// A three-member run reduces through the `Unary` group and the body sees all members at once
/// (AC7): the result is the flat three-member union.
#[test]
fn three_member_run_builds_flat_union() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let result = test_run.run_one_type(parse_one(&program, ":(Number | Str | Bool)"));
    let types = test_run.types();
    assert_eq!(
        result,
        types.union_of(vec![KType::NUMBER, KType::STR, KType::BOOL]),
    );
}

/// `:(Number | Number)` collapses to `:Number` (AC2 idempotency).
#[test]
fn duplicate_member_collapses() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let result = test_run.run_one_type(parse_one(&program, ":(Number | Number)"));
    assert_eq!(result, KType::NUMBER);
}

/// Member order does not matter (AC2): `:(Number | Str)` equals `:(Str | Number)`.
#[test]
fn member_order_blind() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let forward = test_run.run_one_type(parse_one(&program, ":(Number | Str)"));
    let backward = test_run.run_one_type(parse_one(&program, ":(Str | Number)"));
    assert_eq!(forward, backward);
}

/// The explicit prefix form `:(| [Number Str Bool])` reaches the same n-ary body as the infix
/// run — the "prefix and infix coincide" direction for the `|` unary group.
#[test]
fn prefix_form_builds_union() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let result = test_run.run_one_type(parse_one(&program, ":(| [Number Str Bool])"));
    let types = test_run.types();
    assert_eq!(
        result,
        types.union_of(vec![KType::NUMBER, KType::STR, KType::BOOL]),
    );
}

/// A parenthesized compound member `:((LIST OF Number) | Str)` resolves: the `:(...)` member
/// sub-dispatches to `List(Number)` before the union folds.
#[test]
fn parenthesized_compound_member() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let result = test_run.run_one_type(parse_one(&program, ":((LIST OF Number) | Str)"));
    let types = test_run.types();
    assert_eq!(
        result,
        types.union_of(vec![types.list(KType::NUMBER), KType::STR]),
    );
}

/// The two-member keyworded form `:(Wrapped | Number)` correlates a reaching member (a `NEWTYPE`
/// alias, carried through `type_operand`'s `Reaching` arm) with a scalar-literal member (`Number`,
/// which has no carrier and rebuilds at the brand via the `Pure` arm) into a flat union.
#[test]
fn binary_union_with_reaching_member_correlates() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("NEWTYPE Wrapped = :{a :Number}");
    let result = test_run.run_one_type(parse_one(&program, ":(Wrapped | Number)"));
    let types = test_run.types();
    match types.node(result) {
        TypeNode::Union { members } => {
            assert_eq!(members.len(), 2, "expected a flat two-member union");
            assert!(
                members.iter().any(|m| m.name(types) == "Wrapped"),
                "the reaching member must survive the carrier-view crossing, got {members:?}",
            );
            assert!(
                members.contains(&KType::NUMBER),
                "the scalar member must lower to Number, got {members:?}",
            );
        }
        _ => panic!("expected a Union carrier, got {result:?}"),
    }
}

/// The reduced n-ary form `:(Wrapped | Number | Str)` composes all three members — each cloned
/// out of its own carrier, since `expect_type_terminal` yields a carrier for every sub-dispatched
/// member — into a flat three-member union.
#[test]
fn nary_union_with_reaching_member_correlates() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("NEWTYPE Wrapped = :{a :Number}");
    let result = test_run.run_one_type(parse_one(&program, ":(Wrapped | Number | Str)"));
    let types = test_run.types();
    match types.node(result) {
        TypeNode::Union { members } => {
            assert_eq!(members.len(), 3, "expected a flat three-member union");
            assert!(
                members.iter().any(|m| m.name(types) == "Wrapped"),
                "the reaching member must survive the carrier-view crossing, got {members:?}",
            );
            assert!(members.contains(&KType::NUMBER), "got {members:?}");
            assert!(members.contains(&KType::STR), "got {members:?}");
        }
        _ => panic!("expected a Union carrier, got {result:?}"),
    }
}

/// A signature is a type value, so it can be a union member: `:(Number | S)` lowers to a
/// union whose signature arm admits a satisfying module value and whose `Number` arm admits a
/// number — both through one dispatch slot.
#[test]
fn union_with_signature_member_admits_module_and_number() {
    use crate::machine::model::KObject;
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "SIG HasLabel = ((VAL label :Str))\n\
         MODULE widget = ((LET label = (\"button\")))\n\
         FN (EITHER x :(Number | HasLabel)) -> Str = ((\"admitted\"))",
    );
    for call in ["EITHER widget", "EITHER 5"] {
        match test_run.run_one(parse_one(&program, call)) {
            KObject::KString(s) => assert_eq!(*s, "admitted", "for `{call}`"),
            other => panic!(
                "`{call}` should dispatch, got {}",
                other.ktype().name(&test_run.types)
            ),
        }
    }
}
