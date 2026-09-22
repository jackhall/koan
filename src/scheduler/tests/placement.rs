//! The placement hint, inverted over one workload. It is a hint and never a contract, so both ways
//! round compute the same thing; what it decides is what the memory does, and that shows in the
//! process allocation count.

use std::cell::Cell;

use crate::knot::KValue;
use crate::memory::Active;
use crate::scheduler::tests::bundle::{Native, TestGraph, TestStep};
use crate::scheduler::tests::native::{fresh, record, recorded, reset, work};
use crate::scheduler::{Action, Placement, Received, Scheduler, Step, StepError, Use, Work};

/// Enough turns that a region which only ever grows parts company with one that is recycled.
const MANY: usize = 4_000;

/// A short run of the same loop, to read the long one against: what the hops cost is the
/// difference over the difference, and it is that, not one reading, that the placement decides.
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
    mut step: Step<'_, 'graph, '_, '_, '_, Native>,
    head: TestStep<'graph>,
) -> Action<'graph, Native> {
    let asked = step.spawn(fresh(head, Use::Reads, KValue::Null));
    step.park(asked, finish, KValue::Null, None)
}

/// One turn: write a blob where this cell stands, then hand on or deliver.
///
/// The blob is built rather than carried, so what the placement decides is plain: a `Fresh`
/// successor writes a region drawn back from the pool its predecessor returned, while a `Shares`
/// successor writes its host's, which no turn of the loop ever returns.
fn turn<'graph>(
    step: Step<'_, 'graph, '_, '_, '_, Native>,
    placement: Placement,
    next: TestStep<'graph>,
) -> Action<'graph, Native> {
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
        return step.tail(
            placement,
            Work {
                step: next,
                state: KValue::Null,
            },
        );
    }
    step.finish_fresh(move |_, _| Active::new(KValue::Number(length)))
}

/// The caller, woken by the loop's last cell.
fn finish<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let first = step.results().next();
    match first {
        Some(Ok(Received::Scratch(KValue::Number(length)))) => record(length.to_string()),
        _ => return step.failed(StepError::Unredeemable),
    }
    step.done()
}

fn start_fresh<'graph>(step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    start(step, turn_fresh)
}

fn start_shares<'graph>(step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    start(step, turn_shares)
}

fn turn_fresh<'graph>(step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    turn(step, Placement::Fresh, turn_fresh)
}

fn turn_shares<'graph>(step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    turn(step, Placement::Shares, turn_shares)
}

/// What the workload computed, and what it cost the heap to compute it.
struct Run {
    recorded: Vec<String>,
    allocations: u64,
}

fn run(hops: usize, start: TestStep<'static>) -> Run {
    reset();
    REMAINING.with(|remaining| remaining.set(hops));
    let mut graph: TestGraph<'static> = TestGraph::new(1);
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
fn inverting_the_hint_changes_what_is_retained_and_no_value_the_program_computes() {
    let fresh_few = run(FEW, start_fresh);
    let fresh_many = run(MANY, start_fresh);
    let shares_few = run(FEW, start_shares);
    let shares_many = run(MANY, start_shares);

    // The hint is never a contract: soundness rests on the substrate's brands either way, and the
    // loop computes the same thing at either placement and at either length.
    let answer = [BLOB.len().to_string()];
    assert_eq!(fresh_few.recorded, answer);
    assert_eq!(fresh_many.recorded, answer);
    assert_eq!(shares_few.recorded, answer);
    assert_eq!(shares_many.recorded, answer);

    assert!(
        fresh_many.allocations <= fresh_few.allocations,
        "a sibling returns its region at each hand-off, so forty times the turns cost no more \
         heap: {} allocations against {}",
        fresh_many.allocations,
        fresh_few.allocations
    );
    assert!(
        shares_many.allocations > shares_few.allocations,
        "a co-tenant writes its host's region, which no turn returns, so the same forty times \
         costs more: {} allocations against {}",
        shares_many.allocations,
        shares_few.allocations
    );
    assert!(
        shares_many.allocations > fresh_many.allocations,
        "and so the one workload, hinted the other way round, retains what the first reclaimed: \
         {} allocations against {}",
        shares_many.allocations,
        fresh_many.allocations
    );
}
