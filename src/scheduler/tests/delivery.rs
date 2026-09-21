//! A consumer parked on several producers: the count lives in the substrate's receipt run, one
//! producer's delivery completes it, and the consumer wakes exactly once.

use crate::knot::KValue;
use crate::memory::{Active, Receipt};
use crate::scheduler::tests::native::{describe, record, recorded, reset};
use crate::scheduler::{
    Action, Context, Graph, Placement, Request, Resume, Scheduler, Spawns, State, StepError, Work,
};

/// How many producers the consumer parks on. Three, so a delivery that is neither the first nor the
/// last has to answer `Outstanding` too.
const PRODUCERS: usize = 3;

/// The consumer: register a run of three, ask for a producer per slot, and park.
fn park_on_three<'graph>(
    context: &mut Context<'graph, '_, '_, '_>,
    resume: Resume<'graph, '_, '_>,
    spawns: &mut Spawns<'graph>,
) -> Action<'graph> {
    for slot in 0..PRODUCERS - 1 {
        spawns.push(producer(slot));
    }
    // The last push hands back the slot the park waits on; the pushes before it went to the slots
    // ahead of it, in that order.
    let asked = spawns.push(producer(PRODUCERS - 1));
    Action::park(
        context,
        &resume,
        spawns,
        asked,
        drain_the_run,
        State::Empty,
        None,
    )
}

/// One producer, born holding the number it scales. Which slot it fills is its push position.
fn producer<'graph>(slot: usize) -> Request<'graph> {
    Request {
        placement: Placement::Fresh,
        work: Work {
            step: produce,
            state: State::Value(KValue::Number(slot as f64)),
        },
    }
}

/// One producer: fill the slot the drain gave it with the number it was born holding.
fn produce<'graph>(
    context: &mut Context<'graph, '_, '_, '_>,
    resume: Resume<'graph, '_, '_>,
    _: &mut Spawns<'graph>,
) -> Action<'graph> {
    // The number itself, not the value: a value at this producer's brand is not one the consumer's
    // scratch can hold, so what crosses is the word inside it.
    let State::Value(KValue::Number(born)) = resume.state else {
        return Action::failed(StepError::Stale);
    };
    Action::deliver_scratch(context, &resume, move |_, _| {
        let value: KValue<'graph, '_> = KValue::Number(born * 10.0);
        Active::new(value)
    })
}

/// The consumer, woken: read every slot of the run it registered.
fn drain_the_run<'graph>(
    context: &mut Context<'graph, '_, '_, '_>,
    _: Resume<'graph, '_, '_>,
    _: &mut Spawns<'graph>,
) -> Action<'graph> {
    record(format!("woke on {:?}", context.receipt_count()));
    for slot in 0..PRODUCERS {
        match context.receipt(slot) {
            Ok(Receipt::Value(value)) => record(describe(value)),
            _ => return Action::failed(StepError::Unredeemable),
        }
    }
    Action::done()
}

#[test]
fn a_consumer_parked_on_three_producers_wakes_once_when_the_last_slot_fills() {
    reset();
    let mut graph: Graph<'static> = Scheduler::graph(4);
    let mut scheduler = Scheduler::over(&mut graph);
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
