//! A call subtree deeper than any slab cap. The tree pool takes no cap, so depth costs tree cells
//! and nothing else — which is why the matrix is one word wide.

use crate::function::KValue;
use crate::memory::{Active, Delivered, Receipt};
use crate::scheduler::tests::native::{record, recorded, reset};
use crate::scheduler::{
    Action, Context, Continuation, Placement, Request, Resume, Scheduler, Spawns, State, StepError,
    Work,
};

/// Deeper than the sixty-four slots a one-word matrix has, several times over.
const DEPTH: f64 = 200.0;

/// Spawn the head of the descent and park on its single slot.
fn start<'graph>(
    context: &mut Context<'graph, '_, '_, '_>,
    resume: Resume<'graph, '_>,
    spawns: &mut Spawns<'graph>,
) -> Action<'graph> {
    if context.register_receipts(1).is_err() {
        return Action::Failed(StepError::Undeliverable);
    }
    spawns.push(Request {
        placement: Placement::Fresh,
        work: Work {
            step: descend,
            state: State::Value(KValue::Number(DEPTH)),
        },
        slot: 0,
    });
    context.store_successor(Continuation::Native {
        step: finish,
        provenance: resume.provenance,
        state: State::Empty,
    });
    Action::Park
}

/// One level: park on a child one shallower, or turn around at the bottom.
fn descend<'graph>(
    context: &mut Context<'graph, '_, '_, '_>,
    resume: Resume<'graph, '_>,
    spawns: &mut Spawns<'graph>,
) -> Action<'graph> {
    let State::Value(KValue::Number(depth)) = resume.state else {
        return Action::Failed(StepError::Stale);
    };
    if depth == 0.0 {
        return fill(context, resume, 0.0);
    }
    if context.register_receipts(1).is_err() {
        return Action::Failed(StepError::Undeliverable);
    }
    spawns.push(Request {
        placement: Placement::Fresh,
        work: Work {
            step: descend,
            state: State::Value(KValue::Number(depth - 1.0)),
        },
        slot: 0,
    });
    context.store_successor(Continuation::Native {
        step: ascend,
        provenance: resume.provenance,
        state: State::Empty,
    });
    Action::Park
}

/// Woken by the level below: add this level to its count and pass it up.
fn ascend<'graph>(
    context: &mut Context<'graph, '_, '_, '_>,
    resume: Resume<'graph, '_>,
    _: &mut Spawns<'graph>,
) -> Action<'graph> {
    let Ok(Receipt::Value(KValue::Number(below))) = context.receipt(0) else {
        return Action::Failed(StepError::Unredeemable);
    };
    fill(context, resume, below + 1.0)
}

/// Put one number in the slot this cell was born against.
fn fill<'graph>(
    context: &mut Context<'graph, '_, '_, '_>,
    resume: Resume<'graph, '_>,
    count: f64,
) -> Action<'graph> {
    let Some(destination) = resume.provenance.destination else {
        return Action::Failed(StepError::Undeliverable);
    };
    let delivered = context.deliver_scratch(destination.consumer, destination.slot, move |_, _| {
        Active::new(KValue::Number(count))
    });
    match delivered {
        Ok(Delivered::Complete) => Action::Wakes(destination.consumer),
        Ok(Delivered::Outstanding) => Action::Done,
        Err(_) => Action::Failed(StepError::Undeliverable),
    }
}

/// The root, woken by the head of the descent.
fn finish<'graph>(
    context: &mut Context<'graph, '_, '_, '_>,
    _: Resume<'graph, '_>,
    _: &mut Spawns<'graph>,
) -> Action<'graph> {
    match context.receipt(0) {
        Ok(Receipt::Value(KValue::Number(count))) => record(count.to_string()),
        _ => return Action::Failed(StepError::Unredeemable),
    }
    Action::Done
}

#[test]
fn a_subtree_two_hundred_deep_takes_no_slab_slot_but_its_root() {
    reset();
    // A slab of one. Every level of the descent is a tree child, so a level that reached for a
    // slab slot would be refused and the drain would stall before the bottom.
    let mut scheduler: Scheduler<'static> = Scheduler::new(1);
    scheduler
        .admit(start, State::Empty)
        .expect("the slab admits");
    scheduler.run().expect("the drain runs to empty");

    assert_eq!(recorded(), ["200"], "every level answered the one above it");
    assert!(scheduler.is_empty());
}
