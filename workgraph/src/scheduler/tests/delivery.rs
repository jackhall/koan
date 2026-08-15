//! Miri slate (tree borrows) for **delivery at finalize**: the walk that distributes a producer's
//! terminal into every destination waiting on it, and the reclaim that follows it.
//!
//! Each test is a *timeline* — what is alive when, and for how long — rather than a value check.
//! Under Miri they fail on UB; run natively they fail on a liveness probe. The five shapes here are
//! the ones the flip's soundness rests on:
//!
//! 1. a **copy** verdict frees the producer's region at its own finalize,
//! 2. a **pin** verdict transfers it by hold and frees it with the destination,
//! 3. a **consumer that dies before its producer fires** leaves released entries the walk skips and
//!    recycles (Inv-C),
//! 4. a **late wire onto a delivered edge** shares the resident rather than adopting again, and
//! 5. a **root edge** keeps its value readable until the drain boundary releases it.
//!
//! The fixtures come from the parent module: `TestAnchor` owns a real `Region`, `TestWorkload`
//! delivers by copy and `PinWorkload` by pin, so the verdict is the only thing that varies.

use std::rc::Rc;

use super::super::{EdgeId, NodeId, Scheduler, Workload};
use super::*;

/// Run a slot's step and finalize it with `output` — take the work (`PreRun` → `Running`), then
/// deliver. The two together are what a driver's step boundary does.
fn finalize<W>(sched: &mut Scheduler<W>, id: NodeId, output: Result<DeliveredTerminal<W>, ()>)
where
    W: Workload<Error = ()>,
{
    let _ = sched.take_for_run(id);
    sched.finalize(id, output);
}

/// Read the `u32` resting on `edge`, copied out from inside the read.
fn read<W>(sched: &Scheduler<W>, edge: EdgeId) -> u32
where
    W: Workload<Value = U32Value>,
{
    match sched.read_edge_result_with(edge, |v| *v) {
        Ok(value) => value,
        Err(_) => panic!("a value terminal, not an error"),
    }
}

/// **Timeline 1 — the copy verdict frees the producer's region at finalize.** The consumer's edge
/// names its own region; delivery rebuilds the value there and claims nothing of the source, so the
/// producer's anchor is the last hold on its region and dropping it at reclaim frees it. The value
/// stays readable afterwards, because it no longer lives there.
#[test]
fn copy_verdict_frees_the_producer_region_at_finalize() {
    let mut sched: Scheduler<TestWorkload> = Scheduler::new();
    let (producer, producer_anchor, producer_region) = alloc_slot(&mut sched);
    let (_consumer, consumer_anchor, _) = alloc_slot(&mut sched);
    let edge = sched.install_edge(producer, consumer_anchor.owner());

    let output = terminal::<TestWorkload>(&producer_anchor, 41);
    // The test's own hold goes first, so the scheduler's row is the only one left.
    drop(producer_anchor);
    assert!(
        producer_region.upgrade().is_some(),
        "the slot's anchor row is holding the producer region",
    );

    finalize(&mut sched, producer, Ok(output));

    assert!(
        producer_region.upgrade().is_none(),
        "a copy-verdict delivery leaves nothing claiming the producer region, so it frees at reclaim",
    );
    assert_eq!(read(&sched, edge), 41, "the value lives at its destination");
}

/// **Timeline 2 — the pin verdict transfers the producer's region by hold.** The relocation hands
/// the source borrow through and claims it, so the mint retains the producer's owner in the
/// destination region's union bundle: the producer region outlives its own reclaim and dies exactly
/// when the destination does.
#[test]
fn pin_verdict_holds_the_producer_region_until_its_destination_dies() {
    let mut sched: Scheduler<PinWorkload> = Scheduler::new();
    let (producer, producer_anchor, producer_region) = alloc_slot(&mut sched);
    let (_consumer, consumer_anchor, _) = alloc_slot(&mut sched);
    let edge = sched.install_edge(producer, consumer_anchor.owner());

    let output = terminal::<PinWorkload>(&producer_anchor, 42);
    drop(producer_anchor);

    finalize(&mut sched, producer, Ok(output));

    assert!(
        producer_region.upgrade().is_some(),
        "a pin-verdict delivery claims the source, so the destination's retention holds it",
    );
    assert_eq!(read(&sched, edge), 42, "the value reads through the pin");

    // Tear the destination down: its union bundle is the last hold on the producer's region.
    sched.release_edge(edge);
    drop(sched);
    drop(consumer_anchor);
    assert!(
        producer_region.upgrade().is_none(),
        "the producer region dies with the destination that held it",
    );
}

