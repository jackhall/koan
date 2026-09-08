//! Values with reach: the region doors, the mint OR into the pin relation, the reclaim gate over
//! both relations, and the ring detector.

use super::super::*;
use super::{
    ANCHOR, Borrowed, Number, Owned, continuation_reach_index, operand, pin, pinned, state_of,
};

#[test]
fn a_value_allocated_in_the_executing_cell_reaches_only_that_cell() {
    let mut graph: CellGraph<Owned> = CellGraph::new(4, pin);
    let cell = graph.create(None, None).unwrap();

    let read = graph
        .enter(cell, |context| {
            let value = context.alloc::<Number>(|writer| writer.value(41));
            assert!(value.reach().names(cell.slot()));
            *context.read(&value).value()
        })
        .unwrap();

    assert_eq!(read, 41);
    // The self rule: a cell that held itself alive could never reach a zero hold count.
    assert!(!graph.holds(cell, cell));
    assert!(graph.slots[cell.slot() as usize].region.is_some());
}

#[test]
fn a_cell_that_never_allocates_mints_no_region() {
    let mut graph: CellGraph<Owned> = CellGraph::new(2, pin);
    let cell = graph.create(None, None).unwrap();
    graph.enter(cell, |context| context.cell()).unwrap();
    assert!(graph.slots[cell.slot() as usize].region.is_none());
}

#[test]
fn placing_a_value_into_another_cell_mints_that_cell_a_hold_on_its_reach() {
    let mut graph: CellGraph<Owned> = CellGraph::new(4, pin);
    let producer = graph.create(None, None).unwrap();
    let consumer = graph.create(None, None).unwrap();

    let read = graph
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
    assert!(graph.holds(consumer, producer));
    assert!(!graph.holds(consumer, consumer));
    assert!(!graph.holds(producer, consumer));
}

