//! The three locality merges: a dying cell absorbed into its unique slab holder, a count-1 sealed
//! region absorbed at its holder's seal, and a cell with no slab holder sealing into its single
//! sealed namer. See [liveness-matrix.md § Locality
//! tactics](../../../design/liveness-matrix.md#locality-tactics).
//!
//! Each merge is a record the tier never mints, so what these tests read is an absence: no id, no
//! index entry, no accessor indirection — and the storage still there, in the bundle that took it.

use super::super::*;
use super::sealing::{LARGE, SMALL};
use super::{Borrowed, Number, Owned, state_of};

/// Bytes a cell's region bundle occupies, or zero for a cell that never allocated.
fn region_bytes<C: Reattachable>(table: &CellTable<C>, handle: Handle) -> usize {
    table.slots[handle.slot() as usize]
        .region
        .as_ref()
        .map_or(0, Region::allocated_bytes)
}

/// The one record in a table that has exactly one.
fn only_record<C: Reattachable>(table: &CellTable<C>) -> SealedId {
    assert_eq!(table.sealed.len(), 1);
    table.sealed.ids().next().expect("the tier holds a record")
}

#[test]
fn an_empty_table_is_quiescent_and_a_surviving_ring_is_not() {
    let mut table: CellTable<Owned> = CellTable::new(4);
    assert!(table.is_empty());

    let cell = table.create(None, None).unwrap();
    assert!(!table.is_empty());
    table.release(cell, Absorption::IntoHolder).unwrap();
    assert!(table.is_empty());

    // A ring an outside holder kept above every merge's count survives the wind-down, and that is
    // exactly what a non-empty table after the last release means.
    let first = table.create(None, None).unwrap();
    let second = table.create(None, None).unwrap();
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
    for cell in [first, second, bystander] {
        table.release(cell, Absorption::IntoHolder).unwrap();
    }

    assert!(!table.is_empty());
    let ring = table
        .debug_ring_from_sealed(table.sealed.ids().next().unwrap())
        .expect("the survivors are a ring");
    assert_eq!(ring.len(), 2);
}

#[test]
fn a_uniquely_held_cell_is_absorbed_into_its_holder_instead_of_sealing() {
    let mut table: CellTable<Borrowed> = CellTable::new(4);
    let consumer = table.create(None, None).unwrap();
    let producer = table.create(None, None).unwrap();

    // The consumer keeps a continuation over a value living in the producer's region, so it is the
    // producer's one holder and its stored mask names the producer's slot.
    table
        .enter(consumer, |context| {
            let value = context
                .alloc_into::<Number, Number>(producer, &[], |writer, _| writer.value(41))
                .unwrap();
            context.store_successor_capturing(&[&value], |_writer, views| views[0]);
        })
        .unwrap();
    assert!(table.holds(consumer, producer));
    let producer_bytes = region_bytes(&table, producer);
    let consumer_bytes = region_bytes(&table, consumer);
    assert!(producer_bytes > 0);

    table.release(producer, Absorption::IntoHolder).unwrap();

    // No id, no index entry, no record: the storage is the consumer's own now.
    assert_eq!(table.sealed.len(), 0);
    assert_eq!(state_of(&table, producer), SlotState::Free);
    assert!(!table.holds(consumer, producer));
    assert_eq!(
        region_bytes(&table, consumer),
        consumer_bytes + producer_bytes
    );

    let (value, names_consumer, names_producer, sealed_ids) = table
        .enter(consumer, |context| {
            let opened = context.continuation().unwrap();
            (
                *opened.value(),
                opened.reach().names(consumer.slot()),
                opened.reach().names(producer.slot()),
                opened.reach().sealed().len(),
            )
        })
        .unwrap();

    // The bump moved into the consumer's bundle without moving a chunk byte, so the borrow reads
    // the same address — and the reach that comes back is a plain per-value mask over a live cell,
    // never a derivation through the tier.
    assert_eq!(value, 41);
    assert!(names_consumer);
    assert!(!names_producer);
    assert_eq!(sealed_ids, 0);
}

