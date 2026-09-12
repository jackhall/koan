//! The sealed tier: the transition a still-reached cell takes at its death, the hybrid mask that
//! carries its id, the accessor that derives reach from its frozen aggregate, and the cascade that
//! retires it. See [../README.md](../README.md) § The sealed
//! tier.

use super::super::*;
use super::{Borrowed, Number, Owned, kept_reach, number_here, one, operand, pin, pinned};

/// Two dormant-value counts far enough apart that a transition proportional to storage could not
/// produce the same work for both. The Miri run takes the smaller pair — the shapes are what it
/// checks, and the native run already covers the breadth.
const SMALL: usize = 16;
const LARGE: usize = if cfg!(miri) { 512 } else { 10_000 };

/// Seal a held cell and report the maintenance the transition performed.
///
/// The producer stores `stored` values in its region and puts `kept_by_producer` more of them to
/// rest as dormant carriers; the holder takes `kept_by_holder` **distinct** reach-table entries.
/// The three knobs are the three quantities the transition could plausibly be proportional to, and
/// only one of them may be.
///
/// The holder's entries have to be distinct because the reach table interns on content: a dormant
/// carrier is one entry per reach, so counting keeps would count nothing. Each is built into the
/// holder's region from a cell of its own, capturing a value that cell homes, so its reach names
/// the holder and that one cell and matches no other.
fn seal_work_for(stored: usize, kept_by_holder: usize, kept_by_producer: usize) -> u64 {
    let cap = 4 + kept_by_holder as u32;
    let mut graph: CellGraph<Owned> = CellGraph::new(cap, pin);
    let holder = graph.create(None, None).unwrap();
    let producer = graph.create(None, None).unwrap();

    graph
        .enter(producer, |context| {
            for value in 0..stored {
                number_here(context, value as u32);
            }
            for value in 0..kept_by_producer {
                let carrier = number_here(context, value as u32);
                context.keep(carrier);
            }
        })
        .unwrap();

    for value in 0..kept_by_holder {
        let source = graph.create(None, None).unwrap();
        graph
            .enter(source, |context| {
                let local = number_here(context, value as u32);
                let carrier = context
                    .alloc_into::<Number, Number>(holder, &[operand(&local)], |writer, views| {
                        one(writer, *pinned(&views[0]))
                    })
                    .unwrap();
                context.keep(carrier);
            })
            .unwrap();
    }
    assert_eq!(
        graph.slots[holder.slot() as usize].reaches.len() as usize,
        kept_by_holder
    );

    graph
        .enter(holder, |context| context.hold(producer))
        .unwrap()
        .unwrap();

    let before = graph.seal_work;
    graph.release(producer, ReleaseAbsorption::Refused).unwrap();
    assert_eq!(graph.sealed.len(), 1);
    graph.seal_work - before
}

#[test]
fn the_seal_transition_is_bounded_by_the_holders_dormant_carriers_not_the_storage() {
    // Atomicity is exactly this: the aggregate is a word copy out of the matrix, so a region with
    // ten thousand dormant values costs what one with sixteen costs.
    assert_eq!(seal_work_for(SMALL, 4, 0), seal_work_for(LARGE, 4, 0));

    // What the transition *is* proportional to: each holder's reach table, one entry at a time,
    // because the dying slot's bit has to become the sealed cell's id in every mask that names it.
    // Four more entries in the one holder's reach table, four more units of work — exactly. The
    // count is entries, not keeps: interning is what keeps the two from diverging over a run.
    assert_eq!(
        seal_work_for(SMALL, 8, 0) - seal_work_for(SMALL, 4, 0),
        4,
        "the rewrite is bounded by the holders' entry counts"
    );

    // And not to the dying cell's own dormant carriers: those masks are dead bytes the moment the
    // storage they name is in the sealed cell, so the transition forwards one lineage entry for the
    // whole reach table rather than touching an entry per value. The producer's keeps all share
    // one reach and so one entry, which is the point twice over — the reach table did not grow
    // either.
    assert_eq!(
        seal_work_for(SMALL, 4, SMALL),
        seal_work_for(SMALL, 4, LARGE)
    );
}

