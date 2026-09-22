//! A call subtree deeper than any slab cap. The tree pool takes no cap, so depth costs tree cells
//! and nothing else — which is why the matrix is one word wide.

use crate::knot::KValue;
use crate::memory::{Active, Receipt};
use crate::scheduler::tests::native::{record, recorded, reset, slab};
use crate::scheduler::{
    Action, Graph, Placement, Request, Scheduler, ScratchState, State, Step, StepError, Work,
};

/// Deeper than the sixty-four slots a one-word matrix has, several times over.
const DEPTH: f64 = 200.0;

/// Spawn the head of the descent and park on its single slot.
fn start<'graph>(
    mut step: Step<'_, 'graph, '_, '_, '_>,
    _: State<'graph, '_>,
    _: Option<ScratchState<'graph, '_, '_>>,
) -> Action<'graph> {
    let asked = step.spawn(Request {
        placement: Placement::Fresh,
        work: Work {
            step: descend,
            state: State::Value(KValue::Number(DEPTH)),
        },
    });
    step.park(asked, finish, State::Empty, None)
}

/// One level: park on a child one shallower, or turn around at the bottom.
fn descend<'graph>(
    mut step: Step<'_, 'graph, '_, '_, '_>,
    state: State<'graph, '_>,
    _: Option<ScratchState<'graph, '_, '_>>,
) -> Action<'graph> {
    let State::Value(KValue::Number(depth)) = state else {
        return step.failed(StepError::Stale);
    };
    if depth == 0.0 {
        return fill(step, 0.0);
    }
    let asked = step.spawn(Request {
        placement: Placement::Fresh,
        work: Work {
            step: descend,
            state: State::Value(KValue::Number(depth - 1.0)),
        },
    });
    step.park(asked, ascend, State::Empty, None)
}

/// Woken by the level below: add this level to its count and pass it up.
fn ascend<'graph>(
    step: Step<'_, 'graph, '_, '_, '_>,
    _: State<'graph, '_>,
    _: Option<ScratchState<'graph, '_, '_>>,
) -> Action<'graph> {
    let Ok(Receipt::Value(KValue::Number(below))) = step.receipt(0) else {
        return step.failed(StepError::Unredeemable);
    };
    fill(step, below + 1.0)
}

/// Put one number in the slot this cell was born against.
fn fill<'graph>(step: Step<'_, 'graph, '_, '_, '_>, count: f64) -> Action<'graph> {
    step.deliver_scratch(move |_, _| Active::new(KValue::Number(count)))
}

/// The root, woken by the head of the descent.
fn finish<'graph>(
    step: Step<'_, 'graph, '_, '_, '_>,
    _: State<'graph, '_>,
    _: Option<ScratchState<'graph, '_, '_>>,
) -> Action<'graph> {
    match step.receipt(0) {
        Ok(Receipt::Value(KValue::Number(count))) => record(count.to_string()),
        _ => return step.failed(StepError::Unredeemable),
    }
    step.done()
}

#[test]
fn a_subtree_two_hundred_deep_takes_no_slab_slot_but_its_root() {
    reset();
    // A slab of one. Every level of the descent is a tree child, so a level that reached for a
    // slab slot would be refused and the drain would stall before the bottom.
    let mut graph: Graph<'static> = Graph::new(1);
    let mut scheduler = Scheduler::over(&mut graph);
    scheduler.submit(slab(start, State::Empty), 0);
    scheduler.run().expect("the drain runs to empty");

    assert_eq!(recorded(), ["200"], "every level answered the one above it");
    assert!(scheduler.graph().is_empty());
}
