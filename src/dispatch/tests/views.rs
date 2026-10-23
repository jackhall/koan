//! Views the load sees all of: at a `:|` site over a closed content tree the root holds every type
//! the view door computes, so the run builds the view without interning a type of its own — a
//! nested module's view among them.

use std::cell::Cell;

use crate::program::{CellSubstrate, KBirth, KBundle, KState, Outcome};
use crate::scheduler::{Action, Step, StepError};

use super::super::Koan;
use super::{assert_linear, load_counts, output};
use crate::values::SpecBounds;

thread_local! {
    /// What [`branch_length`] read.
    static BRANCH: Cell<usize> = const { Cell::new(0) };
}

/// A root work over the top level's resting view: it records the root cell's branch length.
fn branch_length<'graph>(
    step: Step<'_, 'graph, '_, '_, '_, '_, KBundle>,
) -> Action<'graph, KBundle> {
    let (step, state) = step.state();
    let KState::Born(birth @ KBirth::Inspect { .. }) = state else {
        return step.failed(StepError::Refused);
    };
    BRANCH.with(|branch| {
        branch.set(
            step.header()
                .get()
                .and_then(|header| header.branch())
                .map_or(0, |branch| branch.len()),
        )
    });
    step.leave(birth)
}

/// The root cell's branch length once `source` has run.
fn branch_after(source: &str) -> usize {
    let mut substrate =
        CellSubstrate::load::<Koan>(source, "<test>", 2, SpecBounds::default(), output())
            .unwrap_or_else(|error| panic!("the program loads: {error}"));
    assert_eq!(
        substrate.with(|running| running.run()),
        Ok(Outcome::Completed)
    );
    substrate
        .with(|running| running.inspect(branch_length))
        .expect("the inspection runs");
    BRANCH.with(Cell::get)
}

#[test]
fn a_view_of_a_body_over_literals_lays_no_type_at_run() {
    let source = "SIG Counter FOR ALL #[Carrier] = \
                  #[(VAL zero :Carrier) (VAL succ :(FN :{x :Carrier} -> Carrier))]\n\
                  MODULE ints = ((LET zero = 0) \
                  (LET succ = (FN :{x :Number} -> Number = #(x + 1))))\n\
                  LET m = (ints :| Counter)\n";
    assert_eq!(branch_after(source), 0, "the root holds the view's types");
}

/// A slot declared at an application over the outer parameter re-views the nested module.
#[test]
fn a_view_over_a_nested_module_lays_its_nested_view_at_load() {
    let source = "SIG Inner FOR ALL #[Elem] = #[(VAL item :Elem)]\n\
                  SIG Outer FOR ALL #[Carrier] = \
                  #[(VAL zero :Carrier) (VAL inner :(Inner WITH {Elem = Carrier}))]\n\
                  MODULE ints = ((LET zero = 0) \
                  (MODULE inner = ((LET Elem = Number) (LET item = 5))))\n\
                  LET m = (ints :| Outer)\n";
    assert_eq!(
        branch_after(source),
        0,
        "the root holds the nested view's types"
    );
}

/// A nested module in a record field is re-viewed too, and the record rebuilt at its slot's type.
#[test]
fn a_view_over_a_record_holding_a_module_lays_its_nested_view_at_load() {
    let source = "SIG Inner FOR ALL #[Elem] = #[(VAL item :Elem)]\n\
                  SIG Outer FOR ALL #[Carrier] = \
                  #[(VAL zero :Carrier) (VAL pair :{inner :(Inner WITH {Elem = Carrier})})]\n\
                  MODULE inner = ((LET Elem = Number) (LET item = 5))\n\
                  MODULE ints = ((LET zero = 0) (LET pair = {inner = inner}))\n\
                  LET m = (ints :| Outer)\n";
    assert_eq!(
        branch_after(source),
        0,
        "the root holds the nested view's types"
    );
}

