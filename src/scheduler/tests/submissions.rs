//! The submission table: a unit with unmet dependencies has no cell at all, and gets one the
//! moment the last of them finishes. A diamond is the shape that tells the two failures apart — a
//! join that runs early, and a join that runs twice.

use crate::memory::{Active, Delivered};
use crate::scheduler::tests::native::{record, recorded, reset};
use crate::scheduler::{
    Action, CellPlace, Context, Continuation, DrainStalled, Placement, Request, Resume, Scheduler,
    Spawns, State, StepError, Unit,
};

/// A unit of the diamond, born in the slab: what the table decides is *when* it runs, never where.
fn unit(step: crate::scheduler::NativeStep<'static>) -> Unit<'static> {
    Unit {
        place: CellPlace::Slab,
        placement: Placement::Fresh,
        step,
        state: State::Empty,
    }
}

fn source<'graph>(
    _: &mut Context<'graph, '_, '_, '_>,
    _: Resume<'graph, '_>,
    _: &mut Spawns<'graph>,
) -> Action<'graph> {
    record(String::from("source"));
    Action::Done
}

fn left<'graph>(
    _: &mut Context<'graph, '_, '_, '_>,
    _: Resume<'graph, '_>,
    _: &mut Spawns<'graph>,
) -> Action<'graph> {
    record(String::from("left"));
    Action::Done
}

fn right<'graph>(
    _: &mut Context<'graph, '_, '_, '_>,
    _: Resume<'graph, '_>,
    _: &mut Spawns<'graph>,
) -> Action<'graph> {
    record(String::from("right"));
    Action::Done
}

fn join<'graph>(
    _: &mut Context<'graph, '_, '_, '_>,
    _: Resume<'graph, '_>,
    _: &mut Spawns<'graph>,
) -> Action<'graph> {
    record(String::from("join"));
    Action::Done
}

#[test]
fn a_join_runs_once_and_only_after_both_arms() {
    reset();
    let mut scheduler: Scheduler<'static> = Scheduler::new(4);
    let source = scheduler.submit(unit(source), 0);
    let left = scheduler.submit(unit(left), 1);
    let right = scheduler.submit(unit(right), 1);
    let join = scheduler.submit(unit(join), 2);
    scheduler.edge(source, left);
    scheduler.edge(source, right);
    scheduler.edge(left, join);
    scheduler.edge(right, join);

    scheduler.run().expect("the drain runs to empty");

    let ran = recorded();
    assert_eq!(
        ran.len(),
        4,
        "every unit runs, and each of them once: {ran:?}"
    );
    assert_eq!(ran[0], "source", "the source has no dependency to wait on");
    assert_eq!(
        ran[3], "join",
        "the join waits for the arm that finishes last"
    );
    assert!(ran[1..3].contains(&String::from("left")));
    assert!(ran[1..3].contains(&String::from("right")));
    assert!(scheduler.is_empty(), "the table and the graph both empty");
}

/// Four units, two of them waiting on each other. Neither ever reaches zero, so neither ever gets
/// a cell — and the queue runs dry with the graph already empty.
#[test]
fn units_that_wait_on_each_other_stall_the_drain() {
    reset();
    let mut scheduler: Scheduler<'static> = Scheduler::new(4);
    let first = scheduler.submit(unit(left), 1);
    let second = scheduler.submit(unit(right), 1);
    scheduler.edge(first, second);
    scheduler.edge(second, first);

    assert_eq!(scheduler.run(), Err(DrainStalled::UnitsPending));
    assert_eq!(recorded(), Vec::<String>::new(), "neither unit was born");
}

/// A submitted unit that parks on a child of its own: what satisfies its dependents is the cell
/// finishing, not its first step returning.
fn spawns_and_parks<'graph>(
    context: &mut Context<'graph, '_, '_, '_>,
    resume: Resume<'graph, '_>,
    spawns: &mut Spawns<'graph>,
) -> Action<'graph> {
    record(String::from("parks"));
    if context.register_receipts(1).is_err() {
        return Action::Failed(StepError::Undeliverable);
    }
    spawns.push(Request {
        placement: Placement::Fresh,
        step: answers,
        state: State::Empty,
        slot: 0,
    });
    context.store_successor(Continuation::Native {
        step: wakes,
        provenance: resume.provenance,
        state: State::Empty,
    });
    Action::Park
}

fn answers<'graph>(
    context: &mut Context<'graph, '_, '_, '_>,
    resume: Resume<'graph, '_>,
    _: &mut Spawns<'graph>,
) -> Action<'graph> {
    record(String::from("child"));
    let Some(destination) = resume.provenance.destination else {
        return Action::Failed(StepError::Undeliverable);
    };
    let delivered = context.deliver_scratch(destination.consumer, destination.slot, |_, _| {
        Active::new(crate::function::KValue::Number(1.0))
    });
    match delivered {
        Ok(Delivered::Complete) => Action::Wakes(destination.consumer),
        Ok(Delivered::Outstanding) => Action::Done,
        Err(_) => Action::Failed(StepError::Undeliverable),
    }
}

fn wakes<'graph>(
    _: &mut Context<'graph, '_, '_, '_>,
    _: Resume<'graph, '_>,
    _: &mut Spawns<'graph>,
) -> Action<'graph> {
    record(String::from("wakes"));
    Action::Done
}

#[test]
fn a_dependent_waits_for_its_producers_whole_subtree() {
    reset();
    let mut scheduler: Scheduler<'static> = Scheduler::new(4);
    let producer = scheduler.submit(unit(spawns_and_parks), 0);
    let dependent = scheduler.submit(unit(join), 1);
    scheduler.edge(producer, dependent);

    scheduler.run().expect("the drain runs to empty");

    assert_eq!(recorded(), ["parks", "child", "wakes", "join"]);
    assert!(scheduler.is_empty());
}
