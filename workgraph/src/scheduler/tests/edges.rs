//! White-box slate for the **edge slab** and the install door: which branch install takes against a
//! pending, finalized, or aliased producer, what it records about the destination, and the slab's
//! alloc/release recycling with its debug generation stamps.
//!
//! Runs under Miri alongside the rest of the lib slate, so the raw destination pointer install
//! stores is minted and carried under the same borrow discipline delivery will read it under. The
//! fixtures come from the parent module; `OwnerOf<TestWorkload>` is `Cart`, so a destination owner
//! is just an `Rc<Cart>`.

use std::rc::Rc;

use super::super::nodes::NodeWork;
use super::super::{DepEdge, InstalledEdge, NodeId, ResolvedDeps, Scheduler};
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
    let anchor = Rc::new(TestAnchor(Rc::new(Cart(vec![1, 2, 3]))));
    let continuation: Box<dyn FnOnce() -> u32> = Box::new(|| 0);
    sched.alloc_node(
        NodeWork::new(ResolvedDeps::new(), continuation, None),
        anchor,
        false,
    )
}

/// Drive a node to a terminal: pop it off the ready queue, take its work, and finalize it with an
/// error — an `Err` terminal needs no carrier, and readiness is `Done(..)` either way.
fn finalize_with_error(sched: &mut Scheduler<TestWorkload>, id: NodeId) {
    let ready = sched.pop_next().expect("a dep-free slot is ready");
    assert_eq!(ready, id, "the ready slot is the one just installed");
    finalize_in_place(sched, id);
}

/// Terminalize one node without touching the ready queue — the shape a slate holding several
/// dep-free slots needs, where pop order is not the thing under test.
fn finalize_in_place(sched: &mut Scheduler<TestWorkload>, id: NodeId) {
    let (_work, _anchor, _handoff) = sched.take_for_run(id);
    sched.finalize(id, Err(()));
}

/// A destination owner: a cart the caller pins for the whole test, standing in for the frame a
/// wiring call names.
fn destination() -> Rc<Cart> {
    Rc::new(Cart(vec![7, 8, 9]))
}

/// **Install parks on a pre-terminal producer**, recording it as the producer the edge waits on.
#[test]
fn install_parks_on_a_pending_producer() {
    let (mut sched, producer) = pending_producer();
    let destination = destination();

    let installed = sched.install_edge(producer, &destination);

    assert!(
        matches!(installed, InstalledEdge::Parked(_)),
        "a pre-terminal producer parks its edge",
    );
    assert_eq!(
        sched.edge_producer(installed.edge_id()),
        Some(producer),
        "a parked edge names the producer it waits on",
    );
}

