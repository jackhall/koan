//! The drain's loop over native steps: a queued cell runs, finishes and is reclaimed.

use crate::function::KValue;
use crate::scheduler::{Action, Context, Resume, Scheduler, Spawns, State, StepError};

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
    _: Resume<'graph, '_>,
    _: &mut Spawns<'graph>,
) -> Action<'graph> {
    RAN.with(|ran| ran.set(ran.get() + 1));
    Action::Done
}

/// A step that reads the number it was born with, records it, and finishes.
fn finish_with_state<'graph>(
    _: &mut Context<'graph, '_, '_, '_>,
    resume: Resume<'graph, '_>,
    _: &mut Spawns<'graph>,
) -> Action<'graph> {
    let State::Value(KValue::Number(count)) = resume.state else {
        return Action::Failed(StepError::Stale);
    };
    RAN.with(|ran| ran.set(count as u32));
    Action::Done
}

/// A step that refuses to proceed.
fn fail<'graph>(
    _: &mut Context<'graph, '_, '_, '_>,
    _: Resume<'graph, '_>,
    _: &mut Spawns<'graph>,
) -> Action<'graph> {
    Action::Failed(StepError::Stale)
}

#[test]
fn one_native_cell_runs_dies_and_leaves_the_graph_empty() {
    reset();
    let mut scheduler: Scheduler<'static> = Scheduler::new(4);
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
    let mut scheduler: Scheduler<'static> = Scheduler::new(4);
    scheduler
        .admit(finish_with_state, State::Value(KValue::Number(7.0)))
        .expect("the slab admits");
    scheduler.run().expect("the drain runs to empty");
    assert_eq!(tally(), 7);
}

#[test]
fn every_queued_cell_runs_before_the_queue_empties() {
    reset();
    let mut scheduler: Scheduler<'static> = Scheduler::new(4);
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
    let mut scheduler: Scheduler<'static> = Scheduler::new(4);
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
    reset();
    let mut first: Scheduler<'static> = Scheduler::new(2);
    let mut second: Scheduler<'static> = Scheduler::new(2);
    let a = first.admit(finish, State::Empty).expect("the slab admits");
    let b = second.admit(finish, State::Empty).expect("the slab admits");
    // Neither graph names the other's cell, and each reclaims only its own.
    first.run().expect("the first drain runs to empty");
    assert!(first.is_empty());
    assert!(second.is_live(b));
    second.run().expect("the second drain runs to empty");
    assert!(second.is_empty());
    assert!(!first.is_live(a));
    assert_eq!(tally(), 2);
}
