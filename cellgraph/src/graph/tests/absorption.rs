//! The three locality merges: a dying cell absorbed into its unique slab holder, a count-1 sealed
//! region absorbed at its holder's seal, and a cell with no slab holder sealing into its single
//! sealed namer. See [../README.md § Locality
//! tactics](../README.md#locality-tactics).
//!
//! Each merge is a sealed cell the tier never mints, so what these tests read is an absence: no id,
//! no index entry, no accessor indirection — and the storage still there, in the bundle that took
//! it.

use proptest::prelude::*;

use super::super::*;
use super::{
    Borrowed, Number, Owned, kept_reach, live_bytes, number_here, one, operand, pin, pinned,
    state_of,
};

/// Bytes a cell's region bundle occupies, or zero for a cell that never allocated.
fn region_bytes<C: Reattachable<'static>>(
    graph: &CellGraph<'static, C>,
    handle: SlabHandle,
) -> usize {
    graph.regions.slab_bytes(handle.slot())
}

/// The one sealed cell in a graph that has exactly one.
fn only_sealed_cell<C: Reattachable<'static>>(graph: &CellGraph<'static, C>) -> SealedId {
    assert_eq!(graph.cells.sealed.len(), 1);
    graph
        .cells
        .sealed
        .ids()
        .next()
        .expect("the tier holds a sealed cell")
}

#[test]
fn an_empty_graph_is_quiescent_and_a_surviving_ring_is_not() {
    let mut graph: CellGraph<'static, Owned> = CellGraph::new(4, pin);
    assert!(graph.is_empty());

    let cell = graph.create(None, None).unwrap();
    assert!(!graph.is_empty());
    graph.release(cell, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());

    // A ring an outside holder kept above every merge's count survives the wind-down, and that is
    // exactly what a non-empty graph after the last release means.
    let first = graph.create(None, None).unwrap();
    let second = graph.create(None, None).unwrap();
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
    for cell in [first, second, bystander] {
        graph.release(cell, ReleaseAbsorption::IntoHolder).unwrap();
    }

    assert!(!graph.is_empty());
    let ring = graph
        .cells
        .debug_ring_from(HoldNode::Sealed(graph.cells.sealed.ids().next().unwrap()))
        .expect("the survivors are a ring");
    assert_eq!(ring.len(), 2);
}

#[test]
fn a_uniquely_held_cell_is_absorbed_into_its_holder_instead_of_sealing() {
    let mut graph: CellGraph<'static, Borrowed> = CellGraph::new(4, pin);
    let consumer = graph.create(None, None).unwrap();
    let producer = graph.create(None, None).unwrap();

    // The consumer pins a value living in the producer's region into its own holds and keeps it
    // as its continuation's capture, so it is the producer's one holder. It bundles the same value
    // into its own region and keeps that, which is the stored mask the merge has to rewrite.
    let kept = graph
        .enter(consumer, |context| {
            let value = context
                .alloc_into::<Number, Number>(producer, &[], |writer, _| {
                    Active::new(one(writer, 41))
                })
                .unwrap();
            let captured =
                context.alloc_here(&[operand(&value)], |_writer, views| pinned(&views[0]));
            context.store_successor(captured);
            let bundled = context
                .alloc_into::<Number, Number>(consumer, &[operand(&value)], |_writer, views| {
                    Active::new(pinned(&views[0]))
                })
                .unwrap();
            context.keep(bundled)
        })
        .unwrap();
    assert!(graph.cells.holds(consumer, producer));
    let producer_bytes = region_bytes(&graph, producer);
    let consumer_bytes = region_bytes(&graph, consumer);
    assert!(producer_bytes > 0);

    graph
        .release(producer, ReleaseAbsorption::IntoHolder)
        .unwrap();

    // No id, no index entry, no sealed cell: the storage is the consumer's own now.
    assert_eq!(graph.cells.sealed.len(), 0);
    assert_eq!(state_of(&graph, producer), SlabState::Free);
    assert!(!graph.cells.holds(consumer, producer));
    assert_eq!(
        region_bytes(&graph, consumer),
        consumer_bytes + producer_bytes
    );

    // The merge rewrote the consumer's stored mask: the dead cell's bit became the holder's, and
    // nothing sealed, so the mask stays a plain slab row over live cells.
    let stored = kept_reach(&graph, &kept);
    assert!(stored.names(consumer.slot()));
    assert!(!stored.names(producer.slot()));
    assert_eq!(stored.sealed().len(), 0);

    // The bump moved into the consumer's bundle without moving a chunk byte, so the borrow reads
    // the same address.
    let value = graph
        .enter(consumer, |context| *context.continuation().unwrap())
        .unwrap();
    assert_eq!(value, 41);
}