#[test]
fn a_held_cell_leaves_the_slab_at_its_death_and_its_sealed_cell_goes_with_its_holder() {
    let mut graph: CellGraph<Owned> = CellGraph::new(4, pin);
    let holder = graph.create(None, None).unwrap();
    let held = graph.create(None, None).unwrap();

    graph
        .enter(holder, |context| context.hold(held))
        .unwrap()
        .unwrap();
    assert!(graph.holds(holder, held));

    // The slot comes straight back: retention lives in the sealed tier, never in the slab.
    graph.release(held, ReleaseAbsorption::Refused).unwrap();
    assert_eq!(state_of(&graph, held), SlabState::Free);
    assert_eq!(graph.sealed.len(), 1);
    let id = graph.sealed.ids().next().unwrap();
    assert!(graph.sealed_holds[holder.slot() as usize].contains(id));
    assert!(!graph.holds(holder, held));

    graph
        .release(holder, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert_eq!(graph.sealed.len(), 0);
    assert_eq!(graph.free.len(), 4);
}

/// Chunks enough to spill a fresh `Bump` past the one it starts with, so the growth the borrow
/// survives is a real chunk allocation rather than a bump of the same block's cursor.
const GROWTH: usize = if cfg!(miri) { 256 } else { 4096 };

#[test]
fn a_reattached_borrow_survives_the_live_region_it_names_growing_under_it() {
    let mut graph: CellGraph<Borrowed> = CellGraph::new(4, pin);
    let keeper = graph.create(None, None).unwrap();
    let host = graph.create(None, None).unwrap();

    // The continuation borrows into a cell that stays live, so nothing detaches: the referent is
    // chunks the host still owns, and the host keeps allocating into them.
    graph
        .enter(keeper, |context| {
            let value = context
                .alloc_into::<Number, Number>(host, &[], |writer, _| writer.value(41))
                .unwrap();
            context
                .store_successor_capturing(&[operand(&value)], |_writer, views| pinned(&views[0]));
        })
        .unwrap();

    let read = graph
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
    let mut graph: CellGraph<Owned> = CellGraph::new(4, pin);
    let holder = graph.create(None, None).unwrap();
    let other = graph.create(None, None).unwrap();
    graph.release(other, ReleaseAbsorption::IntoHolder).unwrap();

    let refusal = graph.enter(holder, |context| context.hold(other)).unwrap();
    assert_eq!(refusal, Err(Stale(other)));
    assert!(!graph.holds(holder, other));
}

#[test]
fn a_ring_an_outside_holder_keeps_from_every_merge_is_reported_and_leaks() {
    let mut graph: CellGraph<Owned> = CellGraph::new(4, pin);
    let first = graph.create(None, None).unwrap();
    let second = graph.create(None, None).unwrap();
    // A bystander holding both sides keeps each count above the one a merge needs, so this ring
    // survives to the tier. A ring with no outside holder dissolves instead — see
    // `absorption::a_two_cell_ring_dissolves_when_one_side_dies`.
    let bystander = graph.create(None, None).unwrap();

    graph
        .enter(first, |context| context.hold(second))
        .unwrap()
        .unwrap();
    graph
        .enter(second, |context| context.hold(first))
        .unwrap()
        .unwrap();
    graph
        .enter(bystander, |context| {
            context.hold(first).unwrap();
            context.hold(second)
        })
        .unwrap()
        .unwrap();

    let ring = graph
        .debug_ring_from(HoldNode::Slab(first))
        .expect("the hold graph has a cycle");
    assert_eq!(ring.len(), 2);
    assert!(ring.contains(&HoldNode::Slab(first)) && ring.contains(&HoldNode::Slab(second)));

    // Every death is declared, and the ring moves into the sealed tier intact: each sealed cell
    // holds the other, so neither count ever reaches zero. A ring is a leak, never a dangle.
    graph.release(first, ReleaseAbsorption::IntoHolder).unwrap();
    graph
        .release(second, ReleaseAbsorption::IntoHolder)
        .unwrap();
    graph
        .release(bystander, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert_eq!(graph.free.len(), 4);
    assert_eq!(graph.sealed.len(), 2);

    let sealed_ring = graph
        .debug_ring_from(HoldNode::Sealed(graph.sealed.ids().next().unwrap()))
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
    let mut graph: CellGraph<Owned> = CellGraph::new(4, pin);
    let first = graph.create(None, None).unwrap();
    let second = graph.create(None, None).unwrap();
    let third = graph.create(None, None).unwrap();

    graph
        .enter(first, |context| context.hold(second))
        .unwrap()
        .unwrap();
    graph
        .enter(second, |context| context.hold(third))
        .unwrap()
        .unwrap();

    assert!(graph.debug_ring_from(HoldNode::Slab(first)).is_none());
}

// The doors a value crosses steps through: `keep` puts a carrier down in its home cell's reach
// graph, and `redeem` takes it back up in a later step of a cell entitled to that storage. See
// [design/cellgraph.md § Passing values between cells](../../../design/cellgraph.md).

/// The one entry a cell's reach table holds, by the index a key names.
fn dormant_reach<C: Reattachable>(graph: &CellGraph<C>, slot: u32, index: u32) -> &GraphReach<1> {
    graph.slots[slot as usize]
        .reaches
        .get(index)
        .expect("the entry the key names is in the reach table")
}

#[test]
fn push_completes_a_value_built_into_the_consumer_is_read_in_its_own_step() {
    let mut graph: CellGraph<Owned> = CellGraph::new(4, pin);
    let consumer = graph.create(None, None).unwrap();
    let producer = graph.create(None, None).unwrap();

    // The push shape: the producer builds straight into the consumer's region, so the value's home
    // is the consumer and the producer's own column stays empty.
    let kept = graph
        .enter(producer, |context| {
            let placed = context
                .alloc_into::<Number, Number>(consumer, &[], |writer, _| writer.value(41))
                .unwrap();
            context.keep(placed)
        })
        .unwrap();
    assert_eq!(graph.slots[consumer.slot() as usize].reaches.len(), 1);
    assert!(dormant_reach(&graph, consumer.slot(), 0).names(consumer.slot()));
    assert_eq!(graph.relocations(), 0);

    // Nothing reaches the producer, so its death is a reclamation: the slot comes straight back
    // and the map it never entered stays empty.
    graph
        .release(producer, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert_eq!(state_of(&graph, producer), SlabState::Free);
    assert_eq!(graph.relocations(), 0);

    let read = graph
        .enter(consumer, |context| {
            let carrier = context
                .redeem(kept)
                .expect("the home redeems its own dormant carrier");
            *context.read(&carrier).value()
        })
        .unwrap();
    assert_eq!(read, 41);
}

#[test]
fn pull_completes_after_the_producer_seals() {
    let mut graph: CellGraph<Owned> = CellGraph::new(4, pin);
    let consumer = graph.create(None, None).unwrap();
    let producer = graph.create(None, None).unwrap();

    graph
        .enter(consumer, |context| context.hold(producer))
        .unwrap()
        .unwrap();
    let kept = graph
        .enter(producer, |context| {
            let value = context.alloc::<Number>(|writer| writer.value(41));
            context.keep(value)
        })
        .unwrap();

    // The pull shape: the producer dies still held, so its storage seals and the key it minted
    // forwards to the sealed cell rather than stopping resolving.
    graph.release(producer, ReleaseAbsorption::Refused).unwrap();
    let id = graph.sealed.ids().next().unwrap();
    assert_eq!(graph.relocation_of(producer), Some(SlabForward::Sealed(id)));
    assert_eq!(graph.lineage_of(id), vec![producer]);

    let read = graph
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
    graph
        .release(consumer, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert_eq!(graph.sealed.len(), 0);
    assert_eq!(graph.relocations(), 0);
}

#[test]
fn pull_completes_after_the_producer_is_absorbed_into_the_consumer() {
    let mut graph: CellGraph<Owned> = CellGraph::new(4, pin);
    let consumer = graph.create(None, None).unwrap();
    let producer = graph.create(None, None).unwrap();

    graph
        .enter(consumer, |context| context.hold(producer))
        .unwrap()
        .unwrap();
    let kept = graph
        .enter(producer, |context| {
            let value = context.alloc::<Number>(|writer| writer.value(41));
            context.keep(value)
        })
        .unwrap();

    // The uniquely held producer folds into its holder rather than minting a sealed cell, and its
    // dormant carriers' masks move with the storage, re-homed at the consumer's bit.
    graph
        .release(producer, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert_eq!(graph.sealed.len(), 0);
    assert_eq!(
        graph.relocation_of(producer),
        Some(SlabForward::Slab {
            slot: consumer.slot(),
            first_index: 0,
        })
    );
    let migrated = dormant_reach(&graph, consumer.slot(), 0);
    assert!(migrated.names(consumer.slot()));
    assert!(!migrated.names(producer.slot()));

    // A second dormant carrier kept by the consumer itself lands past the migrated one, so the two
    // keys name different entries and both redeem.
    let own = graph
        .enter(consumer, |context| {
            let value = context.alloc::<Number>(|writer| writer.value(9));
            context.keep(value)
        })
        .unwrap();
    let read = graph
        .enter(consumer, |context| {
            let pulled = context.redeem(kept).expect("the absorbing cell answers");
            let mine = context
                .redeem(own)
                .expect("the home redeems its own dormant carrier");
            (*context.read(&pulled).value(), *context.read(&mine).value())
        })
        .unwrap();
    assert_eq!(read, (41, 9));
}

#[test]
fn a_dormant_carrier_forwarded_through_two_merges_is_still_found() {
    let mut graph: CellGraph<Owned> = CellGraph::new(4, pin);
    let end = graph.create(None, None).unwrap();
    let middle = graph.create(None, None).unwrap();
    let head = graph.create(None, None).unwrap();

    graph
        .enter(middle, |context| context.hold(head))
        .unwrap()
        .unwrap();
    graph
        .enter(end, |context| context.hold(middle))
        .unwrap()
        .unwrap();
    let kept = graph
        .enter(head, |context| {
            let value = context.alloc::<Number>(|writer| writer.value(41));
            context.keep(value)
        })
        .unwrap();

    // Merge one: into the slab. Merge two: into the tier. The key is rewritten by each, so the
    // chain of single-consumer producers costs the map one entry per merge and none per value.
    graph.release(head, ReleaseAbsorption::IntoHolder).unwrap();
    graph.release(middle, ReleaseAbsorption::Refused).unwrap();
    let id = graph.sealed.ids().next().unwrap();
    assert_eq!(graph.relocation_of(head), Some(SlabForward::Sealed(id)));

    let read = graph
        .enter(end, |context| {
            let carrier = context.redeem(kept).expect("the end holds the sealed cell");
            assert!(carrier.reach().names_sealed(id));
            *context.read(&carrier).value()
        })
        .unwrap();
    assert_eq!(read, 41);

    graph.release(end, ReleaseAbsorption::IntoHolder).unwrap();
    assert_eq!(graph.sealed.len(), 0);
    assert_eq!(graph.relocations(), 0);
    assert_eq!(graph.free.len(), 4);
}

#[test]
fn redeem_refuses_a_cell_that_does_not_hold_the_home() {
    let mut graph: CellGraph<Owned> = CellGraph::new(4, pin);
    let home = graph.create(None, None).unwrap();
    let bystander = graph.create(None, None).unwrap();
    let middle = graph.create(None, None).unwrap();
    let far = graph.create(None, None).unwrap();

    let kept = graph
        .enter(home, |context| {
            let value = context.alloc::<Number>(|writer| writer.value(41));
            context.keep(value)
        })
        .unwrap();

    let refused = graph
        .enter(bystander, |context| context.redeem(kept).err())
        .unwrap();
    assert_eq!(refused, Some(RedeemError::Unheld));

    // A hold is what entitles: with one, the same key answers.
    let read = graph
        .enter(bystander, |context| {
            context.hold(home).unwrap();
            let carrier = context.redeem(kept).expect("the holder is entitled");
            *context.read(&carrier).value()
        })
        .unwrap();
    assert_eq!(read, 41);

    // Entitlement is the direct relation, not its closure: reaching the home only through a cell
    // in between is not a claim on the home's storage.
    graph
        .enter(middle, |context| context.hold(home))
        .unwrap()
        .unwrap();
    graph
        .enter(far, |context| context.hold(middle))
        .unwrap()
        .unwrap();
    let transitive = graph
        .enter(far, |context| context.redeem(kept).err())
        .unwrap();
    assert_eq!(transitive, Some(RedeemError::Unheld));
}

#[test]
fn redeem_refuses_once_the_storage_is_gone() {
    let mut graph: CellGraph<Owned> = CellGraph::new(4, pin);
    let reclaimed = graph.create(None, None).unwrap();
    let onlooker = graph.create(None, None).unwrap();

    let orphan = graph
        .enter(reclaimed, |context| {
            let value = context.alloc::<Number>(|writer| writer.value(41));
            context.keep(value)
        })
        .unwrap();
    // Nothing reached the home, so its death frees the chunks the dormant carrier named.
    graph
        .release(reclaimed, ReleaseAbsorption::IntoHolder)
        .unwrap();
    let gone = graph
        .enter(onlooker, |context| context.redeem(orphan).err())
        .unwrap();
    assert_eq!(gone, Some(RedeemError::Gone));

    // The other way storage goes: sealed, then retired when its last holder leaves.
    let sealed_home = graph.create(None, None).unwrap();
    let holder = graph.create(None, None).unwrap();
    graph
        .enter(holder, |context| context.hold(sealed_home))
        .unwrap()
        .unwrap();
    let retired = graph
        .enter(sealed_home, |context| {
            let value = context.alloc::<Number>(|writer| writer.value(41));
            context.keep(value)
        })
        .unwrap();
    graph
        .release(sealed_home, ReleaseAbsorption::Refused)
        .unwrap();
    graph
        .release(holder, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert_eq!(graph.sealed.len(), 0);

    let gone = graph
        .enter(onlooker, |context| context.redeem(retired).err())
        .unwrap();
    assert_eq!(gone, Some(RedeemError::Gone));
}

#[test]
fn a_birth_hold_entitles_a_child_to_its_parents_dormant_carrier() {
    let mut graph: CellGraph<Owned> = CellGraph::new(4, pin);
    let parent = graph.create(None, None).unwrap();
    let child = graph.create(Some(parent), None).unwrap();

    let kept = graph
        .enter(parent, |context| {
            let value = context.alloc::<Number>(|writer| writer.value(41));
            context.keep(value)
        })
        .unwrap();

    // The birth row is a claim on the parent's storage in its own right: the child took no pin
    // hold, and the parent's row names nothing of the child's.
    assert!(!graph.holds(child, parent));
    let read = graph
        .enter(child, |context| {
            *context
                .read(&context.redeem(kept).expect("a child is entitled"))
                .value()
        })
        .unwrap();
    assert_eq!(read, 41);

    // A birth row has no sealed half, so a declared death leaves the parent undisposed in the slab
    // with its storage intact — and the child's claim outlives the death.
    graph
        .release(parent, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert_eq!(state_of(&graph, parent), SlabState::Dead);
    let read = graph
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
    let mut graph: CellGraph<Owned> = CellGraph::new(4, pin);
    let producer = graph.create(None, None).unwrap();
    let middle = graph.create(None, None).unwrap();
    let end = graph.create(None, None).unwrap();

    graph
        .enter(middle, |context| context.hold(producer))
        .unwrap()
        .unwrap();
    graph
        .enter(end, |context| context.hold(middle))
        .unwrap()
        .unwrap();
    let first = graph
        .enter(producer, |context| {
            let value = context.alloc::<Number>(|writer| writer.value(41));
            context.keep(value)
        })
        .unwrap();
    graph.release(producer, ReleaseAbsorption::Refused).unwrap();
    let sealed_id = graph.sealed.ids().next().unwrap();

    // A carrier redeemed out of a sealed cell is a carrier like any other: keeping it registers its
    // sealed-cell-only reach in the redeeming cell's own reach table.
    let again = graph
        .enter(middle, |context| {
            let carrier = context
                .redeem(first)
                .expect("the middle holds the sealed cell");
            context.keep(carrier)
        })
        .unwrap();
    assert!(dormant_reach(&graph, middle.slot(), 0).names_sealed(sealed_id));
    let read = graph
        .enter(middle, |context| {
            *context
                .read(&context.redeem(again).expect("its own dormant carrier"))
                .value()
        })
        .unwrap();
    assert_eq!(read, 41);

    // The re-keeping cell now seals in turn, and the key forwards to its sealed cell.
    graph.release(middle, ReleaseAbsorption::Refused).unwrap();
    let outer = graph.sealed_holds[end.slot() as usize]
        .iter()
        .next()
        .expect("the end holds the sealed cell the middle sealed into");
    let read = graph
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
    let mut graph: CellGraph<Borrowed> = CellGraph::new(4, pin);
    let cell = graph.create(None, None).unwrap();
    let over = graph.create(None, None).unwrap();

    let capturing = |graph: &mut CellGraph<Borrowed>, value: u32| {
        graph
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

    capturing(&mut graph, 41);
    assert_eq!(
        graph.slots[cell.slot() as usize].continuation_reach_index,
        Some(0)
    );
    assert_eq!(graph.slots[cell.slot() as usize].reaches.len(), 1);
    let stored = continuation_reach_index(&graph, cell);
    assert!(stored.names(over.slot()));
    assert!(stored.names(cell.slot()));

    // A continuation that captures nothing reaches nothing, and reaching nothing takes no entry:
    // the cell stops naming one rather than emptying the entry it named, which is what makes an
    // entry immutable content.
    graph
        .enter(cell, |context| {
            context.continuation();
            context.store_successor(&ANCHOR);
        })
        .unwrap();
    assert_eq!(
        graph.slots[cell.slot() as usize].continuation_reach_index,
        None
    );

    // And the capturing store comes back to the entry it minted the first time, because its reach
    // is the same reach. Alternating for a whole run costs that one entry and nothing more.
    for value in 0..8 {
        capturing(&mut graph, value);
        assert_eq!(
            graph.slots[cell.slot() as usize].continuation_reach_index,
            Some(0)
        );
        graph
            .enter(cell, |context| {
                context.continuation();
                context.store_successor(&ANCHOR);
            })
            .unwrap();
    }
    assert_eq!(
        graph.slots[cell.slot() as usize].reaches.len(),
        1,
        "a reach table holds one entry per distinct reach, not one per store"
    );

    capturing(&mut graph, 7);
    let read = graph
        .enter(cell, |context| *context.continuation().unwrap().value())
        .unwrap();
    assert_eq!(read, 7);
}

#[test]
fn keeping_the_same_reach_twice_takes_one_entry_and_both_keys_redeem() {
    let mut graph: CellGraph<Owned> = CellGraph::new(4, pin);
    let cell = graph.create(None, None).unwrap();

    let kept = graph
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
        graph.slots[cell.slot() as usize].reaches.len(),
        1,
        "keeps of one shape share one entry"
    );

    // Sharing an entry is invisible at the door: every key still redeems, and to its own value.
    let read = graph
        .enter(cell, |context| {
            kept.into_iter()
                .map(|dormant| {
                    let carrier = context.redeem(dormant).expect("the home is executing");
                    *context.read(&carrier).value()
                })
                .collect::<Vec<u32>>()
        })
        .unwrap();
    assert_eq!(read, (0..16).collect::<Vec<u32>>());
}