/// `Inner`, an `Outer` holding a list of its applications, and modules of two signatures, for a
/// view over a list of nested modules.
const NESTED_LIST: &str = "SIG Inner FOR ALL #[Elem] = #[(VAL item :Elem)]\n\
                           SIG Outer FOR ALL #[Carrier] = #[(VAL zero :Carrier) \
                           (VAL inners :(LIST OF (Inner WITH {Elem = Carrier})))]\n\
                           MODULE a = ((LET Elem = Number) (LET item = 5))\n\
                           MODULE b = ((LET Elem = Number) (LET item = 6) (LET more = 7))\n";

/// An empty list's element type is `Never`, which names no module to re-view.
#[test]
fn a_view_over_an_empty_list_of_modules_lays_no_type_at_run() {
    let source = format!(
        "{NESTED_LIST}MODULE ints = ((LET zero = 0) (LET inners = []))\nLET m = (ints :| Outer)\n"
    );
    assert_eq!(branch_after(&source), 0);
}

/// A list of modules of two signatures has its element type at their union, and the load lays the
/// nested view of each.
#[test]
fn a_view_over_a_list_of_modules_of_two_signatures_lays_each_nested_view_at_load() {
    let source = format!(
        "{NESTED_LIST}MODULE ints = ((LET zero = 0) (LET inners = [a b]))\nLET m = (ints :| Outer)\n"
    );
    assert_eq!(branch_after(&source), 0);
}

/// A view of a module retyped by `:!`: the header's application declares the nested module at an
/// application fixing each parameter, which the load re-views as the run does the module.
#[test]
fn a_view_of_a_retyped_module_lays_its_nested_view_at_load() {
    let source = "SIG Inner FOR ALL #[Elem] = #[(VAL item :Elem)]\n\
                  SIG Outer FOR ALL #[Carrier] = \
                  #[(VAL zero :Carrier) (VAL inner :(Inner WITH {Elem = Carrier}))]\n\
                  MODULE a = ((LET Elem = Number) (LET item = 5))\n\
                  MODULE ints = ((LET zero = 0) (LET inner = a))\n\
                  LET t = (ints :! (Outer WITH {Carrier = Number}))\n\
                  LET m = (t :| Outer)\n";
    assert_eq!(branch_after(source), 0);
}

/// `Deep` nested in `Inner` nested in `Outer`, and modules of each, for views two deep.
const DEEP: &str = "SIG Deep FOR ALL #[Depth] = #[(VAL v :Depth)]\n\
                    SIG Inner FOR ALL #[Elem] = \
                    #[(VAL item :Elem) (VAL deep :(Deep WITH {Depth = Elem}))]\n\
                    SIG Outer FOR ALL #[Carrier] = \
                    #[(VAL zero :Carrier) (VAL inner :(Inner WITH {Elem = Carrier}))]\n\
                    MODULE d = ((LET Depth = Number) (LET v = 1))\n\
                    MODULE a = ((LET Elem = Number) (LET item = 5) (LET deep = d))\n\
                    MODULE ints = ((LET zero = 0) (LET inner = a))\n";

/// A module two deep under a header: the load reads what each carries off the header's source.
#[test]
fn a_module_two_deep_under_a_header_lays_its_nested_views_at_load() {
    let source = format!(
        "{DEEP}LET t = (ints :! (Outer WITH {{Carrier = Number}}))\nLET m = (t :| Outer)\n"
    );
    assert_eq!(branch_after(&source), 0);
}

/// A retype of a retyped module is one header over its bare member.
#[test]
fn a_retype_of_a_retyped_module_lays_its_nested_views_at_load() {
    let source = format!(
        "{DEEP}LET t = (ints :! (Outer WITH {{Carrier = Number}}))\nLET u = (t :! Outer)\n\
         LET m = (u :| Outer)\n"
    );
    assert_eq!(branch_after(&source), 0);
}

/// An opaque view's nested modules carry their re-views, which the load computes again.
#[test]
fn a_view_of_an_opaque_view_lays_its_nested_views_at_load() {
    let source = format!("{DEEP}LET o = (ints :| Outer)\nLET m = (o :| Outer)\n");
    assert_eq!(branch_after(&source), 0);
}