#[test]
fn absorption_carries_the_dead_cells_holds_onto_its_holder() {
    let mut table: CellTable<Owned> = CellTable::new(6);
    let consumer = table.create(None, None).unwrap();
    let producer = table.create(None, None).unwrap();
    let reached = table.create(None, None).unwrap();
    let shared = table.create(None, None).unwrap();
    let alone = table.create(None, None).unwrap();

    table
        .enter(consumer, |context| {
            context.hold(shared).unwrap();
            context.hold(producer)
        })
        .unwrap()
        .unwrap();
    table
        .enter(producer, |context| {
            context.hold(reached).unwrap();
            context.hold(shared).unwrap();
            context.hold(alone)
        })
        .unwrap()
        .unwrap();

    table.release(shared, Absorption::Refused).unwrap();
    let shared_id = only_record(&table);
    table.release(alone, Absorption::Refused).unwrap();
    let alone_id = table
        .sealed
        .ids()
        .find(|id| *id != shared_id)
        .expect("the second seal minted a record");
    assert_eq!(table.sealed.get(shared_id).unwrap().holders, 2);
    assert_eq!(table.sealed.get(alone_id).unwrap().holders, 1);

    table.release(producer, Absorption::IntoHolder).unwrap();

    // The slab half arrives through the standard mint.
    assert!(table.pins.test(consumer.slot(), reached.slot()));
    // The sparse half changes holder rather than count where the consumer did not already hold it,
    // and where it did, the dead cell's duplicate hold simply goes.
    assert!(table.sealed_holds[consumer.slot() as usize].contains(alone_id));
    assert_eq!(table.sealed.get(alone_id).unwrap().holders, 1);
    assert_eq!(table.sealed.get(shared_id).unwrap().holders, 1);
}

#[test]
fn a_refused_release_seals_as_before() {
    let mut table: CellTable<Borrowed> = CellTable::new(4);
    let consumer = table.create(None, None).unwrap();
    let producer = table.create(None, None).unwrap();

    table
        .enter(consumer, |context| {
            let value = context
                .alloc_into::<Number, Number>(producer, &[], |writer, _| writer.value(41))
                .unwrap();
            context.store_successor_capturing(&[&value], |_writer, views| views[0]);
        })
        .unwrap();

    table.release(producer, Absorption::Refused).unwrap();

    // The very shape a merge would have collapsed, sealed instead: the embedder's refusal is the
    // whole difference.
    let id = only_record(&table);
    assert_eq!(table.sealed.get(id).unwrap().holders, 1);

    let (value, names_record) = table
        .enter(consumer, |context| {
            let opened = context.continuation().unwrap();
            (*opened.value(), opened.reach().names_sealed(id))
        })
        .unwrap();
    assert_eq!(value, 41);
    assert!(names_record);
}

#[test]
fn a_dead_resident_holder_absorbs_too() {
    let mut table: CellTable<Owned> = CellTable::new(4);
    let holder = table.create(None, None).unwrap();
    let child = table.create(Some(holder), None).unwrap();
    let held = table.create(None, None).unwrap();

    table
        .enter(holder, |context| context.hold(held))
        .unwrap()
        .unwrap();

    // The holder's death is declared, but its child's birth row keeps it in the slab. Its pin row
    // is still a maintained row, so it is still a merge target: "live holder" means the slab tier,
    // not a cell that may still execute.
    table.release(holder, Absorption::IntoHolder).unwrap();
    assert_eq!(state_of(&table, holder), SlotState::Dead);

    table.release(held, Absorption::IntoHolder).unwrap();
    assert_eq!(table.sealed.len(), 0);
    assert_eq!(state_of(&table, held), SlotState::Free);

    // Absorbing into a dead-resident cell only brings forward the fold its own disposal would have
    // performed: when the child goes, the whole bundle goes with the holder.
    table.release(child, Absorption::IntoHolder).unwrap();
    assert!(table.is_empty());
}