/// **Install records the destination it was handed**, as the owner's own region — the pointer the
/// delivery deref will read, and the weak shadow that check is asserted against.
#[test]
fn install_records_the_destination_owner() {
    let (mut sched, producer) = pending_producer();
    let destination = destination();

    let edge = sched.install_edge(producer, &destination).edge_id();

    assert!(
        sched.edge_destination_is(edge, &destination),
        "the edge's destination is the region of the owner install was handed",
    );
    #[cfg(debug_assertions)]
    assert!(
        sched
            .edge_destination_owner(edge)
            .is_some_and(|owner| Rc::ptr_eq(&owner, &destination)),
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

/// **Install fills against an already-finalized producer** — the late-wiring branch, taken whenever
/// the producer finalized before the consumer wired in. The consumer reads the value rather than
/// parking on a slot that will not fire again.
#[test]
fn install_fills_on_a_finalized_producer() {
    let (mut sched, producer) = pending_producer();
    finalize_with_error(&mut sched, producer);
    let destination = destination();

    let installed = sched.install_edge(producer, &destination);

    assert!(
        matches!(installed, InstalledEdge::Filled(_)),
        "a finalized producer fills its edge",
    );
    assert_eq!(
        sched.edge_producer(installed.edge_id()),
        None,
        "a filled edge is past parking, so it waits on no producer",
    );
    assert_eq!(
        sched.edges.producer_through(installed.edge_id()),
        producer,
        "it still records the producer its resident is read through",
    );
}

/// **Wiring from a source inherits that source's destination** — a consumer parking on a
/// placeholder's edge delivers into the region the *placeholder* named, not one of its own. The
/// derived edge is a second name on the same producer.
#[test]
fn install_edge_from_inherits_the_destination() {
    let (mut sched, producer) = pending_producer();
    let destination = destination();
    let source = sched.install_edge(producer, &destination).edge_id();

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
        sched.edge_destination_is(derived.edge_id(), &destination),
        "the derived edge lands in the region the source named",
    );
}

/// **Wiring from a filled source fills too**, and reads through the same producer — the late-wiring
/// branch reached through an edge rather than a producer id.
#[test]
fn install_edge_from_follows_a_filled_source() {
    let (mut sched, producer) = pending_producer();
    let destination = destination();
    let source = sched.install_edge(producer, &destination).edge_id();
    finalize_with_error(&mut sched, producer);

    let derived = sched.install_edge_from(source);

    assert!(
        matches!(derived, InstalledEdge::Filled(_)),
        "the producer finalized between the two installs, so the second fills",
    );
    assert_eq!(
        sched.edges.producer_through(derived.edge_id()),
        producer,
        "a filled derived edge still names the producer it reads through",
    );
}

/// **The door reports filled-or-parked per park and wires each accordingly**: a pre-terminal
/// producer takes a notify edge, an already-finalized one takes none but counts its late pull. The
/// realized list is index-aligned with the sources handed in.
#[test]
fn install_deps_parks_and_fills() {
    let mut sched: Scheduler<TestWorkload> = Scheduler::new();
    let pending = alloc_dep_free(&mut sched);
    let finalized = alloc_dep_free(&mut sched);
    let consumer = alloc_dep_free(&mut sched);
    let destination = destination();
    let on_pending = sched.install_edge(pending, &destination).edge_id();
    let on_finalized = sched.install_edge(finalized, &destination).edge_id();
    finalize_in_place(&mut sched, finalized);

    let (resolved, installed) = sched.install_deps(consumer, &[on_pending, on_finalized], &[]);

    assert!(
        matches!(installed[0], InstalledEdge::Parked(_)),
        "the pre-terminal producer's park waits",
    );
    assert!(
        matches!(installed[1], InstalledEdge::Filled(_)),
        "the finalized producer's park is already satisfied",
    );
    assert_eq!(
        resolved.parks(),
        [pending, finalized],
        "the realized parks line up with the sources, one entry each",
    );
    let edges = sched.dep_edges_at(consumer);
    assert_eq!(
        edges.len(),
        1,
        "only the pre-terminal producer takes a notify edge",
    );
    assert!(matches!(edges[0], DepEdge::Notify(p) if p == pending));
    assert_eq!(
        sched.retained_pulls(finalized),
        Some(1),
        "the filled park counts its late pull on the producer's retention hold",
    );
}

/// **A consumer's park edges die with it** — the owner-side half of *an edge never outlives its
/// owner*, on both the success path (`reclaim_deps`) and the death path (`free`).
#[test]
fn park_edges_release_with_the_consumer() {
    let (mut sched, producer) = pending_producer();
    let destination = destination();
    let source = sched.install_edge(producer, &destination).edge_id();

    let reclaimed = alloc_dep_free(&mut sched);
    sched.install_deps(reclaimed, &[source], &[]);
    assert_eq!(sched.edge_free_list_len(), 0, "the park edge is live");
    sched.reclaim_deps(reclaimed, Vec::new());
    assert_eq!(
        sched.edge_free_list_len(),
        1,
        "the success path releases the slot's park edge",
    );

    let freed = alloc_dep_free(&mut sched);
    sched.install_deps(freed, &[source], &[]);
    assert_eq!(
        sched.edge_free_list_len(),
        0,
        "the next consumer's park recycles the released index",
    );
    finalize_in_place(&mut sched, freed);
    sched.free(freed);
    assert_eq!(
        sched.edge_free_list_len(),
        1,
        "the death path releases it too",
    );
}

/// **Edge-keyed reads follow a bare-name-forward alias**, so an edge wired before its producer was
/// spliced out still reaches the real result.
#[test]
fn edge_reads_follow_the_alias() {
    let mut sched: Scheduler<TestWorkload> = Scheduler::new();
    let alias = alloc_dep_free(&mut sched);
    let real = alloc_dep_free(&mut sched);
    let destination = destination();
    let edge = sched.install_edge(alias, &destination).edge_id();

    sched.splice_forward(alias, real);
    finalize_in_place(&mut sched, real);

    assert!(
        sched.edge_result_error(edge).is_err(),
        "the read follows the alias to the real producer's terminal",
    );
}

/// **Install resolves a bare-name-forward alias**, so an edge wired to a spliced-out slot waits on
/// the real producer rather than the dead alias.
#[test]
fn install_follows_an_alias_to_the_real_producer() {
    let mut sched: Scheduler<TestWorkload> = Scheduler::new();
    let alias = alloc_dep_free(&mut sched);
    let real = alloc_dep_free(&mut sched);
    sched.splice_forward(alias, real);
    let destination = destination();

    let installed = sched.install_edge(alias, &destination);

    assert_eq!(
        sched.edge_producer(installed.edge_id()),
        Some(real),
        "the edge parks on the real producer, not the alias",
    );
}

/// **Released indices recycle**, and the name that held one goes stale with it: the recycled edge
/// takes the freed index but compares unequal to the released name.
#[test]
fn released_edges_recycle_through_the_free_list() {
    let (mut sched, producer) = pending_producer();
    let destination = destination();

    let first = sched.install_edge(producer, &destination).edge_id();
    let second = sched.install_edge(producer, &destination).edge_id();
    assert_eq!(sched.edge_slab_len(), 2, "two edges, two indices");

    sched.release_edge(first);
    assert_eq!(sched.edge_free_list_len(), 1, "the released index is free");

    let third = sched.install_edge(producer, &destination).edge_id();
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

/// **A stale name is loud**: the generation stamp catches a name outliving its edge rather than
/// silently renaming whatever recycled its index.
#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "stale EdgeId")]
fn a_stale_edge_id_is_loud() {
    let (mut sched, producer) = pending_producer();
    let destination = destination();

    let edge = sched.install_edge(producer, &destination).edge_id();
    sched.release_edge(edge);
    sched.release_edge(edge);
}

/// **A parked edge's producer is the scheduler's to rewrite** — the alias splice re-points it
/// without disturbing the destination.
#[test]
fn rewrite_producer_repoints_a_parked_edge() {
    let mut sched: Scheduler<TestWorkload> = Scheduler::new();
    let first = alloc_dep_free(&mut sched);
    let second = alloc_dep_free(&mut sched);
    let destination = destination();

    let edge = sched.install_edge(first, &destination).edge_id();
    sched.rewrite_edge_producer(edge, second);

    assert_eq!(
        sched.edge_producer(edge),
        Some(second),
        "the parked edge waits on its rewritten producer",
    );
    assert!(
        sched.edge_destination_is(edge, &destination),
        "the rewrite leaves the destination alone",
    );
}

/// **A filled edge has no producer to rewrite** — post-fill the producer pointer is meaningless, so
/// the attempt is a panic rather than a silent write.
#[test]
#[should_panic(expected = "only a pre-fill edge has a producer to rewrite")]
fn rewrite_producer_rejects_a_filled_edge() {
    let (mut sched, producer) = pending_producer();
    finalize_with_error(&mut sched, producer);
    let destination = destination();

    let edge = sched.install_edge(producer, &destination).edge_id();
    sched.rewrite_edge_producer(edge, producer);
}