/// A module in a record, a list or a dict under a header.
#[test]
fn a_container_of_modules_under_a_header_lays_its_nested_views_at_load() {
    let inner = "SIG Inner FOR ALL #[Elem] = #[(VAL item :Elem)]\n\
                 MODULE a = ((LET Elem = Number) (LET item = 5))\n";
    for (slot, value) in [
        ("{inner :(Inner WITH {Elem = Carrier})}", "{inner = a}"),
        ("(LIST OF (Inner WITH {Elem = Carrier}))", "[a a]"),
        ("(MAP Str -> (Inner WITH {Elem = Carrier}))", "{\"k\": a}"),
    ] {
        let source = format!(
            "{inner}SIG Outer FOR ALL #[Carrier] = #[(VAL zero :Carrier) (VAL held :{slot})]\n\
             MODULE ints = ((LET zero = 0) (LET held = {value}))\n\
             LET t = (ints :! (Outer WITH {{Carrier = Number}}))\nLET m = (t :| Outer)\n"
        );
        assert_eq!(branch_after(&source), 0, "{slot}");
    }
}

/// A slot that leaves a parameter of the nested signature unset: the module's own signature fixes
/// it, which the load reads off the view's source.
#[test]
fn a_slot_leaving_a_nested_parameter_unset_lays_its_nested_view_at_load() {
    let source = "SIG Inner FOR ALL #[Elem Extra] = #[(VAL item :Elem) (VAL extra :Extra)]\n\
                  SIG Outer FOR ALL #[Carrier] = \
                  #[(VAL zero :Carrier) (VAL inner :(Inner WITH {Elem = Carrier}))]\n\
                  MODULE a = ((LET Elem = Number) (LET Extra = Str) (LET item = 5) \
                  (LET extra = \"s\"))\n\
                  MODULE ints = ((LET zero = 0) (LET inner = a))\n\
                  LET t = (ints :! (Outer WITH {Carrier = Number}))\n\
                  LET m = (t :| Outer)\n";
    assert_eq!(branch_after(source), 0);
}

/// A view inside a record literal has no tree, so its own nested modules are read as its
/// signature declares them: at applications fixing every parameter, which the load re-views alike.
#[test]
fn a_view_inside_a_record_literal_lays_its_nested_views_at_load() {
    let source = format!(
        "{DEEP}SIG Holder FOR ALL #[Carrier] = \
         #[(VAL zero :Carrier) (VAL pair :{{inner :(Inner WITH {{Elem = Carrier}})}})]\n\
         MODULE held = ((LET zero = 0) \
         (LET pair = {{inner = (a :! (Inner WITH {{Elem = Number}}))}}))\n\
         LET m = (held :| Holder)\n"
    );
    assert_eq!(branch_after(&source), 0);
}

/// Past the tree's horizon, a nested module whose slot leaves a parameter of its signature open:
/// the load cannot read what the module fixes it to, so the run re-views it.
#[test]
fn a_view_inside_a_record_literal_over_an_open_slot_runs() {
    let source = "SIG Deep FOR ALL #[Depth Extra] = #[(VAL v :Depth) (VAL x :Extra)]\n\
                  SIG Inner FOR ALL #[Elem] = \
                  #[(VAL item :Elem) (VAL deep :(Deep WITH {Depth = Elem}))]\n\
                  SIG Holder FOR ALL #[Carrier] = \
                  #[(VAL zero :Carrier) (VAL pair :{inner :(Inner WITH {Elem = Carrier})})]\n\
                  MODULE d = ((LET Depth = Number) (LET Extra = Str) (LET v = 1) (LET x = \"s\"))\n\
                  MODULE a = ((LET Elem = Number) (LET item = 5) (LET deep = d))\n\
                  MODULE held = ((LET zero = 0) \
                  (LET pair = {inner = (a :! (Inner WITH {Elem = Number}))}))\n\
                  LET m = (held :| Holder)\n";
    branch_after(source);
}

