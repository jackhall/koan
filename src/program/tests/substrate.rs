//! A substrate loads — or reports why not — moves, runs across separate calls, and drops.

use crate::program::{CellSubstrate, KBirth, KBundle, LoadError};
use crate::scheduler::{Action, DrainStalled, Placement, Step, StepError, Work};
use crate::scope::ShapeError;

use super::evaluator::Mini;
use super::{loaded, read_back, run_and_read};

/// Two programs, loaded by a helper that hands each substrate back.
fn two() -> (CellSubstrate, CellSubstrate) {
    (loaded("LET a = 1", 2), loaded("LET a = 2\nLET b = a", 2))
}

#[test]
fn two_programs_run_and_are_read_back_in_a_separate_call() {
    let (first, second) = two();
    let mut substrates = vec![first, second];
    substrates.reverse();
    let mut substrates = Box::new(substrates);
    let read: Vec<Vec<String>> = substrates
        .iter_mut()
        .map(|substrate| run_and_read(substrate, &["a"]))
        .collect();
    assert_eq!(read, [["2"], ["1"]]);
    // The view the first read left at rest is left again, so a third call reads it once more.
    assert_eq!(read_back(&mut substrates[0], &["b"]), ["2"]);
    drop(substrates);
}

#[test]
fn load_reports_a_parse_error_and_a_shape_error() {
    assert!(matches!(
        CellSubstrate::load::<Mini>("foo[2]", "<test>", 2),
        Err(LoadError::Parse(_))
    ));
    assert!(matches!(
        CellSubstrate::load::<Mini>("LET a = nowhere", "<test>", 2),
        Err(LoadError::Shape(ShapeError::Unbound { .. }))
    ));
}

#[test]
fn an_inspection_before_the_program_runs_is_unfinished() {
    let mut substrate = loaded("LET a = 1", 2);
    substrate.with(|running| {
        assert_eq!(
            running.inspect(super::inspecting),
            Err(DrainStalled::Unfinished)
        );
    });
}

/// A step that refuses to proceed.
fn fail<'graph>(step: Step<'_, 'graph, '_, '_, '_, KBundle>) -> Action<'graph, KBundle> {
    step.failed(StepError::Stale)
}

/// A stalled drain leaves its cells in the graph, under the root; a later call still runs the
/// program under that same root.
#[test]
fn a_stalled_substrate_still_runs_a_later_root_work() {
    let mut substrate = loaded("LET a = 1", 2);
    substrate.with(|running| {
        let root = running.root();
        let program = running.program();
        let stalled = running.scheduler().run(
            Work {
                step: fail,
                state: KBirth::Program { program },
            },
            root,
            Placement::Fresh,
        );
        assert_eq!(stalled.err(), Some(DrainStalled::Step(StepError::Stale)));
    });
    assert_eq!(run_and_read(&mut substrate, &["a"]), ["1"]);
}
