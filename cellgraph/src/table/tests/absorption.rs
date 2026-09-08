//! The three locality merges: a dying cell absorbed into its unique slab holder, a count-1 sealed
//! region absorbed at its holder's seal, and a cell with no slab holder sealing into its single
//! sealed namer. See [liveness-matrix.md § Locality
//! tactics](../../../design/liveness-matrix.md#locality-tactics).
//!
//! Each merge is a sealed cell the tier never mints, so what these tests read is an absence: no id,
//! no index entry, no accessor indirection — and the storage still there, in the bundle that took
//! it.

use proptest::prelude::*;

use super::super::*;
use super::{
    Borrowed, Number, Owned, continuation_reach_index, live_bytes, operand, pin, pinned, state_of,
};

/// Bytes a cell's region bundle occupies, or zero for a cell that never allocated.
fn region_bytes<C: Reattachable>(table: &CellTable<C>, handle: Handle) -> usize {
    table.slots[handle.slot() as usize]
        .region
        .as_ref()
        .map_or(0, Region::allocated_bytes)
}

/// The one sealed cell in a table that has exactly one.
fn only_sealed_cell<C: Reattachable>(table: &CellTable<C>) -> SealedId {
    assert_eq!(table.sealed.len(), 1);
    table
        .sealed
        .ids()
        .next()
        .expect("the tier holds a sealed cell")
}

#[test]
fn an_empty_table_is_quiescent_and_a_surviving_ring_is_not() {
    let mut table: CellTable<Owned> = CellTable::new(4, pin);
    assert!(table.is_empty());

    let cell = table.create(None, None).unwrap();
    assert!(!table.is_empty());
    table.release(cell, ReleaseAbsorption::IntoHolder).unwrap();
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
        table.release(cell, ReleaseAbsorption::IntoHolder).unwrap();
    }

    assert!(!table.is_empty());
    let ring = table
        .debug_ring_from(HoldNode::Sealed(table.sealed.ids().next().unwrap()))
        .expect("the survivors are a ring");
    assert_eq!(ring.len(), 2);
}

#[test]
fn a_uniquely_held_cell_is_absorbed_into_its_holder_instead_of_sealing() {
    let mut table: CellTable<Borrowed> = CellTable::new(4, pin);
    let consumer = table.create(None, None).unwrap();
    let producer = table.create(None, None).unwrap();

    // The consumer keeps a continuation over a value living in the producer's region, so it is the
    // producer's one holder and its stored mask names the producer's slot.
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
    let producer_bytes = region_bytes(&table, producer);
    let consumer_bytes = region_bytes(&table, consumer);
    assert!(producer_bytes > 0);

    table
        .release(producer, ReleaseAbsorption::IntoHolder)
        .unwrap();

    // No id, no index entry, no sealed cell: the storage is the consumer's own now.
    assert_eq!(table.sealed.len(), 0);
    assert_eq!(state_of(&table, producer), SlotState::Free);
    assert!(!table.holds(consumer, producer));
    assert_eq!(
        region_bytes(&table, consumer),
        consumer_bytes + producer_bytes
    );

    // The merge rewrote the consumer's stored mask: the dead cell's bit became the holder's, and
    // nothing sealed, so the mask stays a plain slab row over live cells.
    let stored = continuation_reach_index(&table, consumer);
    assert!(stored.names(consumer.slot()));
    assert!(!stored.names(producer.slot()));
    assert_eq!(stored.sealed().len(), 0);

    // The bump moved into the consumer's bundle without moving a chunk byte, so the borrow reads
    // the same address.
    let value = table
        .enter(consumer, |context| *context.continuation().unwrap().value())
        .unwrap();
    assert_eq!(value, 41);
}

