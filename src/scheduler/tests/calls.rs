//! A call: a cell asks for one child, parks on its result, and reads it back when the child
//! delivers. Once at each placement, so both spawn doors and both delivery doors are driven.

use crate::function::{KValue, KValueFamily};
use crate::memory::{Active, Delivered, Receipt};
use crate::scheduler::tests::native::{describe, record, recorded, reset};
use crate::scheduler::{
    Action, Context, Continuation, NativeStep, Placement, Request, Resume, Scheduler, Spawns,
    State, StepError,
};

/// Ask for one child at `placement`, park on its single slot, and read it back in [`read_one`].
fn ask<'graph>(
    context: &mut Context<'graph, '_, '_, '_>,
    resume: Resume<'graph, '_>,
    spawns: &mut Spawns<'graph>,
    placement: Placement,
    step: NativeStep<'graph>,
) -> Action<'graph> {
    if context.register_receipts(1).is_err() {
        return Action::Failed(StepError::Undeliverable);
    }
    spawns.push(Request {
        placement,
        step,
        state: State::Empty,
        slot: 0,
    });
    context.store_successor(Continuation::Native {
        step: read_one,
        provenance: resume.provenance,
        state: State::Empty,
    });
    Action::Park
}

fn call_fresh<'graph>(
    context: &mut Context<'graph, '_, '_, '_>,
    resume: Resume<'graph, '_>,
    spawns: &mut Spawns<'graph>,
) -> Action<'graph> {
    ask(context, resume, spawns, Placement::Fresh, place_in_scratch)
}

fn call_shares<'graph>(
    context: &mut Context<'graph, '_, '_, '_>,
    resume: Resume<'graph, '_>,
    spawns: &mut Spawns<'graph>,
) -> Action<'graph> {
    ask(context, resume, spawns, Placement::Shares, place_in_storage)
}

/// A fresh, read-only result: built operand-free in the consumer's scratch habitat.
fn place_in_scratch<'graph>(
    context: &mut Context<'graph, '_, '_, '_>,
    resume: Resume<'graph, '_>,
    _: &mut Spawns<'graph>,
) -> Action<'graph> {
    let Some(destination) = resume.provenance.destination else {
        return Action::Failed(StepError::Undeliverable);
    };
    let delivered = context.deliver_scratch(destination.consumer, destination.slot, |writer, _| {
        // Annotated because `text` leaves its `'graph` free: it is the consumer's graph, and the
        // brand it is written at is the consumer's own scratch.
        let value: KValue<'graph, '_> = crate::values::text(writer, "seven");
        Active::new(value)
    });
    match delivered {
        Ok(Delivered::Complete) => Action::Wakes(destination.consumer),
        Ok(Delivered::Outstanding) => Action::Done,
        Err(_) => Action::Failed(StepError::Undeliverable),
    }
}

/// A result bound for the consumer's storage: built in the consumer's region from the start, kept,
/// and filed as a carrier.
fn place_in_storage<'graph>(
    context: &mut Context<'graph, '_, '_, '_>,
    resume: Resume<'graph, '_>,
    _: &mut Spawns<'graph>,
) -> Action<'graph> {
    let Some(destination) = resume.provenance.destination else {
        return Action::Failed(StepError::Undeliverable);
    };
    let Ok(placed) =
        context.alloc_into::<KValueFamily, KValueFamily>(destination.consumer, &[], |writer, _| {
            let value: KValue<'graph, '_> = crate::values::text(writer, "seven");
            Active::new(value)
        })
    else {
        return Action::Failed(StepError::Stale);
    };
    let carrier = context.keep(placed);
    match context.deliver_carrier(destination.consumer, destination.slot, carrier) {
        Ok(Delivered::Complete) => Action::Wakes(destination.consumer),
        Ok(Delivered::Outstanding) => Action::Done,
        Err(_) => Action::Failed(StepError::Undeliverable),
    }
}

/// The caller, woken: drain slot zero, whichever door filled it.
fn read_one<'graph>(
    context: &mut Context<'graph, '_, '_, '_>,
    _: Resume<'graph, '_>,
    _: &mut Spawns<'graph>,
) -> Action<'graph> {
    match context.receipt(0) {
        Ok(Receipt::Value(value)) => record(describe(value)),
        Ok(Receipt::Carrier(Ok(carrier))) => record(describe(context.read(&carrier).value())),
        _ => return Action::Failed(StepError::Unredeemable),
    }
    Action::Done
}

#[test]
fn a_fresh_call_returns_its_result_through_the_callers_scratch() {
    reset();
    let mut scheduler: Scheduler<'static> = Scheduler::new(4);
    scheduler
        .admit(call_fresh, State::Empty)
        .expect("the slab admits");
    scheduler.run().expect("the drain runs to empty");
    assert_eq!(recorded(), ["seven"]);
    assert!(scheduler.is_empty());
}

#[test]
fn a_shares_call_returns_its_result_through_the_callers_storage() {
    reset();
    let mut scheduler: Scheduler<'static> = Scheduler::new(4);
    scheduler
        .admit(call_shares, State::Empty)
        .expect("the slab admits");
    scheduler.run().expect("the drain runs to empty");
    assert_eq!(recorded(), ["seven"]);
    assert!(scheduler.is_empty());
}

#[test]
fn both_placements_compute_the_same_value() {
    reset();
    let mut fresh: Scheduler<'static> = Scheduler::new(4);
    fresh
        .admit(call_fresh, State::Empty)
        .expect("the slab admits");
    fresh.run().expect("the drain runs to empty");
    let from_fresh = recorded();

    reset();
    let mut shares: Scheduler<'static> = Scheduler::new(4);
    shares
        .admit(call_shares, State::Empty)
        .expect("the slab admits");
    shares.run().expect("the drain runs to empty");

    // The hint moves where the bytes are, never what the program computes.
    assert_eq!(from_fresh, recorded());
}