#[test]
fn a_two_cell_ring_dissolves_when_one_side_dies() {
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

    // The merge runs even though the source held its own target: the mint's and-not is where the
    // hold on itself lands, so what would have been a sealed ring is a cell holding nothing.
    table.release(first, Absorption::IntoHolder).unwrap();
    assert_eq!(table.sealed.len(), 0);
    assert!(!table.holds(second, first));
    assert!(!table.holds(second, second));

    table.release(second, Absorption::IntoHolder).unwrap();
    assert!(table.is_empty());
    assert_eq!(table.free.len(), 4);
}

#[test]
fn a_seal_absorbs_its_count_one_sealed_holds() {
    let mut table: CellTable<Owned> = CellTable::new(6);
    let top = table.create(None, None).unwrap();
    let other = table.create(None, None).unwrap();
    let middle = table.create(None, None).unwrap();
    let base = table.create(None, None).unwrap();
    let reached = table.create(None, None).unwrap();

    table
        .enter(base, |context| {
            context.alloc::<Number>(|writer| writer.value(1));
            context.hold(reached)
        })
        .unwrap()
        .unwrap();
    table
        .enter(middle, |context| {
            context.alloc::<Number>(|writer| writer.value(2));
            context.hold(base)
        })
        .unwrap()
        .unwrap();
    // Two holders, so the middle cell seals rather than absorbing into one of them.
    for holder in [top, other] {
        table
            .enter(holder, |context| context.hold(middle))
            .unwrap()
            .unwrap();
    }
    let base_bytes = region_bytes(&table, base);
    let middle_bytes = region_bytes(&table, middle);

    table.release(base, Absorption::Refused).unwrap();
    let base_id = only_record(&table);
    assert!(table.naming[reached.slot() as usize].contains(base_id));

    table.release(middle, Absorption::IntoHolder).unwrap();

    // The base's record had one holder — the sealing cell — so the seal folds it in rather than
    // leaving a chain of two records with one indirection each.
    let middle_id = only_record(&table);
    assert_ne!(middle_id, base_id);
    let record = table.sealed.get(middle_id).unwrap();
    assert_eq!(record.holders, 2);
    assert!(record.aggregate.names(reached.slot()));
    assert!(!record.aggregate.names_sealed(base_id));
    assert_eq!(record.retained_bytes(), base_bytes + middle_bytes);
    assert!(!table.naming[reached.slot() as usize].contains(base_id));
    assert!(table.naming[reached.slot() as usize].contains(middle_id));

    table.release(top, Absorption::IntoHolder).unwrap();
    assert_eq!(table.sealed.len(), 1);
    table.release(other, Absorption::IntoHolder).unwrap();
    assert_eq!(table.sealed.len(), 0);
    table.release(reached, Absorption::IntoHolder).unwrap();
    assert!(table.is_empty());
}

#[test]
fn seal_time_absorption_follows_a_chain_whose_counts_dropped() {
    let mut table: CellTable<Owned> = CellTable::new(6);
    let first_keeper = table.create(None, None).unwrap();
    let second_keeper = table.create(None, None).unwrap();
    let top = table.create(None, None).unwrap();
    let middle = table.create(None, None).unwrap();
    let base = table.create(None, None).unwrap();
    let extra = table.create(None, None).unwrap();

    table
        .enter(middle, |context| context.hold(base))
        .unwrap()
        .unwrap();
    table
        .enter(extra, |context| context.hold(base))
        .unwrap()
        .unwrap();
    table
        .enter(top, |context| context.hold(middle))
        .unwrap()
        .unwrap();
    for keeper in [first_keeper, second_keeper] {
        table
            .enter(keeper, |context| context.hold(top))
            .unwrap()
            .unwrap();
    }

    table.release(base, Absorption::Refused).unwrap();
    // Two holders, so the middle cell's own seal finds nothing to absorb.
    table.release(middle, Absorption::Refused).unwrap();
    assert_eq!(table.sealed.len(), 2);

    // The extra holder goes, and the base's count drops to one — but nothing seals here, so the
    // candidate is only noticed at the next seal.
    table.release(extra, Absorption::IntoHolder).unwrap();
    assert_eq!(table.sealed.len(), 2);

    table.release(top, Absorption::IntoHolder).unwrap();

    // The worklist is what makes this one record rather than three: absorbing the middle record
    // transfers the base's id onto the new one, where its count of one qualifies it in turn.
    let id = only_record(&table);
    assert_eq!(table.sealed.get(id).unwrap().holders, 2);

    for keeper in [first_keeper, second_keeper] {
        table.release(keeper, Absorption::IntoHolder).unwrap();
    }
    assert!(table.is_empty());
}