#[test]
fn absorption_carries_the_dead_cells_holds_onto_its_holder() {
    let mut graph: CellGraph<'static, Owned> = CellGraph::new(6, pin);
    let consumer = graph.create(None, None).unwrap();
    let producer = graph.create(None, None).unwrap();
    let reached = graph.create(None, None).unwrap();
    let shared = graph.create(None, None).unwrap();
    let alone = graph.create(None, None).unwrap();

    graph
        .enter(consumer, |context| {
            context.hold(shared).unwrap();
            context.hold(producer)
        })
        .unwrap()
        .unwrap();
    graph
        .enter(producer, |context| {
            context.hold(reached).unwrap();
            context.hold(shared).unwrap();
            context.hold(alone)
        })
        .unwrap()
        .unwrap();

    graph.release(shared, ReleaseAbsorption::Refused).unwrap();
    let shared_id = only_sealed_cell(&graph);
    graph.release(alone, ReleaseAbsorption::Refused).unwrap();
    let alone_id = graph
        .cells
        .sealed
        .ids()
        .find(|id| *id != shared_id)
        .expect("the second seal minted a sealed cell");
    assert_eq!(graph.cells.sealed.get(shared_id).unwrap().holders, 2);
    assert_eq!(graph.cells.sealed.get(alone_id).unwrap().holders, 1);

    graph
        .release(producer, ReleaseAbsorption::IntoHolder)
        .unwrap();

    // The slab half arrives through the standard mint.
    assert!(graph.cells.pins.test(consumer.slot(), reached.slot()));
    // The sparse half changes holder rather than count where the consumer did not already hold it,
    // and where it did, the dead cell's duplicate hold simply goes.
    assert!(graph.cells.sealed_holds[consumer.slot() as usize].contains(alone_id));
    assert_eq!(graph.cells.sealed.get(alone_id).unwrap().holders, 1);
    assert_eq!(graph.cells.sealed.get(shared_id).unwrap().holders, 1);
}

#[test]
fn a_refused_release_seals_as_before() {
    let mut graph: CellGraph<'static, Borrowed> = CellGraph::new(4, pin);
    let consumer = graph.create(None, None).unwrap();
    let producer = graph.create(None, None).unwrap();

    let kept = graph
        .enter(consumer, |context| {
            let value = context
                .alloc_into::<Number, Number>(producer, &[], |writer, _| {
                    Active::new(one(writer, 41))
                })
                .unwrap();
            let captured =
                context.alloc_here(&[operand(&value)], |_writer, views| pinned(&views[0]));
            context.store_successor(captured);
            let bundled = context
                .alloc_into::<Number, Number>(consumer, &[operand(&value)], |_writer, views| {
                    Active::new(pinned(&views[0]))
                })
                .unwrap();
            context.keep(bundled)
        })
        .unwrap();

    graph.release(producer, ReleaseAbsorption::Refused).unwrap();

    // The very shape a merge would have collapsed, sealed instead: the embedder's refusal is the
    // whole difference.
    let id = only_sealed_cell(&graph);
    assert_eq!(graph.cells.sealed.get(id).unwrap().holders, 1);

    assert!(kept_reach(&graph, &kept).names_sealed(id));
    let value = graph
        .enter(consumer, |context| *context.continuation().unwrap())
        .unwrap();
    assert_eq!(value, 41);
}