#[test]
fn a_handle_is_stale_once_its_cell_seals_and_the_slot_takes_a_new_occupant() {
    let mut graph: CellGraph<Owned> = CellGraph::new(2, pin);
    let holder = graph.create(None, None).unwrap();
    let held = graph.create(None, None).unwrap();

    graph
        .enter(holder, |context| context.hold(held))
        .unwrap()
        .unwrap();
    graph.release(held, ReleaseAbsorption::Refused).unwrap();

    assert!(!graph.is_live(held));
    assert_eq!(
        graph.enter(held, |_| ()),
        Err(EnterError::Stale(Stale(CellHandle::Slab(held))))
    );

    let reused = graph.create(None, None).unwrap();
    assert_eq!(reused.slot(), held.slot());
    assert_eq!(reused.generation(), held.generation() + 1);
    // The sealed cell outlives the slot: sealed ids come from their own space and are never reused,
    // so the tier needs no generation of its own.
    assert_eq!(graph.sealed.len(), 1);
}

#[test]
fn a_stored_mask_trades_the_sealed_slot_for_its_id() {
    let mut graph: CellGraph<Borrowed> = CellGraph::new(4, pin);
    let consumer = graph.create(None, None).unwrap();
    let producer = graph.create(None, None).unwrap();
    let reached = graph.create(None, None).unwrap();

    // The producer's own hold set names `reached`, so that bit is in the aggregate it freezes.
    graph
        .enter(producer, |context| context.hold(reached))
        .unwrap()
        .unwrap();

    // The consumer builds a value into the producer's region and pins it into its own holds as
    // its continuation's capture, so the consumer holds the producer. It also bundles the same
    // value into its own region and keeps it, which is the stored mask the transition rewrites.
    let kept = graph
        .enter(consumer, |context| {
            let value = context
                .alloc_into::<Number, Number>(producer, &[], |writer, _| one(writer, 41))
                .unwrap();
            let captured =
                context.alloc_here(&[operand(&value)], |_writer, views| pinned(&views[0]));
            context.store_successor(captured);
            let bundled = context
                .alloc_into::<Number, Number>(consumer, &[operand(&value)], |_writer, views| {
                    pinned(&views[0])
                })
                .unwrap();
            context.keep(bundled)
        })
        .unwrap();
    assert!(graph.holds(consumer, producer));

    graph.release(producer, ReleaseAbsorption::Refused).unwrap();
    let id = graph.sealed.ids().next().unwrap();
    assert!(graph.sealed_holds[consumer.slot() as usize].contains(id));

    // The transition rewrote the consumer's stored mask in place: the dying slot's bit traded for
    // the sealed cell's id, and what that region reached lives on in the sealed cell's frozen
    // aggregate.
    let stored = kept_reach(&graph, &kept);
    assert!(stored.names_sealed(id));
    assert!(!stored.names(producer.slot()));
    assert!(
        graph
            .sealed
            .get(id)
            .unwrap()
            .aggregate
            .names(reached.slot())
    );

    // The storage detached unmoved, so the borrow the re-anchor hands back still reads it.
    let value = graph
        .enter(consumer, |context| *context.continuation().unwrap())
        .unwrap();
    assert_eq!(value, 41);
}

