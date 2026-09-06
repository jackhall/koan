//! Values with reach: the region doors, the mint OR into the pin relation, the reclaim gate over
//! both relations, and the ring detector.

use super::super::*;
use super::{Borrowed, Number, Owned, operand, pin, pinned, state_of};

#[test]
fn a_value_allocated_in_the_executing_cell_reaches_only_that_cell() {
    let mut table: CellTable<Owned> = CellTable::new(4, pin);
    let cell = table.create(None, None).unwrap();

    let read = table
        .enter(cell, |context| {
            let value = context.alloc::<Number>(|writer| writer.value(41));
            assert!(value.reach().names(cell.slot()));
            *context.read(&value).value()
        })
        .unwrap();

    assert_eq!(read, 41);
    // The self rule: a cell that held itself alive could never reach a zero hold count.
    assert!(!table.holds(cell, cell));
    assert!(table.slots[cell.slot() as usize].region.is_some());
}

#[test]
fn a_cell_that_never_allocates_mints_no_region() {
    let mut table: CellTable<Owned> = CellTable::new(2, pin);
    let cell = table.create(None, None).unwrap();
    table.enter(cell, |context| context.handle()).unwrap();
    assert!(table.slots[cell.slot() as usize].region.is_none());
}

#[test]
fn placing_a_value_into_another_cell_mints_that_cell_a_hold_on_its_reach() {
    let mut table: CellTable<Owned> = CellTable::new(4, pin);
    let producer = table.create(None, None).unwrap();
    let consumer = table.create(None, None).unwrap();

    let read = table
        .enter(producer, |context| {
            let value = context.alloc::<Number>(|writer| writer.value(41));
            // The placed value *is* the operand's borrow, so it genuinely reads the producer's
            // storage from the consumer's region.
            let placed = context
                .alloc_into::<Number, Number>(consumer, &[operand(&value)], |_writer, views| {
                    pinned(&views[0])
                })
                .unwrap();
            assert!(placed.reach().names(producer.slot()));
            assert!(placed.reach().names(consumer.slot()));
            *context.read(&placed).value()
        })
        .unwrap();

    assert_eq!(read, 41);
    assert!(table.holds(consumer, producer));
    assert!(!table.holds(consumer, consumer));
    assert!(!table.holds(producer, consumer));
}

#[test]
fn a_held_cell_leaves_the_slab_at_its_death_and_its_record_goes_with_its_holder() {
    let mut table: CellTable<Owned> = CellTable::new(4, pin);
    let holder = table.create(None, None).unwrap();
    let held = table.create(None, None).unwrap();

    table
        .enter(holder, |context| context.hold(held))
        .unwrap()
        .unwrap();
    assert!(table.holds(holder, held));

    // The slot comes straight back: retention lives in the sealed tier, never in the slab.
    table.release(held, Absorption::Refused).unwrap();
    assert_eq!(state_of(&table, held), SlotState::Free);
    assert_eq!(table.sealed.len(), 1);
    let id = table.sealed.ids().next().unwrap();
    assert!(table.sealed_holds[holder.slot() as usize].contains(id));
    assert!(!table.holds(holder, held));

    table.release(holder, Absorption::IntoHolder).unwrap();
    assert_eq!(table.sealed.len(), 0);
    assert_eq!(table.free.len(), 4);
}

/// Chunks enough to spill a fresh `Bump` past the one it starts with, so the growth the borrow
/// survives is a real chunk allocation rather than a bump of the same block's cursor.
const GROWTH: usize = if cfg!(miri) { 256 } else { 4096 };

