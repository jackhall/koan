//! Views the load sees all of: at a `:|` site over a closed content tree the root holds every type
//! the view door computes, so the run builds the view without interning a type of its own — a
//! nested module's view among them.

use std::cell::Cell;

use crate::program::{CellSubstrate, KBirth, KBundle, KState, Outcome};
use crate::scheduler::{Action, Step, StepError};
use crate::type_lattice::Branch;

use super::super::Koan;
use super::output;

thread_local! {
    /// What [`branch_length`] read.
    static BRANCH: Cell<usize> = const { Cell::new(0) };
}

/// A root work over the top level's resting view: it records the root cell's branch length.
fn branch_length<'graph>(step: Step<'_, 'graph, '_, '_, '_, KBundle>) -> Action<'graph, KBundle> {
    let (step, state) = step.state();
    let KState::Born(birth @ KBirth::Inspect { .. }) = state else {
        return step.failed(StepError::Refused);
    };
    BRANCH.with(|branch| branch.set(step.header().get().map_or(0, Branch::len)));
    step.leave(birth)
}

/// The root cell's branch length once `source` has run.
fn branch_after(source: &str) -> usize {
    let mut substrate = CellSubstrate::load::<Koan>(source, "<test>", 2, output())
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

/// `Inner`, an `Outer` holding a list of its applications, and modules of two signatures, for a view over a list of nested modules.
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

/// A view of a transparent view: the source's signature declares the nested module at an
/// application fixing each parameter, which the load re-views as the run does the module.
#[test]
fn a_view_of_a_transparent_view_lays_its_nested_view_at_load() {
    let source = "SIG Inner FOR ALL #[Elem] = #[(VAL item :Elem)]\n\
                  SIG Outer FOR ALL #[Carrier] = \
                  #[(VAL zero :Carrier) (VAL inner :(Inner WITH {Elem = Carrier}))]\n\
                  MODULE a = ((LET Elem = Number) (LET item = 5))\n\
                  MODULE ints = ((LET zero = 0) (LET inner = a))\n\
                  LET t = (ints :! (Outer WITH {Carrier = Number}))\n\
                  LET m = (t :| Outer)\n";
    assert_eq!(branch_after(source), 0);
}
