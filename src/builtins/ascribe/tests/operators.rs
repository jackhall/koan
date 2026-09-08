//! The operator surface across the ascription barrier: a signature declares operator members and
//! groups, and a view publishes **both** halves — the dispatch buckets a keyworded member rides,
//! and a fresh chaining record over exactly the declared members, so a run inside a
//! `USING <view> SCOPE` window reduces by the declared mode.
//!
//! A run is reached by the reducer rather than by name, so every reduction here happens inside a
//! window. What these pin is one property per axis: a mixed run of declared members reduces, a run
//! naming an undeclared member does not (the registry half is a narrowing like the other two), the
//! unary triple's three surfaces all reach the view's wrapper, a view over a *builtin* operator
//! symbol ascribes at all, and the two satisfaction failures the operator channel adds.

use crate::builtins::test_support::TestRun;
use crate::machine::model::KObject;
use crate::machine::{program_storage, run_root_storage};

/// The mixed-run fixture: a module group chaining three operators, against a signature naming two
/// of them. Non-builtin glyphs throughout, so the root's own groups cannot mask a gap in the
/// view's registry — a builtin symbol like `+` would reduce through the root's group whatever the
/// view held.
fn ring_program() -> &'static str {
    "NEWTYPE Carrier = Number\n\
     SIG Ring = ((TYPE Elt) (VAL unit :Elt) \
     (GROUP FOLD LEFT = ((OP #(⊕) OVER Elt) (OP #(⊖) OVER Elt))))\n\
     GROUP rx FOLD LEFT = ((LET Elt = Carrier) (LET unit = (Carrier 0)) \
     (OP #(⊕) OVER Carrier = ((left))) (OP #(⊖) OVER Carrier = ((right))) \
     (OP #(⊗) OVER Carrier = ((left))))\n\
     LET view = (rx :| Ring)\n\
     LET tview = (rx :! Ring)\n\
     EXPR (TAKEELT x :(view.Elt)) -> Number = (3)"
}

/// **AC 3.** A mixed run of two declared members reduces inside the window: the view holds a
/// record over both, so the reducer's probe resolves and folds by the declared mode.
#[test]
fn a_mixed_run_of_declared_members_reduces_through_an_opaque_view() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(ring_program());
    let result =
        test_run.run_one(test_run.parse_one("USING view SCOPE (TAKEELT (unit ⊕ unit ⊖ unit))"));
    assert!(
        matches!(result, KObject::Number(n) if *n == 3.0),
        "the run reduces and its result inhabits the view's own `Elt`",
    );
}

/// The registry half is a **narrowing**, like the value and bucket halves: the source group also
/// chains `⊗`, but the signature never declared it, so a run naming it finds no group in the
/// window.
#[test]
fn a_run_naming_an_undeclared_member_does_not_reduce_in_the_window() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(ring_program());
    let error = test_run.run_one_err(test_run.parse_one("USING view SCOPE (unit ⊕ unit ⊗ unit)"));
    assert!(
        error
            .to_string()
            .contains("no operator group declares all of"),
        "got {error}",
    );
}

/// The transparent view installs the same record, over the source's concrete types.
#[test]
fn a_mixed_run_reduces_through_a_transparent_view_too() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(ring_program());
    let result = test_run.run_one(test_run.parse_one("USING tview SCOPE (unit ⊕ unit ⊖ unit)"));
    assert!(
        matches!(result, KObject::Wrapped { .. }),
        "a transparent view keeps the source's concrete Carrier",
    );
}

/// A view's self-sig carries the declared records, so the view structurally satisfies the very
/// signature it was ascribed to.
#[test]
fn a_views_self_sig_carries_the_declared_record() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(ring_program());
    let rendered = test_run
        .run_one_type(test_run.parse_one("TYPE OF view"))
        .name(test_run.registries());
    assert!(
        rendered.contains("GROUP FOLD LEFT {"),
        "the view's own signature names the record it installed, got {rendered}",
    );
}

// ---------- the unary triple ----------

fn unary_program() -> &'static str {
    "NEWTYPE Carrier = Number\n\
     SIG Coll = ((TYPE Elt) (VAL unit :Elt) (UNARY OP #(~) OVER Elt -> :(LIST OF Elt)))\n\
     MODULE um = ((LET Elt = Carrier) (LET unit = (Carrier 0)) \
     (UNARY OP #(~) OVER Carrier -> :(LIST OF Carrier) = (operands)))\n\
     LET view = (um :| Coll)"
}

