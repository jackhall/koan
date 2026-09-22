//! A consumer parked on several producers: the count lives in the substrate's receipt run, one
//! producer's delivery completes it, and the consumer wakes exactly once.

use crate::knot::KValue;
use crate::memory::{Active, Receipt};
use crate::scheduler::tests::native::{describe, record, recorded, reset, slab};
use crate::scheduler::{
    Action, Graph, Placement, Request, Scheduler, ScratchState, State, Step, StepError, Work,
};

/// How many producers the consumer parks on. Three, so a delivery that is neither the first nor the
/// last has to answer `Outstanding` too.
const PRODUCERS: usize = 3;

/// The consumer: ask for a producer per slot, and park on the run they fill.
fn park_on_three<'graph>(
    mut step: Step<'_, 'graph, '_, '_, '_>,
    _: State<'graph, '_>,
    _: Option<ScratchState<'graph, '_, '_>>,
) -> Action<'graph> {
    for slot in 0..PRODUCERS - 1 {
        step.spawn(producer(slot));
    }
    // The last spawn hands back the slot the park waits on; the spawns before it went to the slots
    // ahead of it, in that order.
    let asked = step.spawn(producer(PRODUCERS - 1));
    step.park(asked, drain_the_run, State::Empty, None)
}

/// One producer, born holding the number it scales. Which slot it fills is its spawn position.
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
    step: Step<'_, 'graph, '_, '_, '_>,
    state: State<'graph, '_>,
    _: Option<ScratchState<'graph, '_, '_>>,
) -> Action<'graph> {
    // The number itself, not the value: a value at this producer's brand is not one the consumer's
    // scratch can hold, so what crosses is the word inside it.
    let State::Value(KValue::Number(born)) = state else {
        return step.failed(StepError::Stale);
    };
    step.deliver_scratch(move |_, _| {
        let value: KValue<'graph, '_> = KValue::Number(born * 10.0);
        Active::new(value)
    })
}

/// The consumer, woken: read every slot of the run it registered.
fn drain_the_run<'graph>(
    step: Step<'_, 'graph, '_, '_, '_>,
    _: State<'graph, '_>,
    _: Option<ScratchState<'graph, '_, '_>>,
) -> Action<'graph> {
    record(format!("woke on {:?}", step.receipt_count()));
    for slot in 0..PRODUCERS {
        match step.receipt(slot) {
            Ok(Receipt::Value(value)) => record(describe(value)),
            _ => return step.failed(StepError::Unredeemable),
        }
    }
    step.done()
}

#[test]
fn a_consumer_parked_on_three_producers_wakes_once_when_the_last_slot_fills() {
    reset();
    let mut graph: Graph<'static> = Graph::new(4);
    let mut scheduler = Scheduler::over(&mut graph);
    scheduler.submit(slab(park_on_three, State::Empty), 0);
    scheduler.run().expect("the drain runs to empty");

    // One wake line, then one line per slot: the consumer ran again exactly once, and only after
    // every producer had filled its slot.
    assert_eq!(
        recorded(),
        ["woke on Some(3)", "0", "10", "20"],
        "the consumer wakes once, with every slot already filled"
    );
    assert!(scheduler.graph().is_empty());
}