#[test]
fn a_reach_that_names_two_sealed_regions_merges_their_ids_in_order() {
    let mut graph: CellGraph<Borrowed> = CellGraph::new(4, pin);
    let consumer = graph.create(None, None).unwrap();
    let first = graph.create(None, None).unwrap();
    let second = graph.create(None, None).unwrap();

    let kept = graph
        .enter(consumer, |context| {
            let from_first = context
                .alloc_into::<Number, Number>(first, &[], |writer, _| one(writer, 1))
                .unwrap();
            let from_second = context
                .alloc_into::<Number, Number>(second, &[], |writer, _| one(writer, 2))
                .unwrap();
            let captured = context.alloc_here(
                &[operand(&from_first), operand(&from_second)],
                |_writer, views| pinned(&views[1]),
            );
            context.store_successor(captured);
            let bundled = context
                .alloc_into::<Number, Number>(
                    consumer,
                    &[operand(&from_first), operand(&from_second)],
                    |_writer, views| pinned(&views[1]),
                )
                .unwrap();
            context.keep(bundled)
        })
        .unwrap();

    graph.release(first, ReleaseAbsorption::Refused).unwrap();
    graph.release(second, ReleaseAbsorption::Refused).unwrap();
    let mut minted: Vec<SealedId> = graph.sealed.ids().collect();
    minted.sort();
    assert_eq!(minted.len(), 2);

    let named: Vec<SealedId> = kept_reach(&graph, &kept).sealed().iter().collect();
    // The sparse half unions by sorted merge, so the two ids arrive deduplicated and in id order.
    assert_eq!(named, minted);

    let value = graph
        .enter(consumer, |context| *context.continuation().unwrap())
        .unwrap();
    assert_eq!(value, 2);
}

#[test]
fn reclaiming_a_sealed_cells_last_holder_cascades_through_its_aggregate() {
    let mut graph: CellGraph<Owned> = CellGraph::new(4, pin);
    let top = graph.create(None, None).unwrap();
    let middle = graph.create(None, None).unwrap();
    let base = graph.create(None, None).unwrap();

    graph
        .enter(middle, |context| context.hold(base))
        .unwrap()
        .unwrap();
    // The top cell holds the base as well, so the base's sealed cell keeps two holders and the
    // middle's seal has no count-1 region to absorb: what this test is about is the cascade, not a
    // merge.
    graph
        .enter(top, |context| {
            context.hold(middle).unwrap();
            context.hold(base)
        })
        .unwrap()
        .unwrap();

    graph.release(base, ReleaseAbsorption::Refused).unwrap();
    assert_eq!(graph.sealed.len(), 1);
    // The middle cell's hold on the base is a sealed id by now, so its own aggregate carries it.
    graph.release(middle, ReleaseAbsorption::Refused).unwrap();
    assert_eq!(graph.sealed.len(), 2);

    // One release retires both: the outer count reaches zero, and releasing its aggregate takes
    // the inner count with it.
    graph.release(top, ReleaseAbsorption::IntoHolder).unwrap();
    assert_eq!(graph.sealed.len(), 0);
    assert_eq!(graph.free.len(), 4);
}

#[test]
fn a_sealed_cell_survives_every_holder_but_the_last() {
    let mut graph: CellGraph<Owned> = CellGraph::new(4, pin);
    let first = graph.create(None, None).unwrap();
    let second = graph.create(None, None).unwrap();
    let held = graph.create(None, None).unwrap();

    for holder in [first, second] {
        graph
            .enter(holder, |context| context.hold(held))
            .unwrap()
            .unwrap();
    }
    graph.release(held, ReleaseAbsorption::IntoHolder).unwrap();
    let id = graph.sealed.ids().next().unwrap();
    assert_eq!(graph.sealed.get(id).unwrap().holders, 2);

    graph.release(first, ReleaseAbsorption::IntoHolder).unwrap();
    assert_eq!(graph.sealed.get(id).unwrap().holders, 1);
    graph
        .release(second, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert_eq!(graph.sealed.len(), 0);
    assert_eq!(graph.free.len(), 4);
}

#[test]
fn a_cell_that_only_a_birth_row_names_waits_in_the_slab_rather_than_sealing() {
    let mut graph: CellGraph<Owned> = CellGraph::new(4, pin);
    let parent = graph.create(None, None).unwrap();
    let child = graph.create(Some(parent), None).unwrap();

    // Birth holds are the one relation with no sealed half: a descendant that can still walk to
    // its parent keeps the parent in place, so nothing seals here.
    graph
        .release(parent, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert_eq!(super::state_of(&graph, parent), SlabState::Dead);
    assert_eq!(graph.sealed.len(), 0);

    graph.release(child, ReleaseAbsorption::IntoHolder).unwrap();
    assert_eq!(graph.sealed.len(), 0);
    assert_eq!(graph.free.len(), 4);
}
