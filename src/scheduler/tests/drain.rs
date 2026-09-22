//! The drain's loop over native steps: a queued cell runs, finishes and is reclaimed.

use crate::knot::KValue;
use crate::scheduler::tests::native::{record, record_cell, recorded, recorded_cells, slab};
use crate::scheduler::{
    Action, DrainStalled, Graph, NativeStep, Placement, Request, Scheduler, ScratchState, State,
    Step, StepError, Work,
};

use std::cell::Cell as Tally;

thread_local! {
    /// What a step records for the test around it. A step is a bare `fn`, so it carries no
    /// closure state; the tally stands in for one, in the test alone.
    static RAN: Tally<u32> = const { Tally::new(0) };
}

fn tally() -> u32 {
    RAN.with(|ran| ran.get())
}

fn reset() {
    RAN.with(|ran| ran.set(0));
    crate::scheduler::tests::native::reset();
}

/// A step that records that it ran, and the cell it ran in, and finishes.
fn finish<'graph>(
    step: Step<'_, 'graph, '_, '_, '_>,
    _: State<'graph, '_>,
    _: Option<ScratchState<'graph, '_, '_>>,
) -> Action<'graph> {
    RAN.with(|ran| ran.set(ran.get() + 1));
    record_cell(step.cell());
    step.done()
}

/// A step that reads the number it was born with, records it, and finishes.
fn finish_with_state<'graph>(
    step: Step<'_, 'graph, '_, '_, '_>,
    state: State<'graph, '_>,
    _: Option<ScratchState<'graph, '_, '_>>,
) -> Action<'graph> {
    let State::Value(KValue::Number(count)) = state else {
        return step.failed(StepError::Stale);
    };
    RAN.with(|ran| ran.set(count as u32));
    step.done()
}

/// A step that refuses to proceed.
fn fail<'graph>(
    step: Step<'_, 'graph, '_, '_, '_>,
    _: State<'graph, '_>,
    _: Option<ScratchState<'graph, '_, '_>>,
) -> Action<'graph> {
    step.failed(StepError::Stale)
}

#[test]
fn one_native_cell_runs_dies_and_leaves_the_graph_empty() {
    reset();
    let mut graph: Graph<'static> = Graph::new(4);
    let mut scheduler = Scheduler::over(&mut graph);
    scheduler.submit(slab(finish, State::Empty), 0);
    scheduler.run().expect("the drain runs to empty");
    assert_eq!(tally(), 1);
    let cells = recorded_cells();
    assert_eq!(cells.len(), 1, "one cell ran");
    assert!(!scheduler.graph().is_live(cells[0]));
    assert!(scheduler.graph().is_empty());
}

#[test]
fn a_birth_state_reaches_the_step_that_runs_the_cell() {
    reset();
    let mut graph: Graph<'static> = Graph::new(4);
    let mut scheduler = Scheduler::over(&mut graph);
    scheduler.submit(
        slab(finish_with_state, State::Value(KValue::Number(7.0))),
        0,
    );
    scheduler.run().expect("the drain runs to empty");
    assert_eq!(tally(), 7);
}

#[test]
fn every_queued_cell_runs_before_the_queue_empties() {
    reset();
    let mut graph: Graph<'static> = Graph::new(4);
    let mut scheduler = Scheduler::over(&mut graph);
    for _ in 0..3 {
        scheduler.submit(slab(finish, State::Empty), 0);
    }
    scheduler.run().expect("the drain runs to empty");
    assert_eq!(tally(), 3);
    assert!(scheduler.graph().is_empty());
}

#[test]
fn a_step_that_cannot_proceed_stalls_the_drain() {
    reset();
    let mut graph: Graph<'static> = Graph::new(4);
    let mut scheduler = Scheduler::over(&mut graph);
    scheduler.submit(slab(fail, State::Empty), 0);
    assert_eq!(scheduler.run(), Err(DrainStalled::Step(StepError::Stale)));
}

