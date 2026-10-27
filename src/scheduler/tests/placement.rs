//! A hop's placement over one workload: every hop is born in a region of its own, drawn back from
//! the pool its predecessor returned, so a loop of hops costs the heap nothing per turn, and that
//! shows in the process allocation count.

use std::cell::Cell;

use crate::knot::KValue;
use crate::memory::Active;
use crate::memory::Bump;
use crate::scheduler::tests::bundle::{AnyStep, Native, TestGraph, TestStep};
use crate::scheduler::tests::native::{fresh, record, recorded, reset, work};
use crate::scheduler::{Action, Placement, Received, Scheduler, Step, StepError, Use, Work};

/// Enough turns that a region which only ever grows parts company with one that is recycled.
const MANY: usize = 4_000;

/// A short run of the same loop, to read the long one against: what the hops cost is the
/// difference over the difference.
const FEW: usize = 100;

/// What each turn writes into whichever region it is standing in.
const BLOB: &str = "sixty-four bytes of text, repeated until the region notices it\
                    sixty-four bytes of text, repeated until the region notices it\
                    sixty-four bytes of text, repeated until the region notices it\
                    sixty-four bytes of text, repeated until the region notices it";

thread_local! {
    /// Turns left before the loop delivers. A native step is a bare `fn` and holds no counter.
    static REMAINING: Cell<usize> = const { Cell::new(0) };
}

/// The caller: ask for the loop's head, park on its single slot.
fn start<'graph>(
    mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>,
    head: TestStep<'graph>,
) -> Action<'graph, Native> {
    let asked = step.spawn(fresh(head, Use::Reads, KValue::Null));
    step.park(asked, finish, KValue::Null, None)
}

/// One turn: write a blob where this cell stands, then hop on or deliver. The blob is built rather
/// than carried, so the successor writes a region drawn back from the pool its predecessor
/// returned.
fn turn<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let written: KValue<'graph, '_> = crate::values::text(step.writer(), BLOB);
    let KValue::Str(text) = written else {
        return step.failed(StepError::Stale);
    };
    let length = text.len() as f64;
    let left = REMAINING.with(|remaining| {
        let left = remaining.get();
        remaining.set(left.saturating_sub(1));
        left
    });
    if left > 0 {
        return step.tail(Work {
            step: turn,
            state: KValue::Null,
        });
    }
    step.finish_fresh(move |_, _, _| Active::new(KValue::Number(length)))
}

/// The caller, woken by the loop's last cell.
fn finish<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let first = step.results().next();
    match first {
        Some(Ok(Received::Scratch(KValue::Number(length)))) => record(length.to_string()),
        _ => return step.failed(StepError::Unredeemable),
    }
    step.done()
}

fn start_turning<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    start(step, turn)
}

/// What the workload computed, and what it cost the heap to compute it.
struct Run {
    recorded: Vec<String>,
    allocations: u64,
}

fn run(hops: usize, start: AnyStep) -> Run {
    reset();
    REMAINING.with(|remaining| remaining.set(hops));
    let type_region = Bump::new();
    let mut graph: TestGraph<'_> = TestGraph::new(
        1,
        crate::values::tests::seeded_tiers(&type_region),
        Default::default(),
    );
    let root = graph.root().expect("a fresh slab admits a root");
    let mut scheduler = Scheduler::over(&mut graph);
    let before = crate::tests::allocation_count();
    scheduler
        .run(work(start, KValue::Null), root, Placement::Fresh)
        .expect("the root work ends");
    let allocations = crate::tests::allocation_count() - before;
    graph.release_root(root).expect("the root releases");
    assert!(graph.is_empty());
    Run {
        recorded: recorded(),
        allocations,
    }
}

#[test]
fn a_loop_of_hops_costs_the_heap_nothing_per_turn() {
    let few = run(FEW, start_turning);
    let many = run(MANY, start_turning);
    let answer = [BLOB.len().to_string()];
    assert_eq!(few.recorded, answer);
    assert_eq!(many.recorded, answer);
    assert!(
        many.allocations <= few.allocations,
        "a hop returns its region to the pool its successor draws from, so forty times the turns \
         cost no more heap: {} allocations against {}",
        many.allocations,
        few.allocations
    );
}