#[test]
fn a_count_one_record_held_by_a_live_cell_stays_sealed() {
    let mut table: CellTable<Owned> = CellTable::new(4);
    let holder = table.create(None, None).unwrap();
    let extra = table.create(None, None).unwrap();
    let held = table.create(None, None).unwrap();

    table
        .enter(holder, |context| {
            context.alloc::<Number>(|writer| writer.value(1));
            context.hold(held)
        })
        .unwrap()
        .unwrap();
    table
        .enter(held, |context| {
            context.alloc::<Number>(|writer| writer.value(2));
            context.hold(extra)
        })
        .unwrap()
        .unwrap();
    table
        .enter(extra, |context| context.hold(held))
        .unwrap()
        .unwrap();

    table.release(held, Absorption::Refused).unwrap();
    let id = only_record(&table);
    assert_eq!(table.sealed.get(id).unwrap().holders, 2);
    let holder_bytes = region_bytes(&table, holder);

    table.release(extra, Absorption::IntoHolder).unwrap();

    // A count of one is not by itself a merge: storage that has already sealed never re-enters the
    // live tier, so the record waits for the cascade instead of folding into the live cell.
    assert_eq!(table.sealed.get(id).unwrap().holders, 1);
    assert_eq!(region_bytes(&table, holder), holder_bytes);

    table.release(holder, Absorption::IntoHolder).unwrap();
    assert!(table.is_empty());
}

#[test]
fn a_cell_with_a_single_sealed_namer_seals_into_it() {
    let mut table: CellTable<Borrowed> = CellTable::new(4);
    let keeper = table.create(None, None).unwrap();
    let namer = table.create(None, None).unwrap();
    let dying = table.create(None, None).unwrap();
    let reached = table.create(None, None).unwrap();

    // The keeper's continuation lives over the namer's storage, and the namer holds the dying
    // cell — so once the namer seals, the dying cell's only name is that record's aggregate.
    table
        .enter(namer, |context| context.hold(dying))
        .unwrap()
        .unwrap();
    table
        .enter(keeper, |context| {
            let value = context
                .alloc_into::<Number, Number>(namer, &[], |writer, _| writer.value(41))
                .unwrap();
            context.store_successor_capturing(&[&value], |_writer, views| views[0]);
        })
        .unwrap();
    table
        .enter(dying, |context| {
            context.alloc::<Number>(|writer| writer.value(7));
            context.hold(reached)
        })
        .unwrap()
        .unwrap();
    let dying_bytes = region_bytes(&table, dying);
    assert!(dying_bytes > 0);

    table.release(namer, Absorption::Refused).unwrap();
    let id = only_record(&table);
    assert!(table.sealed.get(id).unwrap().aggregate.names(dying.slot()));
    let namer_bytes = table.sealed.get(id).unwrap().retained_bytes();

    table.release(dying, Absorption::IntoHolder).unwrap();

    // No second record: the dying cell's storage and holds go into the aggregate that already
    // named it, and the slots its row named trade its bit for the record's name.
    assert_eq!(table.sealed.len(), 1);
    assert_eq!(state_of(&table, dying), SlotState::Free);
    let record = table.sealed.get(id).unwrap();
    assert_eq!(record.holders, 1);
    assert!(!record.aggregate.names(dying.slot()));
    assert!(record.aggregate.names(reached.slot()));
    assert_eq!(record.retained_bytes(), namer_bytes + dying_bytes);
    assert!(table.naming[reached.slot() as usize].contains(id));

    // The read still goes through the record, whose bundle grew a bump under the borrow.
    let (value, names_record) = table
        .enter(keeper, |context| {
            let opened = context.continuation().unwrap();
            (*opened.value(), opened.reach().names_sealed(id))
        })
        .unwrap();
    assert_eq!(value, 41);
    assert!(names_record);
}

