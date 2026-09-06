//! The sealed tier: the transition a still-reached cell takes at its death, the hybrid mask that
//! carries its id, the accessor that derives reach from its frozen aggregate, and the cascade that
//! retires it. See [design/liveness-matrix.md](../../../design/liveness-matrix.md) § The sealed
//! tier.

use super::super::*;
use super::{Borrowed, Number, Owned, continuation_reach, operand, pin, pinned};

/// Two resident-value counts far enough apart that a transition proportional to storage could not
/// produce the same work for both. The Miri run takes the smaller pair — the shapes are what it
/// checks, and the native run already covers the breadth.
const SMALL: usize = 16;
const LARGE: usize = if cfg!(miri) { 512 } else { 10_000 };

/// Seal a held cell and report the maintenance the transition performed.
///
/// The producer stores `stored` values in its region and puts `kept_by_producer` more of them to
/// rest as residents; the holder puts `kept_by_holder` values of its own to rest. The three knobs
/// are the three quantities the transition could plausibly be proportional to, and only one of
/// them may be.
fn seal_work_for(stored: usize, kept_by_holder: usize, kept_by_producer: usize) -> u64 {
    let mut table: CellTable<Owned> = CellTable::new(4, pin);
    let holder = table.create(None, None).unwrap();
    let producer = table.create(None, None).unwrap();

    table
        .enter(producer, |context| {
            for value in 0..stored {
                context.alloc::<Number>(|writer| writer.value(value as u32));
            }
            for value in 0..kept_by_producer {
                let carrier = context.alloc::<Number>(|writer| writer.value(value as u32));
                context.keep(carrier);
            }
        })
        .unwrap();
    table
        .enter(holder, |context| {
            for value in 0..kept_by_holder {
                let carrier = context.alloc::<Number>(|writer| writer.value(value as u32));
                context.keep(carrier);
            }
            context.hold(producer)
        })
        .unwrap()
        .unwrap();

    let before = table.seal_work;
    table.release(producer, Absorption::Refused).unwrap();
    assert_eq!(table.sealed.len(), 1);
    table.seal_work - before
}

#[test]
fn the_seal_transition_is_bounded_by_the_holders_residents_not_the_storage() {
    // Atomicity is exactly this: the aggregate is a word copy out of the matrix, so a region with
    // ten thousand resident values costs what one with sixteen costs.
    assert_eq!(seal_work_for(SMALL, 4, 0), seal_work_for(LARGE, 4, 0));

    // What the transition *is* proportional to: each holder's resident table, one entry at a
    // time, because the dying slot's bit has to become the record's id in every mask that names
    // it. Four more entries in the one holder's table, four more units of work — exactly.
    assert_eq!(
        seal_work_for(SMALL, 8, 0) - seal_work_for(SMALL, 4, 0),
        4,
        "the rewrite is bounded by the holders' resident counts"
    );

    // And not to the dying cell's own residents: those masks are dead bytes the moment the
    // storage they name is in the record, so the transition forwards one lineage entry for the
    // whole table rather than touching an entry per value.
    assert_eq!(
        seal_work_for(SMALL, 4, SMALL),
        seal_work_for(SMALL, 4, LARGE)
    );
}

#[test]
fn a_handle_is_stale_once_its_cell_seals_and_the_slot_takes_a_new_occupant() {
    let mut table: CellTable<Owned> = CellTable::new(2, pin);
    let holder = table.create(None, None).unwrap();
    let held = table.create(None, None).unwrap();

    table
        .enter(holder, |context| context.hold(held))
        .unwrap()
        .unwrap();
    table.release(held, Absorption::Refused).unwrap();

    assert!(!table.is_live(held));
    assert_eq!(
        table.enter(held, |_| ()),
        Err(EnterError::Stale(StaleHandle(held)))
    );

    let reused = table.create(None, None).unwrap();
    assert_eq!(reused.slot(), held.slot());
    assert_eq!(reused.generation(), held.generation() + 1);
    // The record outlives the slot: sealed ids come from their own space and are never reused, so
    // the tier needs no generation of its own.
    assert_eq!(table.sealed.len(), 1);
}

#[test]
fn a_stored_mask_trades_the_sealed_slot_for_its_id() {
    let mut table: CellTable<Borrowed> = CellTable::new(4, pin);
    let consumer = table.create(None, None).unwrap();
    let producer = table.create(None, None).unwrap();
    let reached = table.create(None, None).unwrap();

    // The producer's own hold set names `reached`, so that bit is in the aggregate it freezes.
    table
        .enter(producer, |context| context.hold(reached))
        .unwrap()
        .unwrap();

    // The consumer builds a value into the producer's region and keeps it as its continuation, so
    // the consumer holds the producer and its stored mask names the producer's slot.
    table
        .enter(consumer, |context| {
            let value = context
                .alloc_into::<Number, Number>(producer, &[], |writer, _| writer.value(41))
                .unwrap();
            context
                .store_successor_capturing(&[operand(&value)], |_writer, views| pinned(&views[0]));
        })
        .unwrap();
    assert!(table.holds(consumer, producer));

    table.release(producer, Absorption::Refused).unwrap();
    let id = table.sealed.ids().next().unwrap();
    assert!(table.sealed_holds[consumer.slot() as usize].contains(id));

    // The transition rewrote the consumer's stored mask in place: the dying slot's bit traded for
    // the record's id, and what that region reached lives on in the record's frozen aggregate.
    let stored = continuation_reach(&table, consumer);
    assert!(stored.names_sealed(id));
    assert!(!stored.names(producer.slot()));
    assert!(
        table
            .sealed
            .get(id)
            .unwrap()
            .aggregate
            .names(reached.slot())
    );

    // The storage detached unmoved, so the borrow the re-anchor hands back still reads it.
    let value = table
        .enter(consumer, |context| *context.continuation().unwrap().value())
        .unwrap();
    assert_eq!(value, 41);
}

