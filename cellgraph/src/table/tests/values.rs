//! Values with reach: the region doors, the mint OR into the pin relation, the reclaim gate over
//! both relations, and the ring detector.

use super::super::*;
use super::{
    ANCHOR, Borrowed, Number, Owned, continuation_reach_index, operand, pin, pinned, state_of,
};

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
    table.enter(cell, |context| context.cell()).unwrap();
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
fn a_held_cell_leaves_the_slab_at_its_death_and_its_sealed_cell_goes_with_its_holder() {
    let mut table: CellTable<Owned> = CellTable::new(4, pin);
    let holder = table.create(None, None).unwrap();
    let held = table.create(None, None).unwrap();

    table
        .enter(holder, |context| context.hold(held))
        .unwrap()
        .unwrap();
    assert!(table.holds(holder, held));

    // The slot comes straight back: retention lives in the sealed tier, never in the slab.
    table.release(held, ReleaseAbsorption::Refused).unwrap();
    assert_eq!(state_of(&table, held), SlotState::Free);
    assert_eq!(table.sealed.len(), 1);
    let id = table.sealed.ids().next().unwrap();
    assert!(table.sealed_holds[holder.slot() as usize].contains(id));
    assert!(!table.holds(holder, held));

    table
        .release(holder, ReleaseAbsorption::IntoHolder)
        .unwrap();
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
    table.release(other, ReleaseAbsorption::IntoHolder).unwrap();

    let refusal = table.enter(holder, |context| context.hold(other)).unwrap();
    assert_eq!(refusal, Err(Stale(other)));
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
        .debug_ring_from(HoldNode::Cell(first))
        .expect("the hold graph has a cycle");
    assert_eq!(ring.len(), 2);
    assert!(ring.contains(&HoldNode::Cell(first)) && ring.contains(&HoldNode::Cell(second)));

    // Every death is declared, and the ring moves into the sealed tier intact: each sealed cell
    // holds the other, so neither count ever reaches zero. A ring is a leak, never a dangle.
    table.release(first, ReleaseAbsorption::IntoHolder).unwrap();
    table
        .release(second, ReleaseAbsorption::IntoHolder)
        .unwrap();
    table
        .release(bystander, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert_eq!(table.free.len(), 4);
    assert_eq!(table.sealed.len(), 2);

    let sealed_ring = table
        .debug_ring_from(HoldNode::Sealed(table.sealed.ids().next().unwrap()))
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

    assert!(table.debug_ring_from(HoldNode::Cell(first)).is_none());
}

// The doors a value crosses steps through: `keep` puts a carrier down in its home cell's resident
// table, and `redeem` takes it back up in a later step of a cell entitled to that storage. See
// [design/cellgraph.md § Passing values between cells](../../../design/cellgraph.md).

/// The one entry a cell's reach table holds, by the index a key names.
fn resident_reach<C: Reattachable>(table: &CellTable<C>, slot: u32, index: u32) -> &GraphReach<1> {
    table.slots[slot as usize]
        .reaches
        .get(index)
        .expect("the entry the key names is in the table")
}

#[test]
fn push_completes_a_value_built_into_the_consumer_is_read_in_its_own_step() {
    let mut table: CellTable<Owned> = CellTable::new(4, pin);
    let consumer = table.create(None, None).unwrap();
    let producer = table.create(None, None).unwrap();

    // The push shape: the producer builds straight into the consumer's region, so the value's home
    // is the consumer and the producer's own column stays empty.
    let kept = table
        .enter(producer, |context| {
            let placed = context
                .alloc_into::<Number, Number>(consumer, &[], |writer, _| writer.value(41))
                .unwrap();
            context.keep(placed)
        })
        .unwrap();
    assert_eq!(table.slots[consumer.slot() as usize].reaches.len(), 1);
    assert!(resident_reach(&table, consumer.slot(), 0).names(consumer.slot()));
    assert_eq!(table.relocations(), 0);

    // Nothing reaches the producer, so its death is a reclamation: the slot comes straight back
    // and the map it never entered stays empty.
    table
        .release(producer, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert_eq!(state_of(&table, producer), SlotState::Free);
    assert_eq!(table.relocations(), 0);

    let read = table
        .enter(consumer, |context| {
            let carrier = context
                .redeem(kept)
                .expect("the home redeems its own resident");
            *context.read(&carrier).value()
        })
        .unwrap();
    assert_eq!(read, 41);
}

#[test]
fn pull_completes_after_the_producer_seals() {
    let mut table: CellTable<Owned> = CellTable::new(4, pin);
    let consumer = table.create(None, None).unwrap();
    let producer = table.create(None, None).unwrap();

    table
        .enter(consumer, |context| context.hold(producer))
        .unwrap()
        .unwrap();
    let kept = table
        .enter(producer, |context| {
            let value = context.alloc::<Number>(|writer| writer.value(41));
            context.keep(value)
        })
        .unwrap();

    // The pull shape: the producer dies still held, so its storage seals and the key it minted
    // forwards to the sealed cell rather than stopping resolving.
    table.release(producer, ReleaseAbsorption::Refused).unwrap();
    let id = table.sealed.ids().next().unwrap();
    assert_eq!(table.relocation_of(producer), Some(SlabForward::Sealed(id)));
    assert_eq!(table.lineage_of(id), vec![producer]);

    let read = table
        .enter(consumer, |context| {
            let carrier = context
                .redeem(kept)
                .expect("the consumer holds the sealed cell");
            // A value out of a sealed cell reaches the sealed cell's id and nothing in the slab: a
            // hold on it keeps the whole aggregate alive transitively.
            assert!(carrier.reach().names_sealed(id));
            assert!(carrier.reach().slab_slots().next().is_none());
            assert_eq!(carrier.reach().sealed().iter().count(), 1);
            *context.read(&carrier).value()
        })
        .unwrap();
    assert_eq!(read, 41);

    // The sealed cell's last holder goes, so the sealed cell retires and takes its lineage with it.
    table
        .release(consumer, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert_eq!(table.sealed.len(), 0);
    assert_eq!(table.relocations(), 0);
}

#[test]
fn pull_completes_after_the_producer_is_absorbed_into_the_consumer() {
    let mut table: CellTable<Owned> = CellTable::new(4, pin);
    let consumer = table.create(None, None).unwrap();
    let producer = table.create(None, None).unwrap();

    table
        .enter(consumer, |context| context.hold(producer))
        .unwrap()
        .unwrap();
    let kept = table
        .enter(producer, |context| {
            let value = context.alloc::<Number>(|writer| writer.value(41));
            context.keep(value)
        })
        .unwrap();

    // The uniquely held producer folds into its holder rather than minting a sealed cell, and its
    // resident masks move with the storage, re-homed at the consumer's bit.
    table
        .release(producer, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert_eq!(table.sealed.len(), 0);
    assert_eq!(
        table.relocation_of(producer),
        Some(SlabForward::Slab {
            slot: consumer.slot(),
            base: 0,
        })
    );
    let migrated = resident_reach(&table, consumer.slot(), 0);
    assert!(migrated.names(consumer.slot()));
    assert!(!migrated.names(producer.slot()));

    // A second resident kept by the consumer itself lands past the migrated one, so the two keys
    // name different entries and both redeem.
    let own = table
        .enter(consumer, |context| {
            let value = context.alloc::<Number>(|writer| writer.value(9));
            context.keep(value)
        })
        .unwrap();
    let read = table
        .enter(consumer, |context| {
            let pulled = context.redeem(kept).expect("the absorbing cell answers");
            let mine = context
                .redeem(own)
                .expect("the home redeems its own resident");
            (*context.read(&pulled).value(), *context.read(&mine).value())
        })
        .unwrap();
    assert_eq!(read, (41, 9));
}

#[test]
fn a_resident_forwarded_through_two_merges_is_still_found() {
    let mut table: CellTable<Owned> = CellTable::new(4, pin);
    let end = table.create(None, None).unwrap();
    let middle = table.create(None, None).unwrap();
    let head = table.create(None, None).unwrap();

    table
        .enter(middle, |context| context.hold(head))
        .unwrap()
        .unwrap();
    table
        .enter(end, |context| context.hold(middle))
        .unwrap()
        .unwrap();
    let kept = table
        .enter(head, |context| {
            let value = context.alloc::<Number>(|writer| writer.value(41));
            context.keep(value)
        })
        .unwrap();

    // Merge one: into the slab. Merge two: into the tier. The key is rewritten by each, so the
    // chain of single-consumer producers costs the map one entry per merge and none per value.
    table.release(head, ReleaseAbsorption::IntoHolder).unwrap();
    table.release(middle, ReleaseAbsorption::Refused).unwrap();
    let id = table.sealed.ids().next().unwrap();
    assert_eq!(table.relocation_of(head), Some(SlabForward::Sealed(id)));

    let read = table
        .enter(end, |context| {
            let carrier = context.redeem(kept).expect("the end holds the sealed cell");
            assert!(carrier.reach().names_sealed(id));
            *context.read(&carrier).value()
        })
        .unwrap();
    assert_eq!(read, 41);

    table.release(end, ReleaseAbsorption::IntoHolder).unwrap();
    assert_eq!(table.sealed.len(), 0);
    assert_eq!(table.relocations(), 0);
    assert_eq!(table.free.len(), 4);
}

#[test]
fn redeem_refuses_a_cell_that_does_not_hold_the_home() {
    let mut table: CellTable<Owned> = CellTable::new(4, pin);
    let home = table.create(None, None).unwrap();
    let bystander = table.create(None, None).unwrap();
    let middle = table.create(None, None).unwrap();
    let far = table.create(None, None).unwrap();

    let kept = table
        .enter(home, |context| {
            let value = context.alloc::<Number>(|writer| writer.value(41));
            context.keep(value)
        })
        .unwrap();

    let refused = table
        .enter(bystander, |context| context.redeem(kept).err())
        .unwrap();
    assert_eq!(refused, Some(RedeemError::Unheld));

    // A hold is what entitles: with one, the same key answers.
    let read = table
        .enter(bystander, |context| {
            context.hold(home).unwrap();
            let carrier = context.redeem(kept).expect("the holder is entitled");
            *context.read(&carrier).value()
        })
        .unwrap();
    assert_eq!(read, 41);

    // Entitlement is the direct relation, not its closure: reaching the home only through a cell
    // in between is not a claim on the home's storage.
    table
        .enter(middle, |context| context.hold(home))
        .unwrap()
        .unwrap();
    table
        .enter(far, |context| context.hold(middle))
        .unwrap()
        .unwrap();
    let transitive = table
        .enter(far, |context| context.redeem(kept).err())
        .unwrap();
    assert_eq!(transitive, Some(RedeemError::Unheld));
}

#[test]
fn redeem_refuses_once_the_storage_is_gone() {
    let mut table: CellTable<Owned> = CellTable::new(4, pin);
    let reclaimed = table.create(None, None).unwrap();
    let onlooker = table.create(None, None).unwrap();

    let orphan = table
        .enter(reclaimed, |context| {
            let value = context.alloc::<Number>(|writer| writer.value(41));
            context.keep(value)
        })
        .unwrap();
    // Nothing reached the home, so its death frees the chunks the resident named.
    table
        .release(reclaimed, ReleaseAbsorption::IntoHolder)
        .unwrap();
    let gone = table
        .enter(onlooker, |context| context.redeem(orphan).err())
        .unwrap();
    assert_eq!(gone, Some(RedeemError::Gone));

    // The other way storage goes: sealed, then retired when its last holder leaves.
    let sealed_home = table.create(None, None).unwrap();
    let holder = table.create(None, None).unwrap();
    table
        .enter(holder, |context| context.hold(sealed_home))
        .unwrap()
        .unwrap();
    let retired = table
        .enter(sealed_home, |context| {
            let value = context.alloc::<Number>(|writer| writer.value(41));
            context.keep(value)
        })
        .unwrap();
    table
        .release(sealed_home, ReleaseAbsorption::Refused)
        .unwrap();
    table
        .release(holder, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert_eq!(table.sealed.len(), 0);

    let gone = table
        .enter(onlooker, |context| context.redeem(retired).err())
        .unwrap();
    assert_eq!(gone, Some(RedeemError::Gone));
}

#[test]
fn a_birth_hold_entitles_a_child_to_its_parents_resident() {
    let mut table: CellTable<Owned> = CellTable::new(4, pin);
    let parent = table.create(None, None).unwrap();
    let child = table.create(Some(parent), None).unwrap();

    let kept = table
        .enter(parent, |context| {
            let value = context.alloc::<Number>(|writer| writer.value(41));
            context.keep(value)
        })
        .unwrap();

    // The birth row is a claim on the parent's storage in its own right: the child took no pin
    // hold, and the parent's row names nothing of the child's.
    assert!(!table.holds(child, parent));
    let read = table
        .enter(child, |context| {
            *context
                .read(&context.redeem(kept).expect("a child is entitled"))
                .value()
        })
        .unwrap();
    assert_eq!(read, 41);

    // A birth row has no sealed half, so a declared death leaves the parent resident in the slab
    // with its storage intact — and the child's claim outlives the death.
    table
        .release(parent, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert_eq!(state_of(&table, parent), SlotState::Dead);
    let read = table
        .enter(child, |context| {
            *context
                .read(&context.redeem(kept).expect("the storage is still there"))
                .value()
        })
        .unwrap();
    assert_eq!(read, 41);
}

#[test]
fn a_value_redeemed_from_a_sealed_cell_can_be_kept_again() {
    let mut table: CellTable<Owned> = CellTable::new(4, pin);
    let producer = table.create(None, None).unwrap();
    let middle = table.create(None, None).unwrap();
    let end = table.create(None, None).unwrap();

    table
        .enter(middle, |context| context.hold(producer))
        .unwrap()
        .unwrap();
    table
        .enter(end, |context| context.hold(middle))
        .unwrap()
        .unwrap();
    let first = table
        .enter(producer, |context| {
            let value = context.alloc::<Number>(|writer| writer.value(41));
            context.keep(value)
        })
        .unwrap();
    table.release(producer, ReleaseAbsorption::Refused).unwrap();
    let sealed_id = table.sealed.ids().next().unwrap();

    // A carrier redeemed out of a sealed cell is a carrier like any other: keeping it registers its
    // sealed-cell-only reach in the redeeming cell's own table.
    let again = table
        .enter(middle, |context| {
            let carrier = context
                .redeem(first)
                .expect("the middle holds the sealed cell");
            context.keep(carrier)
        })
        .unwrap();
    assert!(resident_reach(&table, middle.slot(), 0).names_sealed(sealed_id));
    let read = table
        .enter(middle, |context| {
            *context
                .read(&context.redeem(again).expect("its own resident"))
                .value()
        })
        .unwrap();
    assert_eq!(read, 41);

    // The re-keeping cell now seals in turn, and the key forwards to its sealed cell.
    table.release(middle, ReleaseAbsorption::Refused).unwrap();
    let outer = table.sealed_holds[end.slot() as usize]
        .iter()
        .next()
        .expect("the end holds the sealed cell the middle sealed into");
    let read = table
        .enter(end, |context| {
            let carrier = context
                .redeem(again)
                .expect("the end holds the outer sealed cell");
            assert!(carrier.reach().names_sealed(outer));
            *context.read(&carrier).value()
        })
        .unwrap();
    assert_eq!(read, 41);
}

#[test]
fn the_continuation_interns_its_reach_like_any_other_keep() {
    let mut table: CellTable<Borrowed> = CellTable::new(4, pin);
    let cell = table.create(None, None).unwrap();
    let over = table.create(None, None).unwrap();

    let capturing = |table: &mut CellTable<Borrowed>, value: u32| {
        table
            .enter(cell, |context| {
                context.continuation();
                let carrier = context
                    .alloc_into::<Number, Number>(over, &[], |writer, _| writer.value(value))
                    .unwrap();
                context.store_successor_capturing(&[operand(&carrier)], |_writer, views| {
                    pinned(&views[0])
                });
            })
            .unwrap();
    };

    capturing(&mut table, 41);
    assert_eq!(
        table.slots[cell.slot() as usize].continuation_reach_index,
        Some(0)
    );
    assert_eq!(table.slots[cell.slot() as usize].reaches.len(), 1);
    let stored = continuation_reach_index(&table, cell);
    assert!(stored.names(over.slot()));
    assert!(stored.names(cell.slot()));

    // A continuation that captures nothing reaches nothing, and reaching nothing takes no entry:
    // the cell stops naming one rather than emptying the entry it named, which is what makes an
    // entry immutable content.
    table
        .enter(cell, |context| {
            context.continuation();
            context.store_successor(&ANCHOR);
        })
        .unwrap();
    assert_eq!(
        table.slots[cell.slot() as usize].continuation_reach_index,
        None
    );

    // And the capturing store comes back to the entry it minted the first time, because its reach
    // is the same reach. Alternating for a whole run costs that one entry and nothing more.
    for value in 0..8 {
        capturing(&mut table, value);
        assert_eq!(
            table.slots[cell.slot() as usize].continuation_reach_index,
            Some(0)
        );
        table
            .enter(cell, |context| {
                context.continuation();
                context.store_successor(&ANCHOR);
            })
            .unwrap();
    }
    assert_eq!(
        table.slots[cell.slot() as usize].reaches.len(),
        1,
        "a table holds one entry per distinct reach, not one per store"
    );

    capturing(&mut table, 7);
    let read = table
        .enter(cell, |context| *context.continuation().unwrap().value())
        .unwrap();
    assert_eq!(read, 7);
}

#[test]
fn keeping_the_same_reach_twice_takes_one_entry_and_both_keys_redeem() {
    let mut table: CellTable<Owned> = CellTable::new(4, pin);
    let cell = table.create(None, None).unwrap();

    let kept = table
        .enter(cell, |context| {
            let mut kept = Vec::new();
            for value in 0..16 {
                let carrier = context.alloc::<Number>(|writer| writer.value(value));
                kept.push(context.keep(carrier));
            }
            kept
        })
        .unwrap();

    // Sixteen values, one reach: each is homed in the executing cell and reaches nothing else, so
    // every keep interns to the entry the first one minted.
    assert_eq!(
        table.slots[cell.slot() as usize].reaches.len(),
        1,
        "keeps of one shape share one entry"
    );

    // Sharing an entry is invisible at the door: every key still redeems, and to its own value.
    let read = table
        .enter(cell, |context| {
            kept.into_iter()
                .map(|resident| {
                    let carrier = context.redeem(resident).expect("the home is executing");
                    *context.read(&carrier).value()
                })
                .collect::<Vec<u32>>()
        })
        .unwrap();
    assert_eq!(read, (0..16).collect::<Vec<u32>>());
}
