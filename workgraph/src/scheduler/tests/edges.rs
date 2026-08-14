//! White-box slate for the **edge slab** and the install door: which branch install takes against a
//! pre-terminal or a delivered producer, what it records about the destination, the slab's
//! alloc/release recycling with its debug generation stamps, and the splice's re-point.
//!
//! Runs under Miri alongside the rest of the lib slate, so the raw destination pointer install
//! stores is minted and carried under the same borrow discipline delivery reads it under. The
//! fixtures come from the parent module; the delivery *timelines* live in
//! [`delivery`](super::delivery).

use std::rc::Rc;

use super::super::nodes::NodeWork;
use super::super::{EdgeId, InstalledEdge, NodeId, ResolvedDeps, Scheduler};
use super::*;

/// A scheduler holding one dep-free, pre-terminal node — the pending producer every install test
/// starts from.
fn pending_producer() -> (Scheduler<TestWorkload>, NodeId) {
    let mut sched: Scheduler<TestWorkload> = Scheduler::new();
    let id = alloc_dep_free(&mut sched);
    (sched, id)
}

/// Allocate one dep-free node with a trivial continuation and its own anchor.
fn alloc_dep_free(sched: &mut Scheduler<TestWorkload>) -> NodeId {
    let continuation: Box<dyn FnOnce() -> u32> = Box::new(|| 0);
    sched.alloc_node(
        NodeWork::new(ResolvedDeps::new(), continuation, None),
        &[],
        TestAnchor::fresh(),
        false,
    )
}

/// Drive a node to a terminal: pop it off the ready queue, then deliver an error — an `Err` terminal
/// needs no value, and the walk's edge bookkeeping is the same either way.
fn finalize_with_error(sched: &mut Scheduler<TestWorkload>, id: NodeId) {
    let ready = sched.pop_next().expect("a dep-free slot is ready");
    assert_eq!(ready, id, "the ready slot is the one just installed");
    finalize_in_place(sched, id);
}

/// Terminalize one node without touching the ready queue — the shape a slate holding several
/// dep-free slots needs, where pop order is not the thing under test.
fn finalize_in_place(sched: &mut Scheduler<TestWorkload>, id: NodeId) {
    let (_work, _anchor) = sched.take_for_run(id);
    sched.finalize(id, Err(()));
}

/// A destination owner: an anchor the caller pins for the whole test, standing in for the frame a
/// wiring call names.
fn destination() -> Rc<TestAnchor> {
    TestAnchor::fresh()
}

/// **Install parks on a pre-terminal producer**, recording it as the producer the edge waits on.
#[test]
fn install_parks_on_a_pending_producer() {
    let (mut sched, producer) = pending_producer();
    let destination = destination();

    let edge = sched.install_edge(producer, destination.owner());

    assert_eq!(
        sched.edge_producer(edge),
        Some(producer),
        "a parked edge names the producer it waits on",
    );
}

/// **Install records the destination it was handed**, as the owner's own region — the pointer the
/// delivery deref reads, and the weak shadow that deref is asserted against.
#[test]
fn install_records_the_destination_owner() {
    let (mut sched, producer) = pending_producer();
    let destination = destination();

    let edge = sched.install_edge(producer, destination.owner());

    assert!(
        sched.edge_destination_is(edge, destination.owner()),
        "the edge's destination is the region of the owner install was handed",
    );
    #[cfg(debug_assertions)]
    assert!(
        sched
            .edge_destination_owner(edge)
            .is_some_and(|owner| Rc::ptr_eq(&owner, destination.owner())),
        "the debug shadow upgrades to the destination owner while it stands",
    );
    // The shadow is weak by construction: dropping the caller's hold kills the destination outright
    // rather than the edge silently keeping it alive.
    #[cfg(debug_assertions)]
    {
        drop(destination);
        assert!(
            sched.edge_destination_owner(edge).is_none(),
            "the edge holds no strong claim on its destination",
        );
    }
}

/// **Wiring from a source inherits that source's destination** — a consumer parking on a
/// placeholder's edge delivers into the region the *placeholder* named, not one of its own. The
/// derived edge is a second name on the same producer.
#[test]
fn install_edge_from_inherits_the_destination() {
    let (mut sched, producer) = pending_producer();
    let destination = destination();
    let source = sched.install_edge(producer, destination.owner());

    let derived = sched.install_edge_from(source);

    assert!(
        matches!(derived, InstalledEdge::Parked(_)),
        "the source's producer is still pre-terminal, so the derived edge parks too",
    );
    assert_ne!(derived.edge_id(), source, "wiring mints its own edge");
    assert_eq!(
        sched.edge_producer(derived.edge_id()),
        Some(producer),
        "the derived edge waits on the source's producer",
    );
    assert!(
        sched.edge_destination_is(derived.edge_id(), destination.owner()),
        "the derived edge lands in the region the source named",
    );
}

