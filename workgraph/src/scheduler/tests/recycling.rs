//! Steady-state allocation slate: a slot that parks, wakes and re-parks on the same shape stops
//! allocating once its rows have grown.
//!
//! The shape is one **consumer** slot that never dies — it re-parks through `Replace` every
//! iteration — over a fresh fan of dep-free **producers** whose delivery wakes it. That drives all
//! four buffers the claim rests on: the consumer's dep row (taken and restored at step start), each
//! producer's notify row (taken and restored by the delivery walk), the drain's `dep_results`, and
//! the walk's wake list.
//!
//! Two fixture choices keep the reading exact rather than approximate:
//!
//! - every continuation is a **non-capturing** closure, so its `Box<dyn FnOnce>` is a `Box` of a
//!   ZST and takes no heap;
//! - every producer finalizes with an **error** terminal. Delivery then clones a `()` per
//!   destination instead of adopting a value, so no region's bump grows across iterations — a
//!   growing bump takes a fresh chunk from the system every doubling, which would land inside the
//!   measured window as unattributable drift. The buffers under test are the same either way: the
//!   walk fills every edge, decrements every pending, and wakes the consumer whichever arm the
//!   terminal is.
//!
//! Nothing is left: the window is asserted at exactly zero, so a reintroduced per-wake allocation
//! shows up as a `+1` rather than as drift.

use std::cell::Cell;

use super::super::{EdgeId, NodeId, Scheduler, StepVerdict};
use super::*;
use crate::tests::allocation_count;

/// Iterations run before the bracket opens, so every row and the drain's scratch bump have reached
/// their steady-state capacity.
const WARMUP: u32 = 16;
/// Iterations inside the bracket.
const MEASURED: u32 = 32;

/// Fan out `width` dep-free producers over `anchor` and mint one source edge each, into the caller's
/// reused buffer — the per-iteration upstream the consumer re-parks on.
fn spawn_producers(
    sched: &mut Scheduler<TestWorkload>,
    anchor: &Rc<TestAnchor>,
    width: usize,
    sources: &mut Vec<EdgeId>,
) {
    sources.clear();
    for _ in 0..width {
        let continuation: Box<dyn FnOnce() -> u32> = Box::new(|| 0);
        let producer = sched.alloc_node(NodeWork::new(continuation), &[], Rc::clone(anchor), false);
        sources.push(sched.install_edge(producer, anchor.owner()));
    }
}

/// Drive `WARMUP + MEASURED` park/wake iterations at `width` deps and hand back the allocation
/// delta over the measured window.
fn steady_state_delta(width: usize) -> u64 {
    let anchor = TestAnchor::fresh();
    let mut sched: Scheduler<TestWorkload> = Scheduler::new();
    let mut sources: Vec<EdgeId> = Vec::with_capacity(width);

    spawn_producers(&mut sched, &anchor, width, &mut sources);
    let continuation: Box<dyn FnOnce() -> u32> = Box::new(|| 0);
    let consumer: NodeId = sched.alloc_node(
        NodeWork::new(continuation),
        &sources,
        Rc::clone(&anchor),
        true,
    );
    for &source in &sources {
        sched.release_edge(source);
    }

    let iteration = Cell::new(0u32);
    let opened = Cell::new(0u64);
    let closed = Cell::new(0u64);
    sched
        .drain(|sched, step| {
            if step.id != consumer {
                return StepVerdict::Done(Err(()));
            }
            assert_eq!(
                step.dep_results.len(),
                width,
                "the consumer reads one result per dep it parked on",
            );
            let done = iteration.get() + 1;
            iteration.set(done);
            if done == WARMUP {
                opened.set(allocation_count());
            }
            if done == WARMUP + MEASURED {
                closed.set(allocation_count());
                return StepVerdict::Done(Err(()));
            }
            spawn_producers(sched, &anchor, width, &mut sources);
            // Through the scratch door, as an in-step wiring call does: the verdict list is read
            // and dropped inside this pop.
            let _installed = sched.install_deps_in(consumer, &sources, step.scratch);
            for &source in &sources {
                sched.release_edge(source);
            }
            let continuation: Box<dyn FnOnce() -> u32> = Box::new(|| 0);
            StepVerdict::Replace {
                work: NodeWork::new(continuation),
                anchor: None,
            }
        })
        .expect("an acyclic run drains clean");

    closed.get() - opened.get()
}

/// **The steady state allocates nothing at all.** No row vector, no wiring buffer, no wake list and
/// no scratch of the debug acyclicity walk is re-allocated once the shape has been seen — including
/// under `debug_assertions`, where that walk runs on every parked dep.
///
/// Run at two dep widths so a buffer that silently reverted to per-wake reallocation cannot hide
/// inside a width-independent constant: a row rebuilt from empty costs one allocation per growth
/// step, so the two readings would leave this line at different rates.
#[test]
fn a_steady_state_park_and_wake_allocates_nothing() {
    for width in [2usize, 8] {
        assert_eq!(
            steady_state_delta(width),
            0,
            "a {width}-dep park/wake loop allocates nothing over {MEASURED} iterations",
        );
    }
}