#[test]
fn absorption_carries_the_dead_cells_holds_onto_its_holder() {
    let mut table: CellTable<Owned> = CellTable::new(6, pin);
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

    table.release(shared, ReleaseAbsorption::Refused).unwrap();
    let shared_id = only_sealed_cell(&table);
    table.release(alone, ReleaseAbsorption::Refused).unwrap();
    let alone_id = table
        .sealed
        .ids()
        .find(|id| *id != shared_id)
        .expect("the second seal minted a sealed cell");
    assert_eq!(table.sealed.get(shared_id).unwrap().holders, 2);
    assert_eq!(table.sealed.get(alone_id).unwrap().holders, 1);

    table
        .release(producer, ReleaseAbsorption::IntoHolder)
        .unwrap();

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
    let mut table: CellTable<Borrowed> = CellTable::new(4, pin);
    let consumer = table.create(None, None).unwrap();
    let producer = table.create(None, None).unwrap();

    table
        .enter(consumer, |context| {
            let value = context
                .alloc_into::<Number, Number>(producer, &[], |writer, _| writer.value(41))
                .unwrap();
            context
                .store_successor_capturing(&[operand(&value)], |_writer, views| pinned(&views[0]));
        })
        .unwrap();

    table.release(producer, ReleaseAbsorption::Refused).unwrap();

    // The very shape a merge would have collapsed, sealed instead: the embedder's refusal is the
    // whole difference.
    let id = only_sealed_cell(&table);
    assert_eq!(table.sealed.get(id).unwrap().holders, 1);

    assert!(continuation_reach_index(&table, consumer).names_sealed(id));
    let value = table
        .enter(consumer, |context| *context.continuation().unwrap().value())
        .unwrap();
    assert_eq!(value, 41);
}

