//! Delivery into a parked cell's scratch: the run of receipt slots the substrate lays down in a
//! cell's scratch habitat, the two doors another cell's step fills one with, and the drain the
//! owning cell reads them back through.
//!
//! What these pin: a run rests where its own cell's registration put it and holds that bump's reset
//! off; a cell parking round after round on receipts alone starts every round at the foot of a bump
//! handed back whole; a registration replaces a drained run and is refused over an undrained one;
//! both fills survive their producer, including as an honest `Gone`; every refusal leaves the run
//! byte for byte as it found it; and a tenant's run is the tenant's own though the bytes under it
//! are its host's.

use super::super::*;
use super::{Number, number_here, one, operand_at, pin, pinned};

/// The continuation: a run in the cell's region.
struct Storage;
crate::reattachable!(Storage => &'cell [u32]);

/// The scratch state: a borrow of the cell's own scratch bump, so a state at rest names real bytes.
struct Held;
crate::reattachable!(both Held => &'scratch u32);

/// The delivered scratch family: a number a producer writes into the consumer's scratch habitat.
struct Note;
crate::reattachable!(Note => &'cell u32);
impl DropFree for Note {}

/// The bundle: a note built in the consumer's scratch, a [`Number`] carrier filed at rest.
struct Push;
impl<'graph> Delivery<'graph> for Push {
    type Scratch = Note;
    type Carrier = Number;
}

/// A bundle whose scratch fill allocates nothing — the bare signal, "re-read your slot".
struct Signal;
crate::reattachable!(Signal => ());
impl DropFree for Signal {}
impl<'graph> Delivery<'graph> for Signal {
    type Scratch = Signal;
    type Carrier = Number;
}

type Graph = CellGraph<'static, Storage, Held, Push>;

/// Scratch bytes in use in the bump a step in `cell` writes: its own, or its host's.
fn scratch_in_use<D: Delivery<'static>>(
    graph: &CellGraph<'static, Storage, Held, D>,
    cell: impl Into<CellHandle>,
) -> usize {
    let home = graph
        .cells
        .write_home(cell.into())
        .expect("the cell is live");
    graph.regions.scratch_in_use(home)
}

/// The number a `Value` receipt held, or a failure naming what came back instead.
fn note(receipt: Receipt<'static, '_, '_, Push, 1>) -> u32 {
    match receipt {
        Receipt::Value(value) => *value,
        Receipt::Empty => panic!("the slot was empty"),
        Receipt::Carrier(_) => panic!("the slot held a carrier"),
    }
}

/// Register a one-slot run and park on it.
fn await_one(graph: &mut Graph, cell: impl Into<CellHandle>) {
    graph
        .enter(cell, |context| context.register_receipts(1).unwrap())
        .unwrap();
}

/// File a note in one slot of `consumer`'s run, from a step in `producer`.
fn send(
    graph: &mut Graph,
    producer: impl Into<CellHandle>,
    consumer: impl Into<CellHandle>,
    slot: usize,
    value: u32,
) -> Result<Delivered, DeliverError> {
    let consumer = consumer.into();
    graph
        .enter(producer, |context| {
            context.deliver_scratch(consumer, slot, |writer, _| Active::new(one(writer, value)))
        })
        .unwrap()
}

#[test]
fn a_run_rests_where_its_registration_put_it_and_drains_one_slot_at_a_time() {
    let mut graph: Graph = CellGraph::new(2, pin);
    let consumer = graph.create(None).unwrap();
    let producer = graph.create(None).unwrap();

    graph
        .enter(consumer, |context| {
            assert_eq!(context.receipt_count(), None, "nothing registered yet");
            assert!(matches!(context.receipt(0), Err(ReceiptError::NoRun)));
            context.register_receipts(2).unwrap();
            // The run goes down at the step's end, so it is not there yet.
            assert_eq!(context.receipt_count(), None);
        })
        .unwrap();

    graph
        .enter(producer, |context| {
            assert_eq!(
                context
                    .deliver_scratch(consumer, 0, |writer, _| Active::new(one(writer, 41u32)))
                    .unwrap(),
                Delivered::Outstanding,
                "one slot of two"
            );
            assert_eq!(
                context
                    .deliver_scratch(consumer, 1, |writer, _| Active::new(one(writer, 42u32)))
                    .unwrap(),
                Delivered::Complete
            );
        })
        .unwrap();

    graph
        .enter(consumer, |context| {
            assert_eq!(context.receipt_count(), Some(2));
            assert_eq!(note(context.receipt(0).unwrap()), 41);
            // One-shot: the slot is empty for the second read.
            assert!(matches!(context.receipt(0).unwrap(), Receipt::Empty));
            assert_eq!(note(context.receipt(1).unwrap()), 42);
            assert!(matches!(context.receipt(2), Err(ReceiptError::OutOfRange)));
        })
        .unwrap();

    graph
        .release(producer, ReleaseAbsorption::IntoHolder)
        .unwrap();
    graph
        .release(consumer, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert!(graph.is_empty());
}

#[test]
fn a_cell_parking_on_receipts_alone_starts_every_round_at_the_foot_of_the_bump() {
    let mut graph: Graph = CellGraph::new(2, pin);
    let consumer = graph.create(None).unwrap();
    let producer = graph.create(None).unwrap();

    let mut parked = Vec::new();
    for round in 0..4u32 {
        await_one(&mut graph, consumer);
        parked.push(scratch_in_use(&graph, consumer));
        assert_eq!(
            send(&mut graph, producer, consumer, 0, round),
            Ok(Delivered::Complete)
        );
        graph
            .enter(consumer, |context| {
                assert_eq!(note(context.receipt(0).unwrap()), round);
            })
            .unwrap();
    }
    assert!(parked[0] > 0, "the run itself is bump bytes");
    assert!(
        parked.windows(2).all(|pair| pair[0] == pair[1]),
        "every round's run sits at the foot of a bump handed back whole: {parked:?}"
    );

    graph
        .release(producer, ReleaseAbsorption::IntoHolder)
        .unwrap();
    graph
        .release(consumer, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert!(graph.is_empty());
}

#[test]
fn a_run_at_rest_holds_the_reset_off_and_a_registration_lets_it_through() {
    let mut graph: Graph = CellGraph::new(1, pin);
    let cell = graph.create(None).unwrap();
    await_one(&mut graph, cell);
    let laid = scratch_in_use(&graph, cell);
    assert!(laid > 0);

    // A step that writes scratch and stores nothing leaves both its bytes and the run's: the run
    // at rest names that bump.
    graph
        .enter(cell, |context| {
            context.scratch_writer().fill(8, |index| index as u64);
        })
        .unwrap();
    assert!(scratch_in_use(&graph, cell) > laid);

    // A registration replaces the run, and the reset runs before the replacement goes down.
    await_one(&mut graph, cell);
    assert_eq!(scratch_in_use(&graph, cell), laid);

    graph.release(cell, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

#[test]
fn registering_over_an_undrained_run_is_refused_and_changes_nothing() {
    let mut graph: Graph = CellGraph::new(2, pin);
    let consumer = graph.create(None).unwrap();
    let producer = graph.create(None).unwrap();
    await_one(&mut graph, consumer);
    assert_eq!(
        send(&mut graph, producer, consumer, 0, 41),
        Ok(Delivered::Complete)
    );

    graph
        .enter(consumer, |context| {
            assert_eq!(context.register_receipts(4), Err(RegisterError::Undrained));
            // The run at rest is exactly as it was, and drains.
            assert_eq!(context.receipt_count(), Some(1));
            assert_eq!(note(context.receipt(0).unwrap()), 41);
            // Drained, so the registration lands.
            context.register_receipts(4).unwrap();
        })
        .unwrap();
    graph
        .enter(consumer, |context| {
            assert_eq!(context.receipt_count(), Some(4))
        })
        .unwrap();

    graph
        .release(producer, ReleaseAbsorption::IntoHolder)
        .unwrap();
    graph
        .release(consumer, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert!(graph.is_empty());
}

#[test]
fn a_note_outlives_the_producer_that_wrote_it() {
    let mut graph: Graph = CellGraph::new(2, pin);
    let consumer = graph.create(None).unwrap();
    let producer = graph.create(None).unwrap();
    await_one(&mut graph, consumer);
    assert_eq!(
        send(&mut graph, producer, consumer, 0, 41),
        Ok(Delivered::Complete)
    );

    // The note lives in the consumer's own bump, so nothing of the producer's holds it up.
    graph
        .release(producer, ReleaseAbsorption::IntoHolder)
        .unwrap();
    graph
        .enter(consumer, |context| {
            assert_eq!(note(context.receipt(0).unwrap()), 41)
        })
        .unwrap();

    graph
        .release(consumer, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert!(graph.is_empty());
}

#[test]
fn a_producer_places_into_its_consumer_files_the_carrier_and_dies() {
    let mut graph: Graph = CellGraph::new(2, pin);
    let consumer = graph.create(None).unwrap();
    let producer = graph.create(None).unwrap();
    await_one(&mut graph, consumer);

    graph
        .enter(producer, |context| {
            let value = number_here(context, 7);
            let placed = context
                .alloc_into::<Number, Number>(
                    consumer,
                    &[operand_at(&value, usize::MAX)],
                    |_, views| Active::new(pinned(&views[0])),
                )
                .unwrap();
            let carrier = context.keep(placed);
            assert_eq!(
                context.deliver_carrier(consumer, 0, carrier),
                Ok(Delivered::Complete)
            );
        })
        .unwrap();

    // The consumer holds the producer, so its death seals rather than reclaims and the borrow the
    // placed value embeds stays good.
    graph.release(producer, ReleaseAbsorption::Refused).unwrap();
    graph
        .enter(consumer, |context| match context.receipt(0).unwrap() {
            Receipt::Carrier(Ok(carrier)) => assert_eq!(*context.read(&carrier).value(), 7),
            other => panic!("the producer filed a carrier: {}", kind(&other)),
        })
        .unwrap();

    graph
        .release(consumer, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert!(graph.is_empty());
}

#[test]
fn a_carrier_homed_in_a_reclaimed_producer_redeems_as_gone() {
    let mut graph: Graph = CellGraph::new(2, pin);
    let consumer = graph.create(None).unwrap();
    let producer = graph.create(None).unwrap();
    await_one(&mut graph, consumer);

    graph
        .enter(producer, |context| {
            // Homed in the producer itself, so nothing the consumer holds covers it.
            let value = number_here(context, 7);
            let carrier = context.keep(value);
            context.deliver_carrier(consumer, 0, carrier).unwrap();
        })
        .unwrap();
    graph
        .release(producer, ReleaseAbsorption::IntoHolder)
        .unwrap();

    graph
        .enter(consumer, |context| match context.receipt(0).unwrap() {
            Receipt::Carrier(Err(refusal)) => assert_eq!(refusal, RedeemError::Gone),
            other => panic!("a reclaimed producer leaves a refusal: {}", kind(&other)),
        })
        .unwrap();

    graph
        .release(consumer, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert!(graph.is_empty());
}

#[test]
fn two_producers_fill_one_run_and_the_second_completes_it() {
    let mut graph: Graph = CellGraph::new(3, pin);
    let consumer = graph.create(None).unwrap();
    let first = graph.create(None).unwrap();
    let second = graph.create(None).unwrap();
    graph
        .enter(consumer, |context| context.register_receipts(2).unwrap())
        .unwrap();

    assert_eq!(
        send(&mut graph, first, consumer, 0, 41),
        Ok(Delivered::Outstanding)
    );
    assert_eq!(
        send(&mut graph, second, consumer, 1, 42),
        Ok(Delivered::Complete)
    );
    graph
        .enter(consumer, |context| {
            assert_eq!(note(context.receipt(0).unwrap()), 41);
            assert_eq!(note(context.receipt(1).unwrap()), 42);
        })
        .unwrap();

    for cell in [first, second, consumer] {
        graph.release(cell, ReleaseAbsorption::IntoHolder).unwrap();
    }
    assert!(graph.is_empty());
}

#[test]
fn every_delivery_refusal_leaves_the_run_as_it_found_it() {
    let mut graph: Graph = CellGraph::new(4, pin);
    let consumer = graph.create(None).unwrap();
    let producer = graph.create(None).unwrap();
    let silent = graph.create(None).unwrap();
    await_one(&mut graph, consumer);
    assert_eq!(
        send(&mut graph, producer, consumer, 0, 41),
        Ok(Delivered::Complete)
    );
    let filled = scratch_in_use(&graph, consumer);

    // A cell that registered nothing.
    assert_eq!(
        send(&mut graph, producer, silent, 0, 9),
        Err(DeliverError::NoRun)
    );
    // Past the end of the run.
    assert_eq!(
        send(&mut graph, producer, consumer, 1, 9),
        Err(DeliverError::OutOfRange)
    );
    // A slot that already holds a receipt.
    assert_eq!(
        send(&mut graph, producer, consumer, 0, 9),
        Err(DeliverError::Filled)
    );
    // A consumer whose death was declared.
    let stale = graph.create(None).unwrap();
    graph.release(stale, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(matches!(
        send(&mut graph, producer, stale, 0, 9),
        Err(DeliverError::Stale(Stale(CellHandle::Slab(handle)))) if handle == stale
    ));

    assert_eq!(
        scratch_in_use(&graph, consumer),
        filled,
        "a refused delivery writes no byte"
    );
    graph
        .enter(consumer, |context| {
            assert_eq!(
                note(context.receipt(0).unwrap()),
                41,
                "the first fill still stands"
            )
        })
        .unwrap();

    for cell in [producer, silent, consumer] {
        graph.release(cell, ReleaseAbsorption::IntoHolder).unwrap();
    }
    assert!(graph.is_empty());
}

#[test]
fn a_tree_cell_parks_on_a_run_in_its_own_bump() {
    let mut graph: Graph = CellGraph::new(2, pin);
    let root = graph.create(None).unwrap();
    let producer = graph.create(None).unwrap();
    let consumer = graph.create_tree(root, None).unwrap();
    await_one(&mut graph, consumer);
    assert!(scratch_in_use(&graph, consumer) > 0);
    assert_eq!(
        scratch_in_use(&graph, root),
        0,
        "a tree cell's run is in its own bump, not its root's"
    );

    assert_eq!(
        send(&mut graph, producer, consumer, 0, 41),
        Ok(Delivered::Complete)
    );
    graph
        .enter(consumer, |context| {
            assert_eq!(note(context.receipt(0).unwrap()), 41)
        })
        .unwrap();

    graph.release_tree(consumer).unwrap();
    graph
        .release(producer, ReleaseAbsorption::IntoHolder)
        .unwrap();
    graph.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

#[test]
fn a_tenants_run_is_its_own_though_the_bytes_under_it_are_its_hosts() {
    let mut graph: Graph = CellGraph::new(1, pin);
    let host = graph.create(None).unwrap();
    let tenant = graph.create_tenant(host, None).unwrap();
    let home = CellHome::Slab(host.slot());

    await_one(&mut graph, tenant);
    assert!(
        scratch_in_use(&graph, host) > 0,
        "the run went down in the host's bump"
    );
    assert_eq!(graph.cells.tenancy(home).scratch_tenants, 1);

    // The host has none of its own, and its step does not hand the bump back under the tenant's.
    let held = scratch_in_use(&graph, host);
    graph
        .enter(host, |context| assert_eq!(context.receipt_count(), None))
        .unwrap();
    assert_eq!(scratch_in_use(&graph, host), held);

    // The host fills the tenant's run: the handle names the tenant's slots and the host's bytes.
    graph
        .enter(host, |context| {
            assert_eq!(
                context.deliver_scratch(tenant, 0, |writer, _| Active::new(one(writer, 41u32))),
                Ok(Delivered::Complete)
            );
        })
        .unwrap();
    graph
        .enter(tenant, |context| {
            assert_eq!(note(context.receipt(0).unwrap()), 41)
        })
        .unwrap();

    // The run is still at rest, so the host's bump waits on it until the tenant leaves.
    assert_eq!(graph.cells.tenancy(home).scratch_tenants, 1);
    graph.release_tenant(tenant).unwrap();
    assert_eq!(graph.cells.tenancy(home).scratch_tenants, 0);
    graph.enter(host, |_| ()).unwrap();
    assert_eq!(scratch_in_use(&graph, host), 0);

    graph.release(host, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

#[test]
fn a_bump_two_tenants_name_goes_back_at_the_first_step_end_that_finds_both_gone() {
    let mut graph: Graph = CellGraph::new(1, pin);
    let host = graph.create(None).unwrap();
    let first = graph.create_tenant(host, None).unwrap();
    let second = graph.create_tenant(host, None).unwrap();
    let home = CellHome::Slab(host.slot());

    await_one(&mut graph, first);
    await_one(&mut graph, second);
    assert_eq!(graph.cells.tenancy(home).scratch_tenants, 2);
    let held = scratch_in_use(&graph, host);

    graph.enter(host, |_| ()).unwrap();
    assert_eq!(scratch_in_use(&graph, host), held, "two runs name the bump");
    graph.release_tenant(first).unwrap();
    assert_eq!(graph.cells.tenancy(home).scratch_tenants, 1);
    graph.enter(host, |_| ()).unwrap();
    assert_eq!(scratch_in_use(&graph, host), held, "one run still does");

    graph.release_tenant(second).unwrap();
    assert_eq!(graph.cells.tenancy(home).scratch_tenants, 0);
    graph.enter(host, |_| ()).unwrap();
    assert_eq!(scratch_in_use(&graph, host), 0);

    graph.release(host, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

#[test]
fn a_host_retiring_under_a_tenants_run_takes_the_bump_with_it() {
    let mut graph: Graph = CellGraph::new(1, pin);
    let host = graph.create(None).unwrap();
    let tenant = graph.create_tenant(host, None).unwrap();
    let slot = host.slot();
    await_one(&mut graph, tenant);
    assert!(graph.regions.scratch_in_use(CellHome::Slab(slot)) > 0);

    // The host's disposal waits on its tenant, whose run it is still holding.
    graph.release(host, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.regions.scratch_in_use(CellHome::Slab(slot)) > 0);

    graph.release_tenant(tenant).unwrap();
    assert_eq!(
        graph.regions.scratch_in_use(CellHome::Slab(slot)),
        0,
        "the host's retirement takes whatever its bump still holds"
    );
    assert!(graph.is_empty());
}

#[test]
fn a_panicking_step_leaves_its_registration_for_the_next_step_end() {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    let mut graph: Graph = CellGraph::new(2, pin);
    let consumer = graph.create(None).unwrap();
    let producer = graph.create(None).unwrap();

    let panicked = catch_unwind(AssertUnwindSafe(|| {
        graph
            .enter(consumer, |context| {
                context.register_receipts(1).unwrap();
                panic!("the step fails");
            })
            .unwrap();
    }));
    assert!(panicked.is_err());

    // The tail never ran, so there is no run to file against yet.
    assert_eq!(
        send(&mut graph, producer, consumer, 0, 41),
        Err(DeliverError::NoRun)
    );

    // The cell's next step end honours the registration the panic left pending.
    graph.enter(consumer, |_| ()).unwrap();
    assert_eq!(
        send(&mut graph, producer, consumer, 0, 41),
        Ok(Delivered::Complete)
    );
    graph
        .enter(consumer, |context| {
            assert_eq!(note(context.receipt(0).unwrap()), 41)
        })
        .unwrap();

    graph
        .release(producer, ReleaseAbsorption::IntoHolder)
        .unwrap();
    graph
        .release(consumer, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert!(graph.is_empty());
}

#[test]
fn a_bare_signal_costs_no_bump_byte_beyond_the_run() {
    let mut graph: CellGraph<'static, Storage, Held, Signal> = CellGraph::new(2, pin);
    let consumer = graph.create(None).unwrap();
    let producer = graph.create(None).unwrap();
    graph
        .enter(consumer, |context| context.register_receipts(1).unwrap())
        .unwrap();
    let run = scratch_in_use(&graph, consumer);
    assert!(run > 0);

    graph
        .enter(producer, |context| {
            assert_eq!(
                context.deliver_scratch(consumer, 0, |_, _| Active::new(())),
                Ok(Delivered::Complete)
            );
        })
        .unwrap();
    assert_eq!(
        scratch_in_use(&graph, consumer),
        run,
        "a build that allocates nothing costs nothing beyond the run"
    );
    graph
        .enter(consumer, |context| {
            assert!(matches!(context.receipt(0).unwrap(), Receipt::Value(())))
        })
        .unwrap();

    graph
        .release(producer, ReleaseAbsorption::IntoHolder)
        .unwrap();
    graph
        .release(consumer, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert!(graph.is_empty());
}

/// Which arm a receipt came back on, for a failure message: a `Receipt` holds a family's form and
/// nothing bounds it `Debug`.
fn kind<D: Delivery<'static>>(receipt: &Receipt<'static, '_, '_, D, 1>) -> &'static str {
    match receipt {
        Receipt::Empty => "an empty slot",
        Receipt::Value(_) => "a note",
        Receipt::Carrier(Ok(_)) => "a redeemed carrier",
        Receipt::Carrier(Err(_)) => "a refused carrier",
    }
}