/// **Wiring from a delivered source fills** — the late-wiring branch, taken whenever the producer
/// delivered before the consumer wired in. There is no slot behind it: the producer reclaimed at its
/// own finalize, and the resident the new edge carries is read off the source.
#[test]
fn install_edge_from_fills_on_a_delivered_source() {
    let (mut sched, producer) = pending_producer();
    let destination = destination();
    let source = sched.install_edge(producer, destination.owner());
    finalize_with_error(&mut sched, producer);

    let derived = sched.install_edge_from(source);

    assert!(
        matches!(derived, InstalledEdge::Filled(_)),
        "the producer delivered between the two installs, so the second fills",
    );
    assert_eq!(
        sched.edge_producer(derived.edge_id()),
        None,
        "a delivered edge is past parking, so it waits on no producer",
    );
    assert!(
        sched.edge_result_error(derived.edge_id()).is_err(),
        "the derived edge carries the terminal the source received",
    );
}

/// **The door reports filled-or-parked per park and wires each accordingly**: a pre-terminal
/// producer takes a notify entry and counts against the consumer's pending, a delivered one takes
/// neither — its resident is already on the minted edge. The realized list is index-aligned with the
/// sources handed in.
#[test]
fn install_deps_parks_and_fills() {
    let mut sched: Scheduler<TestWorkload> = Scheduler::new();
    let pending = alloc_dep_free(&mut sched);
    let delivered = alloc_dep_free(&mut sched);
    let consumer = alloc_dep_free(&mut sched);
    let destination = destination();
    let on_pending = sched.install_edge(pending, destination.owner());
    let on_delivered = sched.install_edge(delivered, destination.owner());
    finalize_in_place(&mut sched, delivered);

    let (resolved, installed) = sched.install_deps(consumer, &[on_pending, on_delivered], &[]);

    assert!(
        matches!(installed[0], InstalledEdge::Parked(_)),
        "the pre-terminal producer's park waits",
    );
    assert!(
        matches!(installed[1], InstalledEdge::Filled(_)),
        "the delivered producer's park is already satisfied",
    );
    assert_eq!(
        resolved.parks().len(),
        2,
        "the realized parks line up with the sources, one entry each",
    );
    assert_eq!(
        sched.pending_count(consumer),
        1,
        "only the unfilled edge counts against the consumer's pending",
    );
}

/// **An owned dep's edge is destined at the consumer's own region.** The embedder names no
/// destination for sub-work it spawned — the door reads the consumer's anchor, so a sub-result lands
/// exactly where its consumer will read it.
#[test]
fn install_deps_destines_owned_edges_at_the_consumer() {
    let mut sched: Scheduler<TestWorkload> = Scheduler::new();
    let sub = alloc_dep_free(&mut sched);
    let consumer_anchor = TestAnchor::fresh();
    let continuation: Box<dyn FnOnce() -> u32> = Box::new(|| 0);
    let consumer = sched.alloc_node(
        NodeWork::new(ResolvedDeps::new(), continuation, None),
        &[sub],
        Rc::clone(&consumer_anchor),
        false,
    );

    assert_eq!(
        sched.pending_count(consumer),
        1,
        "the owned dep's edge is unfilled, so the consumer waits on it",
    );
    let owned_edge = *sched
        .stored_deps(consumer)
        .owned()
        .first()
        .expect("the door recorded the owned dep's edge");
    assert!(
        sched.edge_destination_is(owned_edge, consumer_anchor.owner()),
        "an owned dep delivers into its consumer's own region",
    );
    assert_eq!(sched.edge_producer(owned_edge), Some(sub));
}

/// **A released listed edge withholds its index until the walk drops it** (Inv-C), then recycles.
/// The withholding is correctness, not hygiene: generation stamps are debug-only, so a release-build
/// walk meeting a recycled index would deliver into a stranger's edge.
#[test]
fn a_released_parked_edge_recycles_at_the_walk() {
    let (mut sched, producer) = pending_producer();
    let destination = destination();
    let edge = sched.install_edge(producer, destination.owner());

    sched.release_edge(edge);
    assert_eq!(
        sched.edge_free_list_len(),
        0,
        "the producer's notify list still names it, so the index is withheld",
    );

    finalize_with_error(&mut sched, producer);
    assert_eq!(
        sched.edge_free_list_len(),
        1,
        "the walk dropped the entry, so the index returns to circulation",
    );
}