#[test]
fn two_schedulers_run_beside_each_other_sharing_nothing() {
    reset();
    let mut first_graph: Graph<'static> = Graph::new(2);
    let mut first = Scheduler::over(&mut first_graph);
    let mut second_graph: Graph<'static> = Graph::new(2);
    let mut second = Scheduler::over(&mut second_graph);

    first.submit(slab(record_state, State::Value(KValue::Number(1.0))), 0);
    second.submit(slab(record_state, State::Value(KValue::Number(2.0))), 0);

    // Each drain runs with the other holding work of its own, and neither touches the other's.
    first.run().expect("the first drain runs to empty");
    assert!(first.graph().is_empty());

    first.submit(slab(record_state, State::Value(KValue::Number(3.0))), 0);
    second.run().expect("the second drain runs to empty");
    assert!(second.graph().is_empty());

    first.run().expect("the first drain runs to empty again");
    assert!(first.graph().is_empty());
    assert_eq!(
        recorded(),
        ["1", "2", "3"],
        "each cell ran in its own graph's turn"
    );
    assert!(!first.graph().is_live(recorded_cells()[0]));
}

/// Record the number this cell was born with, so an interleaved pair of drains is legible as one
/// sequence, and the cell it ran in.
fn record_state<'graph>(
    step: Step<'_, 'graph, '_, '_, '_>,
    state: State<'graph, '_>,
    _: Option<ScratchState<'graph, '_, '_>>,
) -> Action<'graph> {
    let State::Value(KValue::Number(mark)) = state else {
        return step.failed(StepError::Stale);
    };
    record(mark.to_string());
    record_cell(step.cell());
    step.done()
}

/// A hundred units with nothing to wait on, on a slab of one: each gets its cell only when the one
/// before it has gone, so a drain that gave every ready unit its cell up front would be refused.
#[test]
fn independent_units_run_one_at_a_time_in_submission_order() {
    reset();
    let mut graph: Graph<'static> = Graph::new(1);
    let mut scheduler = Scheduler::over(&mut graph);
    for mark in 0..100 {
        scheduler.submit(
            slab(record_state, State::Value(KValue::Number(f64::from(mark)))),
            0,
        );
    }
    scheduler.run().expect("the drain runs to empty");
    let order: Vec<String> = (0..100).map(|mark| mark.to_string()).collect();
    assert_eq!(recorded(), order);
    assert_eq!(scheduler.peak_live_cells(), 1);
}

#[test]
fn two_views_over_one_graph_run_in_turn() {
    reset();
    let mut graph: Graph<'static> = Graph::new(2);
    {
        let mut view = Scheduler::over(&mut graph);
        view.submit(slab(finish, State::Empty), 0);
        view.run().expect("the first view runs to empty");
    }
    let mut view = Scheduler::over(&mut graph);
    view.submit(slab(finish, State::Empty), 0);
    view.run().expect("the second view runs to empty");
    assert_eq!(tally(), 2);
    assert!(view.graph().is_empty());
}

/// Park on two children, the first of which refuses to proceed: the drain stalls on it with the
/// second still queued.
fn park_behind_a_failure<'graph>(
    mut step: Step<'_, 'graph, '_, '_, '_>,
    _: State<'graph, '_>,
    _: Option<ScratchState<'graph, '_, '_>>,
) -> Action<'graph> {
    step.spawn(fresh(fail));
    let asked = step.spawn(fresh(finish));
    step.park(asked, finish, State::Empty, None)
}

/// A child in a region of its own, born holding nothing.
fn fresh(child: NativeStep<'_>) -> Request<'_> {
    Request {
        placement: Placement::Fresh,
        work: Work {
            step: child,
            state: State::Empty,
        },
    }
}

#[test]
fn a_view_dropped_with_a_queued_cell_leaves_the_next_drain_stalled() {
    reset();
    let mut graph: Graph<'static> = Graph::new(2);
    {
        let mut view = Scheduler::over(&mut graph);
        view.submit(slab(park_behind_a_failure, State::Empty), 0);
        assert_eq!(view.run(), Err(DrainStalled::Step(StepError::Stale)));
    }
    let mut view = Scheduler::over(&mut graph);
    assert!(!view.graph().is_empty());
    assert_eq!(view.run(), Err(DrainStalled::CellsLive));
    assert_eq!(
        tally(),
        0,
        "the queued sibling went with the view that dropped it"
    );
}
