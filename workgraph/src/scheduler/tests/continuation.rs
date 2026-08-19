//! Miri slate (tree borrows) for the scheduler's **owned-tier continuation slot**: a droppable
//! continuation rests as `SealedPinned<W::Continuation, Rc<W::Frame>>`, sealed against the slot's
//! anchor at install and opened once per step. The two shapes that matter are the ones the tier
//! exists for — a parked slot dropped *unopened* (the seal's glue must run while its own pin still
//! holds the region the continuation borrows into) and the park → wake → open → run round trip.
//!
//! Fails on UB, not values; the fixtures it runs on live in the parent module.

use std::cell::Cell;
use std::rc::Rc;

use super::super::Scheduler;
use super::super::nodes::NodeWork;
use super::*;
use crate::witnessed::{NoPins, SealedExtern};

/// A capture whose destructor dereferences a region borrow — so a continuation holding one is sound
/// to drop unopened only while the region is still pinned.
struct DropProbe<'r> {
    last: &'r u32,
    seen: Rc<Cell<u32>>,
}

impl Drop for DropProbe<'_> {
    fn drop(&mut self) {
        self.seen.set(*self.last);
    }
}

/// **The parked droppable continuation, dropped unopened.** The scheduler is torn down with the
/// slot still `PreRun`, and by then the seal's bundled pin is the last `Rc` on the region the
/// continuation's captured probe dereferences in its destructor. `Scheduler`'s field order drops
/// `deps` (which holds the slot's anchor row) before `store`, so the seal's own pin is what keeps
/// the region alive while the glue runs.
#[test]
fn parked_continuation_drops_under_its_own_pin() {
    let anchor = TestAnchor::fresh();
    let seen = Rc::new(Cell::new(0u32));
    let alive = Rc::downgrade(anchor.owner());

    let mut sched: Scheduler<TestWorkload> = Scheduler::new();
    {
        let probe = DropProbe {
            last: anchor.handle().allocator().value(42u32),
            seen: Rc::clone(&seen),
        };
        let continuation: Box<dyn FnOnce() -> u32 + '_> = Box::new(move || *probe.last);
        sched.alloc_node(NodeWork::new(continuation), &[], Rc::clone(&anchor), false);
    }
    drop(anchor);
    assert!(
        alive.upgrade().is_some(),
        "the parked slot's pins are the sole holders",
    );

    drop(sched);
    assert_eq!(seen.get(), 42, "the continuation's glue ran");
    assert!(alive.upgrade().is_none(), "teardown released the region");
}

/// **Park → wake → open → run.** The installed continuation is erased to `'static` behind the
/// dormant union slot, popped off the ready queue, re-anchored by the owned tier's one open verb
/// beside a trivial extern operand, and invoked inside the brand — reading its captured region
/// borrow after every direct handle on that region is gone.
#[test]
fn parked_continuation_opens_and_runs_after_its_handles_drop() {
    let anchor = TestAnchor::fresh();

    let mut sched: Scheduler<TestWorkload> = Scheduler::new();
    let id = {
        let captured: &u32 = anchor.handle().allocator().value(9u32);
        let continuation: Box<dyn FnOnce() -> u32 + '_> = Box::new(move || *captured);
        sched.alloc_node(NodeWork::new(continuation), &[], Rc::clone(&anchor), false)
    };
    drop(anchor);

    let ready = sched.pop_next().expect("a dep-free slot is ready");
    assert_eq!(ready, id, "the ready slot is the one just installed");

    let (work, _anchor) = sched.take_for_run(id);
    let sealed = work.continuation;
    let got = sealed.open(
        SealedExtern::<UnitOperand>::erase(()),
        &NoPins,
        |_within, continuation: Box<dyn FnOnce() -> u32 + '_>, ()| continuation(),
    );
    assert_eq!(got, 9);
}
