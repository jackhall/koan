//! The bodyless `GROUP <mode> = (<heads>)` form: a SIG body's group member. Its heads write their
//! own keyworded members and the group writes the one chaining record over exactly them, so these
//! tests pin the split — and the refusals that keep declaration and definition in their own
//! bodies.

use crate::builtins::test_support::{TestRun, lookup_type};
use crate::machine::KErrorKind;
use crate::machine::model::{DeclaredGroup, FoldDirection, ReductionMode, SigSchema, TypeNode};
use crate::memory::{program_storage, run_root_storage};
use crate::parse::KeywordSymbol;

fn sig_schema(
    scope: &crate::machine::Scope<'_>,
    types: &crate::machine::model::TypeRegistry,
    name: &str,
) -> SigSchema {
    let handle = lookup_type(scope, name).unwrap_or_else(|| panic!("{name} must bind a type"));
    match types.node(handle) {
        TypeNode::Signature { schema, .. } => schema,
        _ => panic!("{name} must bind a Signature KType"),
    }
}

fn record(members: &[&str], mode: ReductionMode) -> DeclaredGroup {
    let mut members: Vec<KeywordSymbol> = members
        .iter()
        .map(|text| KeywordSymbol::of(text).expect("a fixture glyph is keyword-class"))
        .collect();
    members.sort_unstable();
    DeclaredGroup { members, mode }
}

fn refusal(source: &str) -> String {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let error = test_run.run_one_err(test_run.parse_one(source));
    match &error.kind {
        KErrorKind::ShapeError(message) => message.clone(),
        _ => panic!("expected a shape error, got {error}"),
    }
}

/// A declared group writes **one** record over its members and no singletons: inside a group the
/// group is the sole registrar, exactly as it is for a definition's members.
#[test]
fn a_declared_group_writes_one_record_and_its_members_write_none() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "SIG Ring = ((LET Carrier = Number) \
         (GROUP FOLD RIGHT = ((OP #(⊕) OVER Carrier) (OP #(⊖) OVER Carrier))))",
    );
    let schema = sig_schema(scope, test_run.types(), "Ring");
    assert_eq!(
        schema.operators,
        vec![record(&["⊕", "⊖"], ReductionMode::FoldRight)],
    );
    // Both heads still declared their buckets — only the registry half moved to the group.
    assert_eq!(schema.keyworded.len(), 2);
}

/// A pairwise group carries its combiner and direction, and its members may be heterogeneous —
/// the pair results fold through the combiner, so a member's result need not be its operand.
#[test]
fn a_pairwise_group_admits_heterogeneous_members() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "SIG Cmp = ((LET Carrier = Number) (OP #(BOTH) OVER Bool) \
         (GROUP PAIRWISE FOLD #(BOTH) LEFT = ((OP #(≺) OVER Carrier -> Bool))))",
    );
    let schema = sig_schema(scope, test_run.types(), "Cmp");
    let combiner = KeywordSymbol::of("BOTH").expect("`BOTH` is keyword-class");
    let pairwise = record(
        &["≺"],
        ReductionMode::Pairwise {
            combiner,
            direction: FoldDirection::Left,
        },
    );
    // The combiner's own bare head declares its singleton beside the group's record; the channel
    // is stored canonically, so the pair is compared as a set.
    assert_eq!(schema.operators.len(), 2);
    assert!(schema.operators.contains(&pairwise));
    assert!(
        schema
            .operators
            .contains(&record(&["BOTH"], ReductionMode::FoldLeft))
    );
}

/// The combiner a pairwise group names must be a member the signature declares: a `USING` window
/// resolves it by the ordinary scope walk, so only a declared combiner reaches a view's window.
/// Checked at the SIG finish, so a combiner declared **after** the group still counts.
#[test]
fn a_pairwise_combiner_may_be_declared_after_its_group() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "SIG Cmp = ((LET Carrier = Number) \
         (GROUP PAIRWISE FOLD #(BOTH) LEFT = ((OP #(≺) OVER Carrier -> Bool))) \
         (OP #(BOTH) OVER Bool))",
    );
    assert!(lookup_type(scope, "Cmp").is_some());
}

#[test]
fn an_undeclared_pairwise_combiner_is_refused() {
    let message = refusal(
        "SIG Bad = ((LET Carrier = Number) \
         (GROUP PAIRWISE FOLD #(BOTH) LEFT = ((OP #(≺) OVER Carrier -> Bool))))",
    );
    assert!(
        message.contains("names a combiner the signature does not declare")
            && message.contains("(OP #(BOTH) OVER <Result>)"),
        "got {message}",
    );
}

#[test]
fn a_bodyless_group_outside_a_sig_body_points_at_the_definition() {
    let message = refusal("GROUP FOLD LEFT = ((OP #(⊕) OVER Str))");
    assert!(
        message.contains("only valid inside a SIG body")
            && message.contains("GROUP <name> FOLD LEFT = (<body>)"),
        "got {message}",
    );
}

#[test]
fn a_group_definition_inside_a_sig_body_points_at_the_bodyless_form() {
    let message = refusal("SIG Bad = ((GROUP g FOLD LEFT = ((OP #(⊕) OVER Number = ((left))))))");
    assert!(
        message.contains("declared rather than defined")
            && message.contains("GROUP FOLD LEFT = ((OP #(<sym>) OVER <Operand>) …)"),
        "got {message}",
    );
}

/// A SIG group body holds heads only: any other statement would be a member the scan skips, so
/// the record would quietly not describe the body that was written.
#[test]
fn a_non_head_statement_in_a_sig_group_body_is_refused() {
    let message = refusal("SIG Bad = ((TYPE Carrier) (GROUP FOLD LEFT = ((VAL x :Carrier))))");
    assert!(
        message.contains("holds operator heads only"),
        "got {message}",
    );
}