/// **A delivered edge recycles at once**: the walk already dropped it from every notify list, so
/// nothing names it and there is no reason to withhold the index.
#[test]
fn a_released_delivered_edge_recycles_at_once() {
    let (mut sched, producer) = pending_producer();
    let destination = destination();
    let edge = sched.install_edge(producer, destination.owner());
    finalize_with_error(&mut sched, producer);

    sched.release_edge(edge);

    assert_eq!(
        sched.edge_free_list_len(),
        1,
        "a delivered edge is on no list, so its index frees immediately",
    );
}

/// **Released indices recycle**, and the name that held one goes stale with it: the recycled edge
/// takes the freed index but compares unequal to the released name.
#[test]
fn released_edges_recycle_through_the_free_list() {
    let (mut sched, producer) = pending_producer();
    let destination = destination();

    let first = sched.install_edge(producer, destination.owner());
    let second = sched.install_edge(producer, destination.owner());
    assert_eq!(sched.edge_slab_len(), 2, "two edges, two indices");

    // Deliver first, so `first` is unlisted and its release recycles at once.
    finalize_with_error(&mut sched, producer);
    sched.release_edge(first);
    assert_eq!(sched.edge_free_list_len(), 1, "the released index is free");

    let third = sched.install_edge_from(second).edge_id();
    assert_eq!(
        sched.edge_free_list_len(),
        0,
        "install drains the free list before extending",
    );
    assert_eq!(
        sched.edge_slab_len(),
        2,
        "the recycled install reuses the freed index",
    );
    assert_ne!(third, second, "the recycled edge is not the standing one");
    #[cfg(debug_assertions)]
    assert_ne!(
        third, first,
        "the recycled edge is a new name — the released one's generation is stale",
    );
}

/// **A stale name is loud**: the generation stamp catches a name outliving its index rather than
/// silently renaming whatever recycled it.
#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "stale EdgeId")]
fn a_stale_edge_id_is_loud() {
    let (mut sched, producer) = pending_producer();
    let destination = destination();

    let edge = sched.install_edge(producer, destination.owner());
    finalize_with_error(&mut sched, producer);
    sched.release_edge(edge);
    sched.release_edge(edge);
}

/// **The splice re-points a slot's parked edges once and reclaims the slot.** No alias survives as a
/// residual: the edges name the real producer from here on, and its fire delivers to them directly.
#[test]
fn splice_repoints_parked_edges_and_reclaims_the_slot() {
    let mut sched: Scheduler<TestWorkload> = Scheduler::new();
    let forwarder = alloc_dep_free(&mut sched);
    let real = alloc_dep_free(&mut sched);
    let destination = destination();
    let edge = sched.install_edge(forwarder, destination.owner());

    let (_work, _anchor) = sched.take_for_run(forwarder);
    sched.splice_forward(forwarder, real);

    assert_eq!(
        sched.edge_producer(edge),
        Some(real),
        "the parked edge waits on the real producer, not the retired slot",
    );
    assert!(
        sched.edge_destination_is(edge, destination.owner()),
        "the re-point leaves the destination alone",
    );

    finalize_in_place(&mut sched, real);
    assert!(
        sched.edge_result_error(edge).is_err(),
        "the real producer's fire delivers into the re-pointed edge",
    );
}

/// **A released entry on a spliced slot's list recycles at the splice**, the other half of Inv-C: the
/// splice is the second place a notify list is dropped, so it must return indices exactly as the
/// walk does.
#[test]
fn splice_recycles_a_released_entry() {
    let mut sched: Scheduler<TestWorkload> = Scheduler::new();
    let forwarder = alloc_dep_free(&mut sched);
    let real = alloc_dep_free(&mut sched);
    let destination = destination();
    let doomed: EdgeId = sched.install_edge(forwarder, destination.owner());

    sched.release_edge(doomed);
    assert_eq!(sched.edge_free_list_len(), 0, "withheld while listed");

    let (_work, _anchor) = sched.take_for_run(forwarder);
    sched.splice_forward(forwarder, real);

    assert_eq!(
        sched.edge_free_list_len(),
        1,
        "the splice dropped the entry, so the index returns to circulation",
    );
}