/// A retype of a module the load types exactly lays the types its members are read at at load.
#[test]
fn a_retype_lays_its_types_at_load() {
    let source = format!("{DEEP}LET t = (ints :! (Outer WITH {{Carrier = Number}}))\n");
    assert_eq!(branch_after(&source), 0);
}

/// A list or a map of modules of two signatures under an opaque view, viewed again: each cell is
/// re-viewed at its own signature, as the run re-views it.
#[test]
fn a_view_of_an_opaque_view_over_modules_of_two_signatures_lays_each_at_load() {
    let inner = "SIG Inner FOR ALL #[Elem] = #[(VAL item :Elem)]\n\
                 MODULE a = ((LET Elem = Number) (LET item = 5))\n\
                 MODULE b = ((LET Elem = Number) (LET item = 6) (LET more = 1))\n";
    for (slot, value) in [
        ("(LIST OF (Inner WITH {Elem = Carrier}))", "[a b]"),
        (
            "(MAP Str -> (Inner WITH {Elem = Carrier}))",
            "{\"k\": a, \"j\": b}",
        ),
    ] {
        let source = format!(
            "{inner}SIG Outer FOR ALL #[Carrier] = #[(VAL zero :Carrier) (VAL held :{slot})]\n\
             MODULE ints = ((LET zero = 0) (LET held = {value}))\n\
             LET o = (ints :| Outer)\nLET m = (o :| Outer)\n"
        );
        assert_eq!(branch_after(&source), 0, "{slot}");
    }
}

/// An opaque view over a module the load does not type exactly re-views its nested modules at run,
/// but a view of it reads each member at the type it declares, so the later views of the chain
/// add nothing at run.
#[test]
fn a_chain_of_views_over_a_module_typed_at_most_re_views_only_its_first_at_run() {
    let source = format!(
        "{DEEP}MODULE w = ((LET zero = 0) (LET inner :(Inner WITH {{Elem = Number}}) = a))\n\
         LET v = (w :| Outer)\nLET v2 = (v :| Outer)\nLET v3 = (v2 :| Outer)\n"
    );
    let first = format!(
        "{DEEP}MODULE w = ((LET zero = 0) (LET inner :(Inner WITH {{Elem = Number}}) = a))\n\
         LET v = (w :| Outer)\n"
    );
    assert!(branch_after(&source) <= branch_after(&first));
}

/// A chain of views over a parameter is open at every step: the load reads each view once.
#[test]
fn a_chain_of_open_views_reads_each_once() {
    let program = |n: usize| {
        let mut body = String::from("(LET v0 = m)");
        for at in 1..=n {
            body += &format!(" (LET v{at} = (v{} :| Counter))", at - 1);
        }
        format!(
            "SIG Counter FOR ALL #[Carrier] = \
             #[(VAL zero :Carrier) (VAL succ :(FN :{{x :Carrier}} -> Carrier))]\n\
             LET f = (FN :{{m :Counter}} -> Any = #({body} (v{n}.zero)))\n"
        )
    };
    assert_linear("view reads", |n| load_counts(&program(n)).view_reads);
}

/// A `:|` over a closed tree fits its source, and reads the type its source carries, once: for
/// its view's tree, for the types the load lays and for every later read of the view.
#[test]
fn a_view_over_a_closed_tree_fits_once() {
    let counts = load_counts(
        "SIG Counter FOR ALL #[Carrier] = \
         #[(VAL zero :Carrier) (VAL succ :(FN :{x :Carrier} -> Carrier))]\n\
         MODULE ints = ((LET zero = 0) \
         (LET succ = (FN :{x :Number} -> Number = #(x + 1))))\n\
         LET v = (ints :| Counter)\n\
         PRINT v.zero\n",
    );
    assert_eq!((counts.fits, counts.source_reads), (1, 1));
}
