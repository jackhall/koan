//! A consumer parked on several producers: the count lives in the substrate's receipt run, one
//! producer's delivery completes it, and the consumer wakes exactly once.

use crate::function::KValue;
use crate::memory::{Active, Delivered, Receipt};
use crate::scheduler::tests::native::{describe, record, recorded, reset};
use crate::scheduler::{
    Action, Context, Continuation, Placement, Request, Resume, Scheduler, Spawns, State, StepError,
    Work,
};

/// How many producers the consumer parks on. Three, so a delivery that is neither the first nor the
/// last has to answer `Outstanding` too.
const PRODUCERS: usize = 3;

/// The consumer: register a run of three, ask for a producer per slot, and park.
fn park_on_three<'graph>(
    context: &mut Context<'graph, '_, '_, '_>,
    resume: Resume<'graph, '_>,
    spawns: &mut Spawns<'graph>,
) -> Action<'graph> {
    if context.register_receipts(PRODUCERS).is_err() {
        return Action::Failed(StepError::Undeliverable);
    }
    for slot in 0..PRODUCERS {
        spawns.push(Request {
            placement: Placement::Fresh,
            work: Work {
                step: produce,
                state: State::Value(KValue::Number(slot as f64)),
            },
            slot,
        });
    }
    context.store_successor(Continuation::Native {
        step: drain_the_run,
        provenance: resume.provenance,
        state: State::Empty,
    });
    Action::Park
}

/// One producer: fill the slot the drain gave it with the number it was born holding.
fn produce<'graph>(
    context: &mut Context<'graph, '_, '_, '_>,
    resume: Resume<'graph, '_>,
    _: &mut Spawns<'graph>,
) -> Action<'graph> {
    let Some(destination) = resume.provenance.destination else {
        return Action::Failed(StepError::Undeliverable);
    };
    // The number itself, not the value: a value at this producer's brand is not one the consumer's
    // scratch can hold, so what crosses is the word inside it.
    let State::Value(KValue::Number(born)) = resume.state else {
        return Action::Failed(StepError::Stale);
    };
    let delivered = context.deliver_scratch(destination.consumer, destination.slot, move |_, _| {
        let value: KValue<'graph, '_> = KValue::Number(born * 10.0);
        Active::new(value)
    });
    match delivered {
        Ok(Delivered::Complete) => Action::Wakes(destination.consumer),
        Ok(Delivered::Outstanding) => Action::Done,
        Err(_) => Action::Failed(StepError::Undeliverable),
    }
}

/// The consumer, woken: read every slot of the run it registered.
fn drain_the_run<'graph>(
    context: &mut Context<'graph, '_, '_, '_>,
    _: Resume<'graph, '_>,
    _: &mut Spawns<'graph>,
) -> Action<'graph> {
    record(format!("woke on {:?}", context.receipt_count()));
    for slot in 0..PRODUCERS {
        match context.receipt(slot) {
            Ok(Receipt::Value(value)) => record(describe(value)),
            _ => return Action::Failed(StepError::Unredeemable),
        }
    }
    Action::Done
}

#[test]
fn a_consumer_parked_on_three_producers_wakes_once_when_the_last_slot_fills() {
    reset();
    let mut scheduler: Scheduler<'static> = Scheduler::new(4);
    scheduler
        .admit(park_on_three, State::Empty)
        .expect("the slab admits");
    scheduler.run().expect("the drain runs to empty");

    // One wake line, then one line per slot: the consumer ran again exactly once, and only after
    // every producer had filled its slot.
    assert_eq!(
        recorded(),
        ["woke on Some(3)", "0", "10", "20"],
        "the consumer wakes once, with every slot already filled"
    );
    assert!(scheduler.is_empty());
}