/// One head declares the triple, so a view installs all three surfaces: the run reduces through
/// the `Unary` record into the list body, and the bridge and prefix spellings dispatch straight to
/// it.
#[test]
fn all_three_unary_surfaces_reach_the_views_body() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(unary_program());
    for (source, expected) in [
        ("USING view SCOPE (unit ~ unit ~ unit)", 3),
        ("USING view SCOPE (unit ~ unit)", 2),
        ("USING view SCOPE (~ [unit unit])", 2),
    ] {
        let result = test_run.run_one(test_run.parse_one(source));
        let KObject::List(items, _) = result else {
            panic!(
                "`{source}` returns the operand list, got {}",
                result.summarize(test_run.registries()),
            );
        };
        assert_eq!(items.elements().len(), expected, "for `{source}`");
    }
}

// ---------- the builtin-shadow door ----------

/// A signature may declare an operator the root already declares. The source's own overload passed
/// the builtin-shadow guard where it was written, so replaying it into a view is not a second
/// declaration — and shadowing stays type-gated: inside the same window an arithmetic run still
/// resolves to the builtin.
#[test]
fn a_view_over_a_builtin_operator_symbol_ascribes_and_stays_type_gated() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "NEWTYPE Carrier = Number\n\
         SIG Addable = ((TYPE Elt) (VAL unit :Elt) (OP #(+) OVER Elt))\n\
         MODULE m = ((LET Elt = Carrier) (LET unit = (Carrier 0)) \
         (OP #(+) OVER Carrier = ((left))))\n\
         LET view = (m :| Addable)",
    );
    let carrier_run = test_run.run_one(test_run.parse_one("USING view SCOPE (unit + unit + unit)"));
    assert!(
        matches!(carrier_run, KObject::Wrapped { .. }),
        "a Carrier-operand run reaches the view's own body",
    );
    let number_run = test_run.run_one(test_run.parse_one("USING view SCOPE (1 + 2 + 3)"));
    assert!(
        matches!(number_run, KObject::Number(n) if *n == 6.0),
        "arithmetic still resolves to the builtin: shadowing is type-gated, not free",
    );
}

// ---------- pairwise ----------

/// A pairwise group's members reduce through the view's own bodies and their pair results fold
/// through the combiner the window resolves — which is why a declared pairwise group's combiner
/// must itself be a declared member.
#[test]
fn a_pairwise_run_reduces_through_a_views_combiner() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "SIG Cmp = ((TYPE Elt) (VAL unit :Elt) \
         (GROUP PAIRWISE FOLD #(BOTH) LEFT = ((OP #(BOTH) OVER Bool) \
         (OP #(≺) OVER Elt -> Bool) (OP #(≼) OVER Elt -> Bool))))\n\
         GROUP cx PAIRWISE FOLD #(BOTH) LEFT = ((LET Elt = Number) (LET unit = 1) \
         (OP #(BOTH) OVER Bool = (left AND right)) \
         (OP #(≺) OVER Number -> Bool = (left < right)) \
         (OP #(≼) OVER Number -> Bool = (left <= right)))\n\
         LET view = (cx :! Cmp)",
    );
    let result = test_run.run_one(test_run.parse_one("USING view SCOPE (unit ≺ unit ≼ unit)"));
    assert!(
        matches!(result, KObject::Bool(false)),
        "`1 ≺ 1` is false and `1 ≼ 1` true, folded through BOTH: {}",
        result.summarize(test_run.registries()),
    );
}

// ---------- the satisfaction failures ----------

/// The ascription error a `source` produces.
fn ascription_error(source: &str) -> String {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let mut statements: Vec<&str> = source.split('\n').collect();
    let last = statements.pop().expect("at least one statement");
    test_run.run(&statements.join("\n"));
    test_run.run_one_err(test_run.parse_one(last)).to_string()
}