/// Absorb a uniquely held cell holding `resident` values, and report the maintenance it performed.
fn absorb_work_for(resident: usize) -> u64 {
    let mut table: CellTable<Owned> = CellTable::new(4);
    let holder = table.create(None, None).unwrap();
    let producer = table.create(None, None).unwrap();

    table
        .enter(producer, |context| {
            for value in 0..resident {
                context.alloc::<Number>(|writer| writer.value(value as u32));
            }
        })
        .unwrap();
    table
        .enter(holder, |context| context.hold(producer))
        .unwrap()
        .unwrap();

    let before = table.seal_work;
    table.release(producer, Absorption::IntoHolder).unwrap();
    assert_eq!(table.sealed.len(), 0);
    table.seal_work - before
}

#[test]
fn absorption_is_bounded_by_the_hold_set_not_the_storage() {
    // The bundle takes the bump whole, so a region with ten thousand resident values folds in for
    // what one with sixteen costs — the same atomicity the seal transition has.
    assert_eq!(absorb_work_for(SMALL), absorb_work_for(LARGE));
}

#[test]
fn a_sealed_ring_dissolves_through_its_last_namer() {
    let mut table: CellTable<Owned> = CellTable::new(4);
    let first = table.create(None, None).unwrap();
    let second = table.create(None, None).unwrap();
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
        .enter(bystander, |context| context.hold(first))
        .unwrap()
        .unwrap();

    // Two holders, so the first cell seals; its aggregate names the second, and the second holds
    // the record.
    table.release(first, Absorption::IntoHolder).unwrap();
    let id = only_record(&table);
    assert_eq!(table.sealed.get(id).unwrap().holders, 2);
    assert!(table.sealed.get(id).unwrap().aggregate.names(second.slot()));

    table.release(bystander, Absorption::IntoHolder).unwrap();
    assert_eq!(table.sealed.get(id).unwrap().holders, 1);

    // The second cell's only namer is the record it itself holds: the fold turns that hold into a
    // self-hold, the count reaches zero, and the ring is freed rather than leaked.
    table.release(second, Absorption::IntoHolder).unwrap();
    assert!(table.is_empty());
    assert_eq!(table.free.len(), 4);
}

#[test]
fn a_seal_that_absorbs_every_holder_it_had_reclaims_itself() {
    let mut table: CellTable<Owned> = CellTable::new(4);
    let held = table.create(None, None).unwrap();
    let first = table.create(None, None).unwrap();
    let second = table.create(None, None).unwrap();

    table
        .enter(held, |context| {
            context.hold(first).unwrap();
            context.hold(second)
        })
        .unwrap()
        .unwrap();
    for namer in [first, second] {
        table
            .enter(namer, |context| context.hold(held))
            .unwrap()
            .unwrap();
        table.release(namer, Absorption::Refused).unwrap();
    }
    assert_eq!(table.sealed.len(), 2);

    // Two namers and no slab holder, so this is a plain seal — and both namers are count-1 regions
    // the new record holds, so both fold in. Each fold turns a hold on the new record into a
    // self-hold, and the second takes its count to zero: the whole ring goes in one release.
    table.release(held, Absorption::IntoHolder).unwrap();
    assert!(table.is_empty());
    assert_eq!(table.free.len(), 4);
}
