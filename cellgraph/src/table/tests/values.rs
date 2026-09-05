//! Values with reach: the region doors, the mint OR into the pin relation, the reclaim gate over
//! both relations, and the ring detector.

use super::super::*;
use super::{Number, Owned, state_of};

#[test]
fn a_value_allocated_in_the_executing_cell_reaches_only_that_cell() {
    let mut table: CellTable<Owned> = CellTable::new(4);
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
    let mut table: CellTable<Owned> = CellTable::new(2);
    let cell = table.create(None, None).unwrap();
    table.enter(cell, |context| context.handle()).unwrap();
    assert!(table.slots[cell.slot() as usize].region.is_none());
}

#[test]
fn placing_a_value_into_another_cell_mints_that_cell_a_hold_on_its_reach() {
    let mut table: CellTable<Owned> = CellTable::new(4);
    let producer = table.create(None, None).unwrap();
    let consumer = table.create(None, None).unwrap();

    let read = table
        .enter(producer, |context| {
            let value = context.alloc::<Number>(|writer| writer.value(41));
            // The placed value *is* the operand's borrow, so it genuinely reads the producer's
            // storage from the consumer's region.
            let placed = context
                .alloc_into::<Number, Number>(consumer, &[&value], |_writer, views| views[0])
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
    let mut table: CellTable<Owned> = CellTable::new(4);
    let holder = table.create(None, None).unwrap();
    let held = table.create(None, None).unwrap();

    table
        .enter(holder, |context| context.hold(held))
        .unwrap()
        .unwrap();
    assert!(table.holds(holder, held));

    // The slot comes straight back: retention lives in the sealed tier, never in the slab.
    table.release(held).unwrap();
    assert_eq!(state_of(&table, held), SlotState::Free);
    assert_eq!(table.sealed.len(), 1);
    let id = table.sealed.ids().next().unwrap();
    assert!(table.sealed_holds[holder.slot() as usize].contains(id));
    assert!(!table.holds(holder, held));

    table.release(holder).unwrap();
    assert_eq!(table.sealed.len(), 0);
    assert_eq!(table.free.len(), 4);
}

#[test]
fn a_bare_hold_on_a_dead_cell_refuses() {
    let mut table: CellTable<Owned> = CellTable::new(4);
    let holder = table.create(None, None).unwrap();
    let other = table.create(None, None).unwrap();
    table.release(other).unwrap();

    let refusal = table.enter(holder, |context| context.hold(other)).unwrap();
    assert_eq!(refusal, Err(StaleHandle(other)));
    assert!(!table.holds(holder, other));
}

#[test]
fn a_ring_is_reported_and_leaks_rather_than_dangles() {
    let mut table: CellTable<Owned> = CellTable::new(4);
    let first = table.create(None, None).unwrap();
    let second = table.create(None, None).unwrap();

    table
        .enter(first, |context| context.hold(second))
        .unwrap()
        .unwrap();
    table
        .enter(second, |context| context.hold(first))
        .unwrap()
        .unwrap();

    let ring = table
        .debug_ring_from(first)
        .expect("the hold graph has a cycle");
    assert_eq!(ring.len(), 2);
    assert!(ring.contains(&HoldNode::Cell(first)) && ring.contains(&HoldNode::Cell(second)));

    // Both deaths are declared, and the ring moves into the sealed tier intact: each record holds
    // the other, so neither count ever reaches zero. A ring is a leak, never a dangle.
    table.release(first).unwrap();
    table.release(second).unwrap();
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
    let mut table: CellTable<Owned> = CellTable::new(4);
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
