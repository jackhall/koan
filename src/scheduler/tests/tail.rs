//! A tail hop: a cell hands its work to a successor and dies, and a loop of them runs in bounded
//! memory. Once at each placement, because what the hand-off costs is exactly what the placement
//! decides — a forced copy between siblings, nothing at all between co-tenants.

use std::cell::Cell;

use crate::function::{KValue, KValueFamily};
use crate::memory::{Active, Delivered, Receipt};
use crate::scheduler::tests::native::{describe, record, recorded, reset};
use crate::scheduler::{
    Action, Context, Continuation, DrainStalled, Hop, NativeStep, Placement, Request, Resume,
    Scheduler, Spawns, State, StepError,
};

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
    context: &mut Context<'graph, '_, '_, '_>,
    resume: Resume<'graph, '_>,
    spawns: &mut Spawns<'graph>,
    placement: Placement,
    step: NativeStep<'graph>,
) -> Action<'graph> {
    if context.register_receipts(1).is_err() {
        return Action::Failed(StepError::Undeliverable);
    }
    spawns.push(Request {
        placement,
        step,
        state: State::Empty,
        slot: 0,
    });
    context.store_successor(Continuation::Native {
        step: finish,
        provenance: resume.provenance,
        state: State::Empty,
    });
    Action::Park
}

/// One turn of the loop: take the carried value in, and either hop again or deliver.
///
/// The first turn has nothing to take in and builds the value instead, so the same step is both the
/// head of the loop and every hop of it.
fn turn<'graph>(
    context: &mut Context<'graph, '_, '_, '_>,
    resume: Resume<'graph, '_>,
    placement: Placement,
    step: NativeStep<'graph>,
) -> Action<'graph> {
    let carried: KValue<'graph, '_> = match resume.state {
        State::Parked(dormant) => {
            let Ok(carrier) = context.redeem(dormant) else {
                return Action::Failed(StepError::Unredeemable);
            };
            // What this costs is the placement's whole story. A sibling is `Apart` from the cell
            // that kept the value, so the crossing is a forced copy into this cell's own region; a
            // co-tenant writes the same host, so the same call pins what is already at its `'here`.
            crate::values::cross_here(context, &carrier)
        }
        _ => crate::values::text(context.writer(), CARRIED),
    };
    let left = REMAINING.with(|remaining| {
        let left = remaining.get();
        remaining.set(left.saturating_sub(1));
        left
    });
    if left == 0 {
        return deliver(context, resume, carried);
    }
    let carrier = context.lift::<KValueFamily>(carried);
    let state = State::Parked(context.keep(carrier));
    Action::Tail(Hop {
        placement,
        step,
        state,
    })
}

/// The last turn: fill the slot the loop's head was born against. Every hop carried the same
/// destination forward, so the caller sees one result however many cells produced it.
fn deliver<'graph>(
    context: &mut Context<'graph, '_, '_, '_>,
    resume: Resume<'graph, '_>,
    carried: KValue<'graph, '_>,
) -> Action<'graph> {
    let Some(destination) = resume.provenance.destination else {
        return Action::Failed(StepError::Undeliverable);
    };
    // Recorded here rather than delivered, because what proves the value survived every crossing is
    // its bytes, and its bytes are at this cell's brand.
    record(describe(carried));
    let KValue::Str(text) = carried else {
        return Action::Failed(StepError::Stale);
    };
    let length = text.len() as f64;
    let delivered = context.deliver_scratch(destination.consumer, destination.slot, move |_, _| {
        Active::new(KValue::Number(length))
    });
    match delivered {
        Ok(Delivered::Complete) => Action::Wakes(destination.consumer),
        Ok(Delivered::Outstanding) => Action::Done,
        Err(_) => Action::Failed(StepError::Undeliverable),
    }
}

/// The caller, woken by the loop's last cell.
fn finish<'graph>(
    context: &mut Context<'graph, '_, '_, '_>,
    _: Resume<'graph, '_>,
    _: &mut Spawns<'graph>,
) -> Action<'graph> {
    match context.receipt(0) {
        Ok(Receipt::Value(value)) => record(describe(value)),
        _ => return Action::Failed(StepError::Unredeemable),
    }
    Action::Done
}

fn start_fresh<'graph>(
    context: &mut Context<'graph, '_, '_, '_>,
    resume: Resume<'graph, '_>,
    spawns: &mut Spawns<'graph>,
) -> Action<'graph> {
    start(context, resume, spawns, Placement::Fresh, turn_fresh)
}

fn start_shares<'graph>(
    context: &mut Context<'graph, '_, '_, '_>,
    resume: Resume<'graph, '_>,
    spawns: &mut Spawns<'graph>,
) -> Action<'graph> {
    start(context, resume, spawns, Placement::Shares, turn_shares)
}

fn turn_fresh<'graph>(
    context: &mut Context<'graph, '_, '_, '_>,
    resume: Resume<'graph, '_>,
    _: &mut Spawns<'graph>,
) -> Action<'graph> {
    turn(context, resume, Placement::Fresh, turn_fresh)
}

fn turn_shares<'graph>(
    context: &mut Context<'graph, '_, '_, '_>,
    resume: Resume<'graph, '_>,
    _: &mut Spawns<'graph>,
) -> Action<'graph> {
    turn(context, resume, Placement::Shares, turn_shares)
}

/// What one run of the loop leaves behind: what it recorded, the most cells it held live at once,
/// and the heap allocations the drain made while it ran.
struct Run {
    recorded: Vec<String>,
    peak: usize,
    allocations: u64,
}

/// One loop of `hops` turns, on a slab of one — so a second slab cell would be refused, and a loop
/// that grew its slab footprint could not finish.
fn run(hops: usize, start: NativeStep<'static>) -> Run {
    reset();
    REMAINING.with(|remaining| remaining.set(hops));
    let mut scheduler: Scheduler<'static> = Scheduler::new(1);
    scheduler
        .admit(start, State::Empty)
        .expect("the slab admits");
    let before = crate::tests::allocation_count();
    scheduler.run().expect("the drain runs to empty");
    let allocations = crate::tests::allocation_count() - before;
    assert!(scheduler.is_empty(), "the loop leaves no cell behind");
    Run {
        recorded: recorded(),
        peak: scheduler.peak_live_cells(),
        allocations,
    }
}

/// The caller, the hop running, and the hop it is handing to: three cells, whatever `hops` is.
const PEAK: usize = 3;

#[test]
fn a_fresh_loop_of_ten_thousand_hops_runs_in_two_cells_and_a_flat_heap() {
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

/// A slab cell asking to hop. Every other step here is reached through a spawn, so this one is
/// admitted straight into the slab.
fn hop_from_the_slab<'graph>(
    _: &mut Context<'graph, '_, '_, '_>,
    _: Resume<'graph, '_>,
    _: &mut Spawns<'graph>,
) -> Action<'graph> {
    Action::Tail(Hop {
        placement: Placement::Fresh,
        step: hop_from_the_slab,
        state: State::Empty,
    })
}

#[test]
fn a_slab_cell_has_no_sibling_to_hop_to() {
    let mut scheduler: Scheduler<'static> = Scheduler::new(1);
    scheduler
        .admit(hop_from_the_slab, State::Empty)
        .expect("the slab admits");
    // A successor would have to be a slab cell of its own, holding its predecessor to redeem — and
    // a held predecessor seals rather than reclaims, which is the one thing a hop exists to avoid.
    assert_eq!(scheduler.run(), Err(DrainStalled::Unhoppable));
}
