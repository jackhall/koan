//! A call subtree deeper than any slab cap. The tree pool takes no cap, so depth costs tree cells
//! and nothing else — which is why the matrix is one word wide.

use crate::knot::KValue;
use crate::memory::Active;
use crate::scheduler::tests::bundle::{Native, TestGraph};
use crate::scheduler::tests::native::{fresh, record, recorded, reset, work};
use crate::scheduler::{Action, Placement, Received, Scheduler, Step, StepError, Use};

/// Deeper than the sixty-four slots a one-word matrix has, several times over.
const DEPTH: f64 = 200.0;

/// Ask for the head of the descent and park on its single slot.
fn start<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let asked = step.spawn(fresh(descend, Use::Reads, KValue::Number(DEPTH)));
    step.park(asked, finish, KValue::Null, None)
}

/// One level: park on a child one shallower, or turn around at the bottom.
fn descend<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let KValue::Number(depth) = step.state() else {
        return step.failed(StepError::Stale);
    };
    if depth == 0.0 {
        return fill(step, 0.0);
    }
    let asked = step.spawn(fresh(descend, Use::Reads, KValue::Number(depth - 1.0)));
    step.park(asked, ascend, KValue::Null, None)
}

/// The count the level below delivered.
fn below(step: &mut Step<'_, '_, '_, '_, '_, Native>) -> Option<f64> {
    match step.results().next() {
        Some(Ok(Received::Scratch(KValue::Number(count)))) => Some(count),
        _ => None,
    }
}

/// Woken by the level below: add this level to its count and pass it up.
fn ascend<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let Some(count) = below(&mut step) else {
        return step.failed(StepError::Unredeemable);
    };
    fill(step, count + 1.0)
}

/// Put one number in the slot this cell was born against.
fn fill<'graph>(step: Step<'_, 'graph, '_, '_, '_, Native>, count: f64) -> Action<'graph, Native> {
    step.finish_fresh(move |_, _| Active::new(KValue::Number(count)))
}

/// The root work, woken by the head of the descent.
fn finish<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let Some(count) = below(&mut step) else {
        return step.failed(StepError::Unredeemable);
    };
    record(count.to_string());
    step.done()
}

#[test]
fn a_subtree_two_hundred_deep_takes_no_slab_slot_but_its_root() {
    reset();
    // A slab of one, and its one slot is the root. Every level of the descent is a tree cell, so a
    // level that reached for a slab slot would be refused and the drain would stall before the
    // bottom.
    let mut graph: TestGraph<'static> = TestGraph::new(1);
    let root = graph.root().expect("a fresh slab admits a root");
    Scheduler::over(&mut graph)
        .run(work(start, KValue::Null), root, Placement::Fresh)
        .expect("the root work ends");

    assert_eq!(recorded(), ["200"], "every level answered the one above it");
    graph.release_root(root).expect("the root releases");
    assert!(graph.is_empty());
}