#[test]
fn an_undisposed_dead_holder_absorbs_too() {
    let mut graph: CellGraph<'static, Owned> = CellGraph::new(4, pin);
    let holder = graph.create(None, None).unwrap();
    let child = graph.create(Some(holder), None).unwrap();
    let held = graph.create(None, None).unwrap();

    graph
        .enter(holder, |context| context.hold(held))
        .unwrap()
        .unwrap();

    // The holder's death is declared, but its child's birth row keeps it in the slab. Its pin row
    // is still a maintained row, so it is still a merge target: "live holder" means the slab tier,
    // not a cell that may still execute.
    graph
        .release(holder, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert_eq!(state_of(&graph, holder), SlabState::Dead);

    graph.release(held, ReleaseAbsorption::IntoHolder).unwrap();
    assert_eq!(graph.cells.sealed.len(), 0);
    assert_eq!(state_of(&graph, held), SlabState::Free);

    // Absorbing into a dead-but-undisposed cell only brings forward the fold its own disposal would
    // have performed: when the child goes, the whole bundle goes with the holder.
    graph.release(child, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

#[test]
fn a_two_cell_ring_dissolves_when_one_side_dies() {
    let mut graph: CellGraph<'static, Owned> = CellGraph::new(4, pin);
    let first = graph.create(None, None).unwrap();
    let second = graph.create(None, None).unwrap();

    graph
        .enter(first, |context| context.hold(second))
        .unwrap()
        .unwrap();
    graph
        .enter(second, |context| context.hold(first))
        .unwrap()
        .unwrap();

    // The merge runs even though the source held its own target: the mint's and-not is where the
    // hold on itself lands, so what would have been a sealed ring is a cell holding nothing.
    graph.release(first, ReleaseAbsorption::IntoHolder).unwrap();
    assert_eq!(graph.cells.sealed.len(), 0);
    assert!(!graph.cells.holds(second, first));
    assert!(!graph.cells.holds(second, second));

    graph
        .release(second, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert!(graph.is_empty());
    assert_eq!(graph.cells.free.len(), 4);
}

#[test]
fn a_seal_absorbs_its_count_one_sealed_holds() {
    let mut graph: CellGraph<'static, Owned> = CellGraph::new(6, pin);
    let top = graph.create(None, None).unwrap();
    let other = graph.create(None, None).unwrap();
    let middle = graph.create(None, None).unwrap();
    let base = graph.create(None, None).unwrap();
    let reached = graph.create(None, None).unwrap();

    graph
        .enter(base, |context| {
            number_here(context, 1);
            context.hold(reached)
        })
        .unwrap()
        .unwrap();
    graph
        .enter(middle, |context| {
            number_here(context, 2);
            context.hold(base)
        })
        .unwrap()
        .unwrap();
    // Two holders, so the middle cell seals rather than absorbing into one of them.
    for holder in [top, other] {
        graph
            .enter(holder, |context| context.hold(middle))
            .unwrap()
            .unwrap();
    }
    let base_bytes = region_bytes(&graph, base);
    let middle_bytes = region_bytes(&graph, middle);

    graph.release(base, ReleaseAbsorption::Refused).unwrap();
    let base_id = only_sealed_cell(&graph);
    assert!(graph.cells.naming[reached.slot() as usize].contains(base_id));

    graph
        .release(middle, ReleaseAbsorption::IntoHolder)
        .unwrap();

    // The base's sealed cell had one holder — the sealing cell — so the seal folds it in rather
    // than leaving a chain of two sealed cells with one indirection each.
    let middle_id = only_sealed_cell(&graph);
    assert_ne!(middle_id, base_id);
    let sealed_cell = graph.cells.sealed.get(middle_id).unwrap();
    assert_eq!(sealed_cell.holders, 2);
    assert!(sealed_cell.aggregate.names(reached.slot()));
    assert!(!sealed_cell.aggregate.names_sealed(base_id));
    assert_eq!(sealed_cell.retained_bytes(), base_bytes + middle_bytes);
    assert!(!graph.cells.naming[reached.slot() as usize].contains(base_id));
    assert!(graph.cells.naming[reached.slot() as usize].contains(middle_id));

    graph.release(top, ReleaseAbsorption::IntoHolder).unwrap();
    assert_eq!(graph.cells.sealed.len(), 1);
    graph.release(other, ReleaseAbsorption::IntoHolder).unwrap();
    assert_eq!(graph.cells.sealed.len(), 0);
    graph
        .release(reached, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert!(graph.is_empty());
}

#[test]
fn seal_time_absorption_follows_a_chain_whose_counts_dropped() {
    let mut graph: CellGraph<'static, Owned> = CellGraph::new(6, pin);
    let first_keeper = graph.create(None, None).unwrap();
    let second_keeper = graph.create(None, None).unwrap();
    let top = graph.create(None, None).unwrap();
    let middle = graph.create(None, None).unwrap();
    let base = graph.create(None, None).unwrap();
    let extra = graph.create(None, None).unwrap();

    graph
        .enter(middle, |context| context.hold(base))
        .unwrap()
        .unwrap();
    graph
        .enter(extra, |context| context.hold(base))
        .unwrap()
        .unwrap();
    graph
        .enter(top, |context| context.hold(middle))
        .unwrap()
        .unwrap();
    for keeper in [first_keeper, second_keeper] {
        graph
            .enter(keeper, |context| context.hold(top))
            .unwrap()
            .unwrap();
    }

    graph.release(base, ReleaseAbsorption::Refused).unwrap();
    // Two holders, so the middle cell's own seal finds nothing to absorb.
    graph.release(middle, ReleaseAbsorption::Refused).unwrap();
    assert_eq!(graph.cells.sealed.len(), 2);

    // The extra holder goes, and the base's count drops to one — but nothing seals here, so the
    // candidate is only noticed at the next seal.
    graph.release(extra, ReleaseAbsorption::IntoHolder).unwrap();
    assert_eq!(graph.cells.sealed.len(), 2);

    graph.release(top, ReleaseAbsorption::IntoHolder).unwrap();

    // The worklist is what makes this one sealed cell rather than three: absorbing the middle
    // sealed cell transfers the base's id onto the new one, where its count of one qualifies it in
    // turn.
    let id = only_sealed_cell(&graph);
    assert_eq!(graph.cells.sealed.get(id).unwrap().holders, 2);

    for keeper in [first_keeper, second_keeper] {
        graph
            .release(keeper, ReleaseAbsorption::IntoHolder)
            .unwrap();
    }
    assert!(graph.is_empty());
}

#[test]
fn a_count_one_sealed_cell_held_by_a_live_cell_stays_sealed() {
    let mut graph: CellGraph<'static, Owned> = CellGraph::new(4, pin);
    let holder = graph.create(None, None).unwrap();
    let extra = graph.create(None, None).unwrap();
    let held = graph.create(None, None).unwrap();

    graph
        .enter(holder, |context| {
            number_here(context, 1);
            context.hold(held)
        })
        .unwrap()
        .unwrap();
    graph
        .enter(held, |context| {
            number_here(context, 2);
            context.hold(extra)
        })
        .unwrap()
        .unwrap();
    graph
        .enter(extra, |context| context.hold(held))
        .unwrap()
        .unwrap();

    graph.release(held, ReleaseAbsorption::Refused).unwrap();
    let id = only_sealed_cell(&graph);
    assert_eq!(graph.cells.sealed.get(id).unwrap().holders, 2);
    let holder_bytes = region_bytes(&graph, holder);
    let sealed_bytes = graph.cells.sealed.get(id).unwrap().retained_bytes();
    let slab_bytes = live_bytes(&graph, 4);
    assert!(sealed_bytes > 0);

    graph.release(extra, ReleaseAbsorption::IntoHolder).unwrap();

    // A count of one is not by itself a merge: storage that has already sealed never re-enters the
    // live tier, so the sealed cell waits for the cascade instead of folding into the live cell.
    // The provenance is read from the two tiers' byte totals rather than tracked through the
    // release — the sealed cell keeps every byte it had, and the whole slab tier is no larger than
    // it was.
    assert_eq!(graph.cells.sealed.get(id).unwrap().holders, 1);
    assert_eq!(
        graph.cells.sealed.get(id).unwrap().retained_bytes(),
        sealed_bytes
    );
    assert_eq!(region_bytes(&graph, holder), holder_bytes);
    assert!(live_bytes(&graph, 4) <= slab_bytes);

    graph
        .release(holder, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert!(graph.is_empty());
}

#[test]
fn a_cell_with_a_single_sealed_namer_seals_into_it() {
    let mut graph: CellGraph<'static, Borrowed> = CellGraph::new(4, pin);
    let keeper = graph.create(None, None).unwrap();
    let namer = graph.create(None, None).unwrap();
    let dying = graph.create(None, None).unwrap();
    let reached = graph.create(None, None).unwrap();

    // The keeper's continuation lives over the namer's storage, and the namer holds the dying cell
    // — so once the namer seals, the dying cell's only name is that sealed cell's aggregate.
    graph
        .enter(namer, |context| context.hold(dying))
        .unwrap()
        .unwrap();
    let kept = graph
        .enter(keeper, |context| {
            let value = context
                .alloc_into::<Number, Number>(namer, &[], |writer, _| Active::new(one(writer, 41)))
                .unwrap();
            let captured =
                context.alloc_here(&[operand(&value)], |_writer, views| pinned(&views[0]));
            context.store_successor(captured);
            let bundled = context
                .alloc_into::<Number, Number>(keeper, &[operand(&value)], |_writer, views| {
                    Active::new(pinned(&views[0]))
                })
                .unwrap();
            context.keep(bundled)
        })
        .unwrap();
    graph
        .enter(dying, |context| {
            number_here(context, 7);
            context.hold(reached)
        })
        .unwrap()
        .unwrap();
    let dying_bytes = region_bytes(&graph, dying);
    assert!(dying_bytes > 0);

    graph.release(namer, ReleaseAbsorption::Refused).unwrap();
    let id = only_sealed_cell(&graph);
    assert!(
        graph
            .cells
            .sealed
            .get(id)
            .unwrap()
            .aggregate
            .names(dying.slot())
    );
    let namer_bytes = graph.cells.sealed.get(id).unwrap().retained_bytes();

    graph.release(dying, ReleaseAbsorption::IntoHolder).unwrap();

    // No second sealed cell: the dying cell's storage and holds go into the aggregate that already
    // named it, and the slots its row named trade its bit for the sealed cell's name.
    assert_eq!(graph.cells.sealed.len(), 1);
    assert_eq!(state_of(&graph, dying), SlabState::Free);
    let sealed_cell = graph.cells.sealed.get(id).unwrap();
    assert_eq!(sealed_cell.holders, 1);
    assert!(!sealed_cell.aggregate.names(dying.slot()));
    assert!(sealed_cell.aggregate.names(reached.slot()));
    assert_eq!(sealed_cell.retained_bytes(), namer_bytes + dying_bytes);
    assert!(graph.cells.naming[reached.slot() as usize].contains(id));

    // The read still goes through the sealed cell, whose bundle grew a bump under the borrow.
    assert!(kept_reach(&graph, &kept).names_sealed(id));
    let value = graph
        .enter(keeper, |context| *context.continuation().unwrap())
        .unwrap();
    assert_eq!(value, 41);
}

/// Absorb a uniquely held producer into its consumer, and report the maintenance the merge
/// performed.
///
/// The producer holds `reached` live cells, `shared` sealed regions the consumer already holds,
/// and `alone` sealed regions only it holds — so the hold set varies in both halves and in whether
/// each sealed id transfers or duplicates, while `dormant` varies what the region stores.
fn absorb_work_for(dormant: usize, reached: u32, shared: u32, alone: u32) -> u64 {
    let mut graph: CellGraph<'static, Owned> = CellGraph::new(2 + reached + shared + alone, pin);
    let consumer = graph.create(None, None).unwrap();
    let producer = graph.create(None, None).unwrap();
    let mut make = |count| {
        (0..count)
            .map(|_| graph.create(None, None).unwrap())
            .collect()
    };
    let reached_cells: Vec<SlabHandle> = make(reached);
    let shared_cells: Vec<SlabHandle> = make(shared);
    let alone_cells: Vec<SlabHandle> = make(alone);

    graph
        .enter(producer, |context| {
            for value in 0..dormant {
                number_here(context, value as u32);
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
    graph
        .enter(consumer, |context| {
            context.hold(producer).unwrap();
            for cell in &shared_cells {
                context.hold(*cell).unwrap();
            }
        })
        .unwrap();
    // Refused, so each becomes a sealed cell rather than absorbing into the producer first.
    for cell in shared_cells.iter().chain(&alone_cells) {
        graph.release(*cell, ReleaseAbsorption::Refused).unwrap();
    }

    let before = graph.cells.seal_work;
    graph
        .release(producer, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert_eq!(graph.cells.sealed.len(), (shared + alone) as usize);
    graph.cells.seal_work - before
}

/// A dormant-value count large enough that work proportional to storage could not match the lean
/// run's.
fn heavy() -> std::ops::Range<usize> {
    if cfg!(miri) { 64..128 } else { 1_600..2_000 }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: if cfg!(miri) { 4 } else { (ProptestConfig::default().cases / 4).max(64) },
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
    let mut graph: CellGraph<'static, Owned> = CellGraph::new(4, pin);
    let first = graph.create(None, None).unwrap();
    let second = graph.create(None, None).unwrap();
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
        .enter(bystander, |context| context.hold(first))
        .unwrap()
        .unwrap();

    // Two holders, so the first cell seals; its aggregate names the second, and the second holds
    // the sealed cell.
    graph.release(first, ReleaseAbsorption::IntoHolder).unwrap();
    let id = only_sealed_cell(&graph);
    assert_eq!(graph.cells.sealed.get(id).unwrap().holders, 2);
    assert!(
        graph
            .cells
            .sealed
            .get(id)
            .unwrap()
            .aggregate
            .names(second.slot())
    );

    graph
        .release(bystander, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert_eq!(graph.cells.sealed.get(id).unwrap().holders, 1);

    // The second cell's only namer is the sealed cell it itself holds: the fold turns that hold
    // into a self-hold, the count reaches zero, and the ring is freed rather than leaked.
    graph
        .release(second, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert!(graph.is_empty());
    assert_eq!(graph.cells.free.len(), 4);
}

#[test]
fn a_seal_that_absorbs_every_holder_it_had_reclaims_itself() {
    let mut graph: CellGraph<'static, Owned> = CellGraph::new(4, pin);
    let held = graph.create(None, None).unwrap();
    let first = graph.create(None, None).unwrap();
    let second = graph.create(None, None).unwrap();

    graph
        .enter(held, |context| {
            context.hold(first).unwrap();
            context.hold(second)
        })
        .unwrap()
        .unwrap();
    for namer in [first, second] {
        graph
            .enter(namer, |context| context.hold(held))
            .unwrap()
            .unwrap();
        graph.release(namer, ReleaseAbsorption::Refused).unwrap();
    }
    assert_eq!(graph.cells.sealed.len(), 2);

    // Two namers and no slab holder, so this is a plain seal — and both namers are count-1 regions
    // the new sealed cell holds, so both fold in. Each fold turns a hold on the new sealed cell
    // into a self-hold, and the second takes its count to zero: the whole ring goes in one release.
    graph.release(held, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
    assert_eq!(graph.cells.free.len(), 4);
}
