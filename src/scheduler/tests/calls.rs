//! A call: a cell asks for one child, parks on its result, and reads it back when the child
//! delivers. Once at each placement, so both spawn doors and both delivery doors are driven.

use crate::knot::{KValue, KValueFamily};
use crate::memory::{Active, Receipt};
use crate::scheduler::tests::native::{describe, record, recorded, reset, slab};
use crate::scheduler::{
    Action, Graph, NativeStep, Placement, Request, Scheduler, ScratchState, State, Step, StepError,
    Work,
};

/// Ask for one child at `placement`, park on its single slot, and read it back in [`read_one`].
fn ask<'graph>(
    mut step: Step<'_, 'graph, '_, '_, '_>,
    placement: Placement,
    child: NativeStep<'graph>,
) -> Action<'graph> {
    let asked = step.spawn(Request {
        placement,
        work: Work {
            step: child,
            state: State::Empty,
        },
    });
    step.park(asked, read_one, State::Empty, None)
}

fn call_fresh<'graph>(
    step: Step<'_, 'graph, '_, '_, '_>,
    _: State<'graph, '_>,
    _: Option<ScratchState<'graph, '_, '_>>,
) -> Action<'graph> {
    ask(step, Placement::Fresh, place_in_scratch)
}

fn call_shares<'graph>(
    step: Step<'_, 'graph, '_, '_, '_>,
    _: State<'graph, '_>,
    _: Option<ScratchState<'graph, '_, '_>>,
) -> Action<'graph> {
    ask(step, Placement::Shares, place_in_storage)
}

/// A fresh, read-only result: built operand-free in the consumer's scratch habitat.
fn place_in_scratch<'graph>(
    step: Step<'_, 'graph, '_, '_, '_>,
    _: State<'graph, '_>,
    _: Option<ScratchState<'graph, '_, '_>>,
) -> Action<'graph> {
    step.deliver_scratch(|writer, _| {
        // Annotated because `text` leaves its `'graph` free: it is the consumer's graph, and the
        // brand it is written at is the consumer's own scratch.
        let value: KValue<'graph, '_> = crate::values::text(writer, "seven");
        Active::new(value)
    })
}

/// A result bound for the consumer's storage: built in the consumer's region from the start, kept,
/// and filed as a carrier.
fn place_in_storage<'graph>(
    mut step: Step<'_, 'graph, '_, '_, '_>,
    _: State<'graph, '_>,
    _: Option<ScratchState<'graph, '_, '_>>,
) -> Action<'graph> {
    // The build is the consumer's own region, so this step still names it; which slot the carrier
    // fills is the drain's, and `deliver_carrier` reads it off the provenance.
    let Some(consumer) = step.consumer() else {
        return step.failed(StepError::Undeliverable);
    };
    let Ok(placed) = step.alloc_into::<KValueFamily, KValueFamily>(consumer, &[], |writer, _| {
        let value: KValue<'graph, '_> = crate::values::text(writer, "seven");
        Active::new(value)
    }) else {
        return step.failed(StepError::Stale);
    };
    let carrier = step.keep(placed);
    step.deliver_carrier(carrier)
}

/// The caller, woken: drain slot zero, whichever door filled it.
fn read_one<'graph>(
    step: Step<'_, 'graph, '_, '_, '_>,
    _: State<'graph, '_>,
    _: Option<ScratchState<'graph, '_, '_>>,
) -> Action<'graph> {
    match step.receipt(0) {
        Ok(Receipt::Value(value)) => record(describe(value)),
        Ok(Receipt::Carrier(Ok(carrier))) => record(describe(step.read(&carrier).value())),
        _ => return step.failed(StepError::Unredeemable),
    }
    step.done()
}

#[test]
fn a_fresh_call_returns_its_result_through_the_callers_scratch() {
    reset();
    let mut graph: Graph<'static> = Graph::new(4);
    let mut scheduler = Scheduler::over(&mut graph);
    scheduler.submit(slab(call_fresh, State::Empty), 0);
    scheduler.run().expect("the drain runs to empty");
    assert_eq!(recorded(), ["seven"]);
    assert!(scheduler.graph().is_empty());
}

#[test]
fn a_shares_call_returns_its_result_through_the_callers_storage() {
    reset();
    let mut graph: Graph<'static> = Graph::new(4);
    let mut scheduler = Scheduler::over(&mut graph);
    scheduler.submit(slab(call_shares, State::Empty), 0);
    scheduler.run().expect("the drain runs to empty");
    assert_eq!(recorded(), ["seven"]);
    assert!(scheduler.graph().is_empty());
}

#[test]
fn both_placements_compute_the_same_value() {
    reset();
    let mut fresh_graph: Graph<'static> = Graph::new(4);
    let mut fresh = Scheduler::over(&mut fresh_graph);
    fresh.submit(slab(call_fresh, State::Empty), 0);
    fresh.run().expect("the drain runs to empty");
    let from_fresh = recorded();

    reset();
    let mut shares_graph: Graph<'static> = Graph::new(4);
    let mut shares = Scheduler::over(&mut shares_graph);
    shares.submit(slab(call_shares, State::Empty), 0);
    shares.run().expect("the drain runs to empty");

    // The hint moves where the bytes are, never what the program computes.
    assert_eq!(from_fresh, recorded());
}
