//! A tail hop: a cell hands its work to a successor and dies, and a loop of them runs in bounded
//! memory. Once at each placement, because what the hand-off costs is exactly what the placement
//! decides — a forced copy between siblings, nothing at all between co-tenants.

use std::cell::Cell;

use crate::knot::KValue;
use crate::memory::Active;
use crate::scheduler::tests::bundle::{Native, TestGraph, TestStep};
use crate::scheduler::tests::native::{describe, fresh, record, recorded, reset, shares, work};
use crate::scheduler::{Action, Placement, Received, Scheduler, Step, StepError, Taken, Use, Work};

/// Enough hops that a per-hop cost would be unmissable in the allocation count.
const MANY: usize = 10_000;

/// A short run of the same loop. The two counts are read against each other: what a hop costs is
/// the difference over the difference, so a flat reading is a loop that pays nothing per hop.
const FEW: usize = 100;

/// What the loop carries: a value with bytes in a region, so a crossing has something to copy.
const CARRIED: &str = "a value with bytes in it";

thread_local! {
    /// Hops left before the loop delivers. A native step is a bare `fn` and holds no counter of
    /// its own, and the carried value is the crossing's subject rather than the loop's bookkeeping.
    static REMAINING: Cell<usize> = const { Cell::new(0) };
}

/// The caller: ask for the loop's head at `placement`, park on its single slot.
fn start<'graph>(
    mut step: Step<'_, 'graph, '_, '_, '_, Native>,
    placement: Placement,
    head: TestStep<'graph>,
) -> Action<'graph, Native> {
    let request = match placement {
        Placement::Fresh => fresh(head, Use::Reads, KValue::Null),
        Placement::Shares => shares(head, Use::Reads, KValue::Null),
    };
    let asked = step.spawn(request);
    step.park(asked, finish, KValue::Null, None)
}

/// One turn of the loop: take the carried value, and either hop again or deliver.
///
/// The first turn has nothing to take and builds the value instead, so the same step is both the
/// head of the loop and every hop of it. What the veneer's wake cost is the placement's whole
/// story: a sibling is `Apart` from the cell that kept the value, so it arrives by a forced copy
/// into this cell's own region; a co-tenant writes the same host, so the same wake pins what is
/// already at its `'here`.
fn turn<'graph>(
    step: Step<'_, 'graph, '_, '_, '_, Native>,
    placement: Placement,
    next: TestStep<'graph>,
) -> Action<'graph, Native> {
    let (step, state) = step.state();
    let carried = match state {
        KValue::Null => crate::values::text(step.writer(), CARRIED),
        carried => carried,
    };
    let left = REMAINING.with(|remaining| {
        let left = remaining.get();
        remaining.set(left.saturating_sub(1));
        left
    });
    if left == 0 {
        return deliver(step, carried);
    }
    step.tail(
        placement,
        Work {
            step: next,
            state: carried,
        },
    )
}

/// The last turn: fill the slot the loop's head was born against. Every hop carried the same
/// provenance forward, so the caller sees one result however many cells produced it.
fn deliver<'graph>(
    step: Step<'_, 'graph, '_, '_, '_, Native, Taken>,
    carried: KValue<'graph, '_>,
) -> Action<'graph, Native> {
    // Recorded here rather than delivered, because what proves the value survived every crossing is
    // its bytes, and its bytes are at this cell's brand.
    record(describe(carried));
    let KValue::Str(text) = carried else {
        return step.failed(StepError::Stale);
    };
    let length = text.len() as f64;
    step.finish_fresh(move |_, _| Active::new(KValue::Number(length)))
}

/// The caller, woken by the loop's last cell.
fn finish<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let first = step.results().next();
    match first {
        Some(Ok(Received::Scratch(value))) => record(describe(value)),
        _ => return step.failed(StepError::Unredeemable),
    }
    step.done()
}

fn start_fresh<'graph>(step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    start(step, Placement::Fresh, turn_fresh)
}

fn start_shares<'graph>(step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    start(step, Placement::Shares, turn_shares)
}

fn turn_fresh<'graph>(step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    turn(step, Placement::Fresh, turn_fresh)
}

fn turn_shares<'graph>(step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    turn(step, Placement::Shares, turn_shares)
}

/// What one run of the loop leaves behind: what it recorded, the most cells it held live at once,
/// and the heap allocations the drain made while it ran.
struct Run {
    recorded: Vec<String>,
    peak: usize,
    allocations: u64,
}

/// One loop of `hops` turns, on a slab of one — its root — so a second slab cell would be
/// refused, and a loop that grew its slab footprint could not finish.
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
    let peak = scheduler.peak_live_cells();
    graph.release_root(root).expect("the root releases");
    assert!(graph.is_empty(), "the loop leaves no cell behind");
    Run {
        recorded: recorded(),
        peak,
        allocations,
    }
}

/// The caller, the hop running, and the hop it is handing to: three cells, whatever `hops` is.
const PEAK: usize = 3;

#[test]
fn a_fresh_loop_of_ten_thousand_hops_runs_in_three_cells_and_a_flat_heap() {
    let few = run(FEW, start_fresh);
    let many = run(MANY, start_fresh);

    assert_eq!(few.recorded, [CARRIED, "24"]);
    assert_eq!(many.recorded, few.recorded, "a hop changes no value");
    assert_eq!(many.peak, PEAK, "a hop releases its predecessor");
    assert_eq!(few.peak, PEAK);
    assert!(
        many.allocations <= few.allocations,
        "a hundred times the hops and no more heap ({} against {}): every hop's region is recycled \
         into the next",
        many.allocations,
        few.allocations
    );
}

#[test]
fn a_shares_loop_of_ten_thousand_hops_copies_nothing_into_its_host() {
    let few = run(FEW, start_shares);
    let many = run(MANY, start_shares);

    assert_eq!(few.recorded, [CARRIED, "24"]);
    assert_eq!(many.recorded, few.recorded, "a hop changes no value");
    assert_eq!(many.peak, PEAK, "a hop releases its predecessor");
    assert!(
        many.allocations <= few.allocations,
        "a co-tenant's crossing pins, so a hop writes nothing: {} allocations against {} for a \
         hundredth of the hops",
        many.allocations,
        few.allocations
    );
}

/// The hand-off at its minimal shape, both ways round: three hops, and the carried bytes read
/// back at the end.
///
/// The long loops above read the heap; this one reads the memory. A successor redeems out of a
/// region the drain reclaims in the round that follows, and — at `Fresh` — hands straight back out
/// of the pool to the hop after it, so every byte the loop carries is written into a region that
/// was somebody else's a moment ago.
#[test]
fn a_hand_off_redeems_out_of_the_region_the_drain_reclaims_next() {
    let fresh = run(3, start_fresh);
    assert_eq!(
        fresh.recorded,
        [CARRIED, "24"],
        "three hops and nothing lost"
    );
    assert_eq!(fresh.peak, PEAK);

    let shares = run(3, start_shares);
    assert_eq!(shares.recorded, fresh.recorded);
    assert_eq!(shares.peak, PEAK);
}