#[test]
fn a_reach_that_names_two_sealed_regions_merges_their_ids_in_order() {
    let mut table: CellTable<Borrowed> = CellTable::new(4, pin);
    let consumer = table.create(None, None).unwrap();
    let first = table.create(None, None).unwrap();
    let second = table.create(None, None).unwrap();

    table
        .enter(consumer, |context| {
            let one = context
                .alloc_into::<Number, Number>(first, &[], |writer, _| writer.value(1))
                .unwrap();
            let two = context
                .alloc_into::<Number, Number>(second, &[], |writer, _| writer.value(2))
                .unwrap();
            context.store_successor_capturing(&[operand(&one), operand(&two)], |_writer, views| {
                pinned(&views[1])
            });
        })
        .unwrap();

    table.release(first, Absorption::Refused).unwrap();
    table.release(second, Absorption::Refused).unwrap();
    let mut minted: Vec<SealedId> = table.sealed.ids().collect();
    minted.sort();
    assert_eq!(minted.len(), 2);

    let named: Vec<SealedId> = continuation_reach(&table, consumer)
        .sealed()
        .iter()
        .collect();
    // The sparse half unions by sorted merge, so the two ids arrive deduplicated and in id order.
    assert_eq!(named, minted);

    let value = table
        .enter(consumer, |context| *context.continuation().unwrap().value())
        .unwrap();
    assert_eq!(value, 2);
}

#[test]
fn reclaiming_a_records_last_holder_cascades_through_its_aggregate() {
    let mut table: CellTable<Owned> = CellTable::new(4, pin);
    let top = table.create(None, None).unwrap();
    let middle = table.create(None, None).unwrap();
    let base = table.create(None, None).unwrap();

    table
        .enter(middle, |context| context.hold(base))
        .unwrap()
        .unwrap();
    // The top cell holds the base as well, so the base's record keeps two holders and the middle's
    // seal has no count-1 region to absorb: what this test is about is the cascade, not a merge.
    table
        .enter(top, |context| {
            context.hold(middle).unwrap();
            context.hold(base)
        })
        .unwrap()
        .unwrap();

    table.release(base, Absorption::Refused).unwrap();
    assert_eq!(table.sealed.len(), 1);
    // The middle cell's hold on the base is a sealed id by now, so its own aggregate carries it.
    table.release(middle, Absorption::Refused).unwrap();
    assert_eq!(table.sealed.len(), 2);

    // One release retires both: the outer count reaches zero, and releasing its aggregate takes
    // the inner count with it.
    table.release(top, Absorption::IntoHolder).unwrap();
    assert_eq!(table.sealed.len(), 0);
    assert_eq!(table.free.len(), 4);
}

#[test]
fn a_record_survives_every_holder_but_the_last() {
    let mut table: CellTable<Owned> = CellTable::new(4, pin);
    let first = table.create(None, None).unwrap();
    let second = table.create(None, None).unwrap();
    let held = table.create(None, None).unwrap();

    for holder in [first, second] {
        table
            .enter(holder, |context| context.hold(held))
            .unwrap()
            .unwrap();
    }
    table.release(held, Absorption::IntoHolder).unwrap();
    let id = table.sealed.ids().next().unwrap();
    assert_eq!(table.sealed.get(id).unwrap().holders, 2);

    table.release(first, Absorption::IntoHolder).unwrap();
    assert_eq!(table.sealed.get(id).unwrap().holders, 1);
    table.release(second, Absorption::IntoHolder).unwrap();
    assert_eq!(table.sealed.len(), 0);
    assert_eq!(table.free.len(), 4);
}

#[test]
fn a_cell_that_only_a_birth_row_names_waits_in_the_slab_rather_than_sealing() {
    let mut table: CellTable<Owned> = CellTable::new(4, pin);
    let parent = table.create(None, None).unwrap();
    let child = table.create(Some(parent), None).unwrap();

    // Birth holds are the one relation with no sealed half: a descendant that can still walk to
    // its parent keeps the parent in place, so nothing seals here.
    table.release(parent, Absorption::IntoHolder).unwrap();
    assert_eq!(super::state_of(&table, parent), SlotState::Dead);
    assert_eq!(table.sealed.len(), 0);

    table.release(child, Absorption::IntoHolder).unwrap();
    assert_eq!(table.sealed.len(), 0);
    assert_eq!(table.free.len(), 4);
}
