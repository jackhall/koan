//! Views the load sees all of: at a `:|` site over a closed content tree the root holds every type
//! the view door computes, so the run builds the view without interning a type of its own.

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