#[test]
fn a_reattached_borrow_survives_the_live_region_it_names_growing_under_it() {
    let mut table: CellTable<Borrowed> = CellTable::new(4, pin);
    let keeper = table.create(None, None).unwrap();
    let host = table.create(None, None).unwrap();

    // The continuation borrows into a cell that stays live, so nothing detaches: the referent is
    // chunks the host still owns, and the host keeps allocating into them.
    table
        .enter(keeper, |context| {
            let value = context
                .alloc_into::<Number, Number>(host, &[], |writer, _| writer.value(41))
                .unwrap();
            context
                .store_successor_capturing(&[operand(&value)], |_writer, views| pinned(&views[0]));
        })
        .unwrap();

    let read = table
        .enter(keeper, |context| {
            let opened = context.continuation().unwrap();
            let borrow = opened.value();
            // Every allocation takes the host's region through `&mut`, which is the retag the
            // reattached borrow has to survive — the chunk it names is its own allocation, reached
            // through the bump rather than inside it.
            for value in 0..GROWTH {
                context
                    .alloc_into::<Number, Number>(host, &[], |writer, _| writer.value(value as u32))
                    .unwrap();
            }
            *borrow
        })
        .unwrap();

    assert_eq!(read, 41);
}

#[test]
fn a_bare_hold_on_a_dead_cell_refuses() {
    let mut table: CellTable<Owned> = CellTable::new(4, pin);
    let holder = table.create(None, None).unwrap();
    let other = table.create(None, None).unwrap();
    table.release(other, Absorption::IntoHolder).unwrap();

    let refusal = table.enter(holder, |context| context.hold(other)).unwrap();
    assert_eq!(refusal, Err(StaleHandle(other)));
    assert!(!table.holds(holder, other));
}

#[test]
fn a_ring_an_outside_holder_keeps_from_every_merge_is_reported_and_leaks() {
    let mut table: CellTable<Owned> = CellTable::new(4, pin);
    let first = table.create(None, None).unwrap();
    let second = table.create(None, None).unwrap();
    // A bystander holding both sides keeps each count above the one a merge needs, so this ring
    // survives to the tier. A ring with no outside holder dissolves instead — see
    // `absorption::a_two_cell_ring_dissolves_when_one_side_dies`.
    let bystander = table.create(None, None).unwrap();

    table
        .enter(first, |context| context.hold(second))
        .unwrap()
        .unwrap();
    table
        .enter(second, |context| context.hold(first))
        .unwrap()
        .unwrap();
    table
        .enter(bystander, |context| {
            context.hold(first).unwrap();
            context.hold(second)
        })
        .unwrap()
        .unwrap();

    let ring = table
        .debug_ring_from(first)
        .expect("the hold graph has a cycle");
    assert_eq!(ring.len(), 2);
    assert!(ring.contains(&HoldNode::Cell(first)) && ring.contains(&HoldNode::Cell(second)));

    // Every death is declared, and the ring moves into the sealed tier intact: each record holds
    // the other, so neither count ever reaches zero. A ring is a leak, never a dangle.
    table.release(first, Absorption::IntoHolder).unwrap();
    table.release(second, Absorption::IntoHolder).unwrap();
    table.release(bystander, Absorption::IntoHolder).unwrap();
    assert_eq!(table.free.len(), 4);
    assert_eq!(table.sealed.len(), 2);

    let sealed_ring = table
        .debug_ring_from_sealed(table.sealed.ids().next().unwrap())
        .expect("the ring survives the seal");
    assert_eq!(sealed_ring.len(), 2);
    assert!(
        sealed_ring
            .iter()
            .all(|node| matches!(node, HoldNode::Sealed(_)))
    );
}

#[test]
fn an_acyclic_hold_graph_reports_no_ring() {
    let mut table: CellTable<Owned> = CellTable::new(4, pin);
    let first = table.create(None, None).unwrap();
    let second = table.create(None, None).unwrap();
    let third = table.create(None, None).unwrap();

    table
        .enter(first, |context| context.hold(second))
        .unwrap()
        .unwrap();
    table
        .enter(second, |context| context.hold(third))
        .unwrap()
        .unwrap();

    assert!(table.debug_ring_from(first).is_none());
}
