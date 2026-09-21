//! A call subtree deeper than any slab cap. The tree pool takes no cap, so depth costs tree cells
//! and nothing else — which is why the matrix is one word wide.

use crate::knot::KValue;
use crate::memory::{Active, Receipt};
use crate::scheduler::tests::native::{record, recorded, reset};
use crate::scheduler::{
    Action, Context, Graph, Placement, Request, Resume, Scheduler, Spawns, State, StepError, Work,
};

/// Deeper than the sixty-four slots a one-word matrix has, several times over.
const DEPTH: f64 = 200.0;

/// Spawn the head of the descent and park on its single slot.
fn start<'graph>(
    context: &mut Context<'graph, '_, '_, '_>,
    resume: Resume<'graph, '_, '_>,
    spawns: &mut Spawns<'graph>,
) -> Action<'graph> {
    let asked = spawns.push(Request {
        placement: Placement::Fresh,
        work: Work {
            step: descend,
            state: State::Value(KValue::Number(DEPTH)),
        },
    });
    Action::park(context, &resume, spawns, asked, finish, State::Empty, None)
}

/// One level: park on a child one shallower, or turn around at the bottom.
fn descend<'graph>(
    context: &mut Context<'graph, '_, '_, '_>,
    resume: Resume<'graph, '_, '_>,
    spawns: &mut Spawns<'graph>,
) -> Action<'graph> {
    let State::Value(KValue::Number(depth)) = resume.state else {
        return Action::failed(StepError::Stale);
    };
    if depth == 0.0 {
        return fill(context, resume, 0.0);
    }
    let asked = spawns.push(Request {
        placement: Placement::Fresh,
        work: Work {
            step: descend,
            state: State::Value(KValue::Number(depth - 1.0)),
        },
    });
    Action::park(context, &resume, spawns, asked, ascend, State::Empty, None)
}

/// Woken by the level below: add this level to its count and pass it up.
fn ascend<'graph>(
    context: &mut Context<'graph, '_, '_, '_>,
    resume: Resume<'graph, '_, '_>,
    _: &mut Spawns<'graph>,
) -> Action<'graph> {
    let Ok(Receipt::Value(KValue::Number(below))) = context.receipt(0) else {
        return Action::failed(StepError::Unredeemable);
    };
    fill(context, resume, below + 1.0)
}

/// Put one number in the slot this cell was born against.
fn fill<'graph>(
    context: &mut Context<'graph, '_, '_, '_>,
    resume: Resume<'graph, '_, '_>,
    count: f64,
) -> Action<'graph> {
    Action::deliver_scratch(context, &resume, move |_, _| {
        Active::new(KValue::Number(count))
    })
}

/// The root, woken by the head of the descent.
fn finish<'graph>(
    context: &mut Context<'graph, '_, '_, '_>,
    _: Resume<'graph, '_, '_>,
    _: &mut Spawns<'graph>,
) -> Action<'graph> {
    match context.receipt(0) {
        Ok(Receipt::Value(KValue::Number(count))) => record(count.to_string()),
        _ => return Action::failed(StepError::Unredeemable),
    }
    Action::done()
}

#[test]
fn a_subtree_two_hundred_deep_takes_no_slab_slot_but_its_root() {
    reset();
    // A slab of one. Every level of the descent is a tree child, so a level that reached for a
    // slab slot would be refused and the drain would stall before the bottom.
    let mut graph: Graph<'static> = Scheduler::graph(1);
    let mut scheduler = Scheduler::over(&mut graph);
    scheduler
        .admit(start, State::Empty)
        .expect("the slab admits");
    scheduler.run().expect("the drain runs to empty");

    assert_eq!(recorded(), ["200"], "every level answered the one above it");
    assert!(scheduler.is_empty());
}