/// **AC 4.** A module supplying the bucket and no group supplies no chaining record: an `FN` head
/// over an operator key declares the bucket only, which is exactly what the `EXPR`-head spelling means.
#[test]
fn a_module_with_the_bucket_but_no_group_misses_the_declared_record() {
    let message = ascription_error(
        "NEWTYPE Carrier = Number\n\
         SIG Addable = ((TYPE Elt) (VAL unit :Elt) (OP #(⊕) OVER Elt))\n\
         MODULE m = ((LET Elt = Carrier) (LET unit = (Carrier 0)) \
         (EXPR (left :Carrier ⊕ right :Carrier) -> Carrier = ((left))))\n\
         LET v = (m :| Addable)",
    );
    assert!(
        message.contains("no chaining mode covers `⊕`"),
        "got {message}",
    );
}

/// **AC 4.** Two modules differing only in fold direction are distinguished: a mode is not an
/// approximation of another mode.
#[test]
fn a_module_folding_the_other_way_fails_the_declared_mode() {
    let message = ascription_error(
        "NEWTYPE Carrier = Number\n\
         SIG Ring = ((TYPE Elt) (VAL unit :Elt) \
         (GROUP FOLD LEFT = ((OP #(⊕) OVER Elt) (OP #(⊖) OVER Elt))))\n\
         GROUP rx FOLD RIGHT = ((LET Elt = Carrier) (LET unit = (Carrier 0)) \
         (OP #(⊕) OVER Carrier = ((left))) (OP #(⊖) OVER Carrier = ((right))))\n\
         LET v = (rx :| Ring)",
    );
    assert!(
        message.contains("chain fold-left in the signature but fold-right in the module"),
        "got {message}",
    );
}

/// **AC 2.** A keyworded failure over an operator member renders the operator head, not the FN
/// head its bucket key would otherwise spell.
#[test]
fn a_keyworded_failure_on_an_operator_member_renders_the_op_head() {
    let message = ascription_error(
        "NEWTYPE Carrier = Number\n\
         SIG Addable = ((TYPE Elt) (VAL unit :Elt) (OP #(⊕) OVER Elt))\n\
         MODULE m = ((LET Elt = Carrier) (LET unit = (Carrier 0)) \
         (OP #(⊕) OVER Str = ((left))))\n\
         LET v = (m :| Addable)",
    );
    assert!(
        message.contains("keyworded member `OP #(⊕) OVER Elt`"),
        "got {message}",
    );
}

/// A nested signature slot's view builds through the same `ViewMembers::of_schema` door, so it
/// installs the registry half too. The nested view's own self-sig is derived from its scope's
/// registry table, so a record showing up there *is* the record having been installed.
#[test]
fn a_nested_signature_slots_view_installs_the_declared_record() {
    use crate::machine::model::{TypeNode, values::Module};
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "NEWTYPE Carrier = Number\n\
         SIG Inner = ((TYPE Item) (VAL one :Item) \
         (GROUP FOLD LEFT = ((OP #(⊕) OVER Item) (OP #(⊖) OVER Item))))\n\
         SIG Outer = ((TYPE Elt) (VAL subs :(LIST OF (Inner WITH {Item = Elt}))))\n\
         GROUP ci FOLD LEFT = ((LET Item = Carrier) (LET one = (Carrier 1)) \
         (OP #(⊕) OVER Carrier = ((left))) (OP #(⊖) OVER Carrier = ((right))))\n\
         MODULE outer = ((LET Elt = Carrier) (LET subs = [ci]))\n\
         LET view = (outer :| Outer)",
    );
    let parsed = test_run.parse_one("view.subs");
    let subs = test_run.run_one(parsed);
    let KObject::List(items, _) = subs else {
        panic!("`subs` is a list of nested views");
    };
    let nested: &Module<'_> = match items.elements().first().and_then(|cell| cell.as_object()) {
        Some(KObject::Module(module)) => module,
        _ => panic!("the list holds one nested view"),
    };
    let record_count = nested.with_self_sig(test_run.types(), |sig| sig.operators.len());
    assert_eq!(
        record_count, 1,
        "the nested view's registry holds the one record its signature declares",
    );
    let rendered = test_run
        .types()
        .with_node(nested.ktype(), |node| match node {
            TypeNode::Signature { .. } => nested.ktype().name(test_run.registries()),
            _ => panic!("a view's ktype is its self-sig"),
        });
    assert!(
        rendered.contains("GROUP FOLD LEFT {"),
        "and names it, got {rendered}",
    );
}