/// **Timeline 3 — a consumer that dies before its producer fires.** Its teardown releases the edge
/// it owns and its region goes with it; the producer's later walk meets a released slab entry, skips
/// it — never dereferencing the destination that is now gone — and recycles the index (Inv-C). No
/// ordering is required between the two parties, and a live sibling edge is delivered regardless.
///
/// Under Miri this is where a walk that read a released entry's destination would be caught; in
/// debug builds the edge's weak shadow would fire first.
#[test]
fn a_dead_consumers_edges_are_skipped_and_recycled() {
    let mut sched: Scheduler<TestWorkload> = Scheduler::new();
    let (producer, producer_anchor, _) = alloc_slot(&mut sched);
    let doomed_destination = TestAnchor::fresh();
    let doomed_region = Rc::downgrade(doomed_destination.owner());
    let survivor_destination = TestAnchor::fresh();

    let doomed = sched.install_edge(producer, doomed_destination.owner());
    let survivor = sched.install_edge(producer, survivor_destination.owner());

    // The consumer dies: it releases the edge it owns, then its region goes.
    sched.release_edge(doomed);
    assert_eq!(
        sched.edge_free_list_len(),
        0,
        "a released *listed* edge withholds its index until the walk drops the entry (Inv-C)",
    );
    drop(doomed_destination);
    assert!(
        doomed_region.upgrade().is_none(),
        "nothing in the slab claims a destination, so the dead consumer's region is gone",
    );

    let output = terminal::<TestWorkload>(&producer_anchor, 7);
    drop(producer_anchor);
    sched.finalize(producer, Ok(output)); // no `take_for_run`: the slot is still parked-shaped here

    assert_eq!(
        sched.edge_free_list_len(),
        1,
        "the walk recycled the released entry as it dropped it",
    );
    assert_eq!(
        read(&sched, survivor),
        7,
        "the live edge received delivery regardless",
    );
}

/// **Timeline 4 — a late wire onto a delivered edge shares the resident.** The producer has already
/// delivered into the destination the source edge names; wiring a second edge off that source
/// inherits the same destination, so install hands back the same value rather than adopting into
/// that region twice. Pointer identity is the observation.
#[test]
fn a_late_wire_onto_a_delivered_edge_shares_the_resident() {
    let mut sched: Scheduler<TestWorkload> = Scheduler::new();
    let (producer, producer_anchor, _) = alloc_slot(&mut sched);
    let (_consumer, consumer_anchor, _) = alloc_slot(&mut sched);
    let source = sched.install_edge(producer, consumer_anchor.owner());

    let output = terminal::<TestWorkload>(&producer_anchor, 11);
    drop(producer_anchor);
    finalize(&mut sched, producer, Ok(output));

    let derived = sched.install_edge_from(source);
    assert!(
        matches!(derived, super::super::InstalledEdge::Filled(_)),
        "the producer has delivered, so the late wire fills rather than parking",
    );

    let address = |edge| match sched.read_edge_result_with(edge, std::ptr::from_ref) {
        Ok(address) => address,
        Err(_) => panic!("a value terminal, not an error"),
    };
    assert_eq!(
        address(source),
        address(derived.edge_id()),
        "both edges name one destination, so they name one resident — no second adopt",
    );
    assert_eq!(read(&sched, derived.edge_id()), 11);
}

/// **Timeline 5 — root-edge release at the drain boundary.** A top-level root has no consumer node:
/// its edge is owned by the run frame it destines into. The terminal is delivered there at the
/// producer's finalize and stays readable as an ordinary resident of that region until the drain
/// releases the edge — which is the only lifecycle act a root needs.
#[test]
fn a_root_edge_holds_its_value_until_the_drain_releases_it() {
    let mut sched: Scheduler<TestWorkload> = Scheduler::new();
    let run_frame = TestAnchor::fresh();
    let run_region = Rc::downgrade(run_frame.owner());
    let (producer, producer_anchor, producer_region) = alloc_slot(&mut sched);
    let root = sched.install_edge(producer, run_frame.owner());

    let output = terminal::<TestWorkload>(&producer_anchor, 99);
    drop(producer_anchor);
    finalize(&mut sched, producer, Ok(output));

    assert!(
        producer_region.upgrade().is_none(),
        "the root's value was copied into the run region, so the producer's own region is done",
    );
    assert_eq!(
        read(&sched, root),
        99,
        "a drain-boundary read is a resident read of the run region",
    );

    // The drain boundary: koan owns the root, so koan releases it before the run frame goes.
    sched.release_edge(root);
    drop(sched);
    drop(run_frame);
    assert!(
        run_region.upgrade().is_none(),
        "nothing outlives the run frame the root delivered into",
    );
}

/// **An error terminal delivers per edge, cloned.** It carries no value to adopt, so every waiting
/// edge gets its own copy and the reclaim is the same unconditional one.
#[test]
fn an_error_terminal_reaches_every_waiting_edge() {
    let mut sched: Scheduler<TestWorkload> = Scheduler::new();
    let (producer, producer_anchor, producer_region) = alloc_slot(&mut sched);
    let (_consumer, consumer_anchor, _) = alloc_slot(&mut sched);
    let first = sched.install_edge(producer, consumer_anchor.owner());
    let second = sched.install_edge(producer, consumer_anchor.owner());
    drop(producer_anchor);

    finalize(&mut sched, producer, Err(()));

    assert!(sched.edge_result_error(first).is_err());
    assert!(sched.edge_result_error(second).is_err());
    assert!(
        producer_region.upgrade().is_none(),
        "an error reaches nothing, so the producer's region frees at reclaim like any other",
    );
}