#[test]
fn a_dead_resident_holder_absorbs_too() {
    let mut table: CellTable<Owned> = CellTable::new(4, pin);
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
    table
        .release(holder, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert_eq!(state_of(&table, holder), SlotState::Dead);

    table.release(held, ReleaseAbsorption::IntoHolder).unwrap();
    assert_eq!(table.sealed.len(), 0);
    assert_eq!(state_of(&table, held), SlotState::Free);

    // Absorbing into a dead-resident cell only brings forward the fold its own disposal would have
    // performed: when the child goes, the whole bundle goes with the holder.
    table.release(child, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(table.is_empty());
}

#[test]
fn a_two_cell_ring_dissolves_when_one_side_dies() {
    let mut table: CellTable<Owned> = CellTable::new(4, pin);
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
    table.release(first, ReleaseAbsorption::IntoHolder).unwrap();
    assert_eq!(table.sealed.len(), 0);
    assert!(!table.holds(second, first));
    assert!(!table.holds(second, second));

    table
        .release(second, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert!(table.is_empty());
    assert_eq!(table.free.len(), 4);
}

#[test]
fn a_seal_absorbs_its_count_one_sealed_holds() {
    let mut table: CellTable<Owned> = CellTable::new(6, pin);
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

    table.release(base, ReleaseAbsorption::Refused).unwrap();
    let base_id = only_sealed_cell(&table);
    assert!(table.naming[reached.slot() as usize].contains(base_id));

    table
        .release(middle, ReleaseAbsorption::IntoHolder)
        .unwrap();

    // The base's sealed cell had one holder — the sealing cell — so the seal folds it in rather
    // than leaving a chain of two sealed cells with one indirection each.
    let middle_id = only_sealed_cell(&table);
    assert_ne!(middle_id, base_id);
    let sealed_cell = table.sealed.get(middle_id).unwrap();
    assert_eq!(sealed_cell.holders, 2);
    assert!(sealed_cell.aggregate.names(reached.slot()));
    assert!(!sealed_cell.aggregate.names_sealed(base_id));
    assert_eq!(sealed_cell.retained_bytes(), base_bytes + middle_bytes);
    assert!(!table.naming[reached.slot() as usize].contains(base_id));
    assert!(table.naming[reached.slot() as usize].contains(middle_id));

    table.release(top, ReleaseAbsorption::IntoHolder).unwrap();
    assert_eq!(table.sealed.len(), 1);
    table.release(other, ReleaseAbsorption::IntoHolder).unwrap();
    assert_eq!(table.sealed.len(), 0);
    table
        .release(reached, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert!(table.is_empty());
}

#[test]
fn seal_time_absorption_follows_a_chain_whose_counts_dropped() {
    let mut table: CellTable<Owned> = CellTable::new(6, pin);
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

    table.release(base, ReleaseAbsorption::Refused).unwrap();
    // Two holders, so the middle cell's own seal finds nothing to absorb.
    table.release(middle, ReleaseAbsorption::Refused).unwrap();
    assert_eq!(table.sealed.len(), 2);

    // The extra holder goes, and the base's count drops to one — but nothing seals here, so the
    // candidate is only noticed at the next seal.
    table.release(extra, ReleaseAbsorption::IntoHolder).unwrap();
    assert_eq!(table.sealed.len(), 2);

    table.release(top, ReleaseAbsorption::IntoHolder).unwrap();

    // The worklist is what makes this one sealed cell rather than three: absorbing the middle
    // sealed cell transfers the base's id onto the new one, where its count of one qualifies it in
    // turn.
    let id = only_sealed_cell(&table);
    assert_eq!(table.sealed.get(id).unwrap().holders, 2);

    for keeper in [first_keeper, second_keeper] {
        table
            .release(keeper, ReleaseAbsorption::IntoHolder)
            .unwrap();
    }
    assert!(table.is_empty());
}

#[test]
fn a_count_one_sealed_cell_held_by_a_live_cell_stays_sealed() {
    let mut table: CellTable<Owned> = CellTable::new(4, pin);
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

    table.release(held, ReleaseAbsorption::Refused).unwrap();
    let id = only_sealed_cell(&table);
    assert_eq!(table.sealed.get(id).unwrap().holders, 2);
    let holder_bytes = region_bytes(&table, holder);
    let sealed_bytes = table.sealed.get(id).unwrap().retained_bytes();
    let slab_bytes = live_bytes(&table, 4);
    assert!(sealed_bytes > 0);

    table.release(extra, ReleaseAbsorption::IntoHolder).unwrap();

    // A count of one is not by itself a merge: storage that has already sealed never re-enters the
    // live tier, so the sealed cell waits for the cascade instead of folding into the live cell.
    // The provenance is read from the two tiers' byte totals rather than tracked through the
    // release — the sealed cell keeps every byte it had, and the whole slab tier is no larger than
    // it was.
    assert_eq!(table.sealed.get(id).unwrap().holders, 1);
    assert_eq!(table.sealed.get(id).unwrap().retained_bytes(), sealed_bytes);
    assert_eq!(region_bytes(&table, holder), holder_bytes);
    assert!(live_bytes(&table, 4) <= slab_bytes);

    table
        .release(holder, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert!(table.is_empty());
}

#[test]
fn a_cell_with_a_single_sealed_namer_seals_into_it() {
    let mut table: CellTable<Borrowed> = CellTable::new(4, pin);
    let keeper = table.create(None, None).unwrap();
    let namer = table.create(None, None).unwrap();
    let dying = table.create(None, None).unwrap();
    let reached = table.create(None, None).unwrap();

    // The keeper's continuation lives over the namer's storage, and the namer holds the dying cell
    // — so once the namer seals, the dying cell's only name is that sealed cell's aggregate.
    table
        .enter(namer, |context| context.hold(dying))
        .unwrap()
        .unwrap();
    table
        .enter(keeper, |context| {
            let value = context
                .alloc_into::<Number, Number>(namer, &[], |writer, _| writer.value(41))
                .unwrap();
            context
                .store_successor_capturing(&[operand(&value)], |_writer, views| pinned(&views[0]));
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

    table.release(namer, ReleaseAbsorption::Refused).unwrap();
    let id = only_sealed_cell(&table);
    assert!(table.sealed.get(id).unwrap().aggregate.names(dying.slot()));
    let namer_bytes = table.sealed.get(id).unwrap().retained_bytes();

    table.release(dying, ReleaseAbsorption::IntoHolder).unwrap();

    // No second sealed cell: the dying cell's storage and holds go into the aggregate that already
    // named it, and the slots its row named trade its bit for the sealed cell's name.
    assert_eq!(table.sealed.len(), 1);
    assert_eq!(state_of(&table, dying), SlotState::Free);
    let sealed_cell = table.sealed.get(id).unwrap();
    assert_eq!(sealed_cell.holders, 1);
    assert!(!sealed_cell.aggregate.names(dying.slot()));
    assert!(sealed_cell.aggregate.names(reached.slot()));
    assert_eq!(sealed_cell.retained_bytes(), namer_bytes + dying_bytes);
    assert!(table.naming[reached.slot() as usize].contains(id));

    // The read still goes through the sealed cell, whose bundle grew a bump under the borrow.
    assert!(continuation_reach_index(&table, keeper).names_sealed(id));
    let value = table
        .enter(keeper, |context| *context.continuation().unwrap().value())
        .unwrap();
    assert_eq!(value, 41);
}

/// Absorb a uniquely held producer into its consumer, and report the maintenance the merge
/// performed.
///
/// The producer holds `reached` live cells, `shared` sealed regions the consumer already holds,
/// and `alone` sealed regions only it holds — so the hold set varies in both halves and in whether
/// each sealed id transfers or duplicates, while `resident` varies what the region stores.
fn absorb_work_for(resident: usize, reached: u32, shared: u32, alone: u32) -> u64 {
    let mut table: CellTable<Owned> = CellTable::new(2 + reached + shared + alone, pin);
    let consumer = table.create(None, None).unwrap();
    let producer = table.create(None, None).unwrap();
    let mut make = |count| {
        (0..count)
            .map(|_| table.create(None, None).unwrap())
            .collect()
    };
    let reached_cells: Vec<Handle> = make(reached);
    let shared_cells: Vec<Handle> = make(shared);
    let alone_cells: Vec<Handle> = make(alone);

    table
        .enter(producer, |context| {
            for value in 0..resident {
                context.alloc::<Number>(|writer| writer.value(value as u32));
            }
            for cell in reached_cells
                .iter()
                .chain(&shared_cells)
                .chain(&alone_cells)
            {
                context.hold(*cell).unwrap();
            }
        })
        .unwrap();
    table
        .enter(consumer, |context| {
            context.hold(producer).unwrap();
            for cell in &shared_cells {
                context.hold(*cell).unwrap();
            }
        })
        .unwrap();
    // Refused, so each becomes a sealed cell rather than absorbing into the producer first.
    for cell in shared_cells.iter().chain(&alone_cells) {
        table.release(*cell, ReleaseAbsorption::Refused).unwrap();
    }

    let before = table.seal_work;
    table
        .release(producer, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert_eq!(table.sealed.len(), (shared + alone) as usize);
    table.seal_work - before
}

/// A resident count large enough that work proportional to storage could not match the lean run's.
fn heavy() -> std::ops::Range<usize> {
    if cfg!(miri) { 64..128 } else { 1_600..2_000 }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: if cfg!(miri) { 4 } else { 64 },
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// The merge's cost is the hold set's shape and nothing else.
    ///
    /// Two assertions, and the second is what the first alone could not say: a shape whose hold
    /// sets are singletons makes any flat cost look storage-independent, so the closed form over a
    /// *generated* hold set is what separates "independent of the region" from "constant".
    #[test]
    fn absorption_costs_the_hold_set_and_not_the_storage(
        reached in 0..3u32,
        shared in 0..3u32,
        alone in 0..3u32,
        lean in 0..8usize,
        laden in heavy(),
    ) {
        let cost = absorb_work_for(lean, reached, shared, alone);
        prop_assert_eq!(cost, absorb_work_for(laden, reached, shared, alone));
        // The merge itself, plus one release per sealed id the consumer already held. A slab bit
        // costs nothing beyond the word OR, and a transferred id changes holder, not count.
        prop_assert_eq!(cost, 1 + u64::from(shared));
    }
}

#[test]
fn a_sealed_ring_dissolves_through_its_last_namer() {
    let mut table: CellTable<Owned> = CellTable::new(4, pin);
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
    // the sealed cell.
    table.release(first, ReleaseAbsorption::IntoHolder).unwrap();
    let id = only_sealed_cell(&table);
    assert_eq!(table.sealed.get(id).unwrap().holders, 2);
    assert!(table.sealed.get(id).unwrap().aggregate.names(second.slot()));

    table
        .release(bystander, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert_eq!(table.sealed.get(id).unwrap().holders, 1);

    // The second cell's only namer is the sealed cell it itself holds: the fold turns that hold
    // into a self-hold, the count reaches zero, and the ring is freed rather than leaked.
    table
        .release(second, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert!(table.is_empty());
    assert_eq!(table.free.len(), 4);
}

#[test]
fn a_seal_that_absorbs_every_holder_it_had_reclaims_itself() {
    let mut table: CellTable<Owned> = CellTable::new(4, pin);
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
        table.release(namer, ReleaseAbsorption::Refused).unwrap();
    }
    assert_eq!(table.sealed.len(), 2);

    // Two namers and no slab holder, so this is a plain seal — and both namers are count-1 regions
    // the new sealed cell holds, so both fold in. Each fold turns a hold on the new sealed cell
    // into a self-hold, and the second takes its count to zero: the whole ring goes in one release.
    table.release(held, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(table.is_empty());
    assert_eq!(table.free.len(), 4);
}
