//! The drain's loop over native steps: a queued cell runs, finishes and is reclaimed.

use crate::knot::KValue;
use crate::scheduler::tests::native::{record, recorded};
use crate::scheduler::{Action, Context, Graph, Resume, Scheduler, Spawns, State, StepError};

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
}

/// A step that records that it ran and finishes.
fn finish<'graph>(
    _: &mut Context<'graph, '_, '_, '_>,
    _: Resume<'graph, '_, '_>,
    _: &mut Spawns<'graph>,
) -> Action<'graph> {
    RAN.with(|ran| ran.set(ran.get() + 1));
    Action::done()
}

/// A step that reads the number it was born with, records it, and finishes.
fn finish_with_state<'graph>(
    _: &mut Context<'graph, '_, '_, '_>,
    resume: Resume<'graph, '_, '_>,
    _: &mut Spawns<'graph>,
) -> Action<'graph> {
    let State::Value(KValue::Number(count)) = resume.state else {
        return Action::failed(StepError::Stale);
    };
    RAN.with(|ran| ran.set(count as u32));
    Action::done()
}

/// A step that refuses to proceed.
fn fail<'graph>(
    _: &mut Context<'graph, '_, '_, '_>,
    _: Resume<'graph, '_, '_>,
    _: &mut Spawns<'graph>,
) -> Action<'graph> {
    Action::failed(StepError::Stale)
}

#[test]
fn one_native_cell_runs_dies_and_leaves_the_graph_empty() {
    reset();
    let mut graph: Graph<'static> = Scheduler::graph(4);
    let mut scheduler = Scheduler::over(&mut graph);
    let cell = scheduler
        .admit(finish, State::Empty)
        .expect("the slab admits");
    assert!(scheduler.is_live(cell));
    scheduler.run().expect("the drain runs to empty");
    assert_eq!(tally(), 1);
    assert!(!scheduler.is_live(cell));
    assert!(scheduler.is_empty());
}

#[test]
fn a_birth_state_reaches_the_step_that_runs_the_cell() {
    reset();
    let mut graph: Graph<'static> = Scheduler::graph(4);
    let mut scheduler = Scheduler::over(&mut graph);
    scheduler
        .admit(finish_with_state, State::Value(KValue::Number(7.0)))
        .expect("the slab admits");
    scheduler.run().expect("the drain runs to empty");
    assert_eq!(tally(), 7);
}

#[test]
fn every_queued_cell_runs_before_the_queue_empties() {
    reset();
    let mut graph: Graph<'static> = Scheduler::graph(4);
    let mut scheduler = Scheduler::over(&mut graph);
    for _ in 0..3 {
        scheduler
            .admit(finish, State::Empty)
            .expect("the slab admits");
    }
    scheduler.run().expect("the drain runs to empty");
    assert_eq!(tally(), 3);
    assert!(scheduler.is_empty());
}

#[test]
fn a_step_that_cannot_proceed_stalls_the_drain() {
    reset();
    let mut graph: Graph<'static> = Scheduler::graph(4);
    let mut scheduler = Scheduler::over(&mut graph);
    scheduler
        .admit(fail, State::Empty)
        .expect("the slab admits");
    assert_eq!(
        scheduler.run(),
        Err(crate::scheduler::DrainStalled::Step(StepError::Stale))
    );
}

#[test]
fn two_schedulers_run_beside_each_other_sharing_nothing() {
    crate::scheduler::tests::native::reset();
    let mut first_graph: Graph<'static> = Scheduler::graph(2);
    let mut first = Scheduler::over(&mut first_graph);
    let mut second_graph: Graph<'static> = Scheduler::graph(2);
    let mut second = Scheduler::over(&mut second_graph);

    let a = first
        .admit(record_state, State::Value(KValue::Number(1.0)))
        .expect("the slab admits");
    let b = second
        .admit(record_state, State::Value(KValue::Number(2.0)))
        .expect("the slab admits");

    // Each drain runs with the other holding a live cell, and neither graph names the other's.
    first.run().expect("the first drain runs to empty");
    assert!(first.is_empty());
    assert!(second.is_live(b));

    let c = first
        .admit(record_state, State::Value(KValue::Number(3.0)))
        .expect("the slab admits, the slot the first cell had being free again");
    second.run().expect("the second drain runs to empty");
    assert!(second.is_empty());
    assert!(first.is_live(c));

    first.run().expect("the first drain runs to empty again");
    assert!(first.is_empty());
    assert!(!first.is_live(a));
    assert_eq!(
        recorded(),
        ["1", "2", "3"],
        "each cell ran in its own graph's turn"
    );
}

/// Record the number this cell was born with, so an interleaved pair of drains is legible as one
/// sequence.
fn record_state<'graph>(
    _: &mut Context<'graph, '_, '_, '_>,
    resume: Resume<'graph, '_, '_>,
    _: &mut Spawns<'graph>,
) -> Action<'graph> {
    let State::Value(KValue::Number(mark)) = resume.state else {
        return Action::failed(StepError::Stale);
    };
    record(mark.to_string());
    Action::done()
}

#[test]
fn two_views_over_one_graph_run_in_turn() {
    reset();
    let mut graph: Graph<'static> = Scheduler::graph(2);
    {
        let mut view = Scheduler::over(&mut graph);
        view.admit(finish, State::Empty).expect("the slab admits");
        view.run().expect("the first view runs to empty");
    }
    let mut view = Scheduler::over(&mut graph);
    view.admit(finish, State::Empty).expect("the slab admits");
    view.run().expect("the second view runs to empty");
    assert_eq!(tally(), 2);
    assert!(view.is_empty());
}

#[test]
fn a_view_dropped_with_a_queued_cell_leaves_the_next_drain_stalled() {
    reset();
    let mut graph: Graph<'static> = Scheduler::graph(2);
    let cell = {
        let mut view = Scheduler::over(&mut graph);
        view.admit(finish, State::Empty).expect("the slab admits")
    };
    let mut view = Scheduler::over(&mut graph);
    assert!(view.is_live(cell));
    assert_eq!(view.run(), Err(crate::scheduler::DrainStalled::CellsLive));
    assert_eq!(tally(), 0);
}
