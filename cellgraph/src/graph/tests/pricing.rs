//! The price queries an embedder decides copy-versus-hold with: what a hold on a sealed cell keeps
//! alive, the slice of that only one of several candidates reaches, what a live cart has accreted,
//! and how full the two tiers are. See [../README.md § Bounding the two
//! tiers](../README.md#bounding-the-two-tiers).
//!
//! Every shape here keeps a sealed cell above the count a locality merge would absorb it at — an
//! extra live holder, or a refused release — because a merge that fires leaves nothing to price.

use super::super::*;
use super::{Owned, number_here, pin};

/// Give a cell a region of its own, so it prices at more than nothing.
fn allocate(graph: &mut CellGraph<Owned>, cell: SlabHandle) {
    graph
        .enter(cell, |context| {
            number_here(context, 1);
        })
        .unwrap();
}

/// Mint a bare pin hold from one live cell onto another.
fn hold(graph: &mut CellGraph<Owned>, holder: SlabHandle, held: SlabHandle) {
    graph
        .enter(holder, |context| context.hold(held))
        .unwrap()
        .unwrap();
}

/// What a hold on one sealed cell keeps alive. A single candidate's slice is its whole closure, so
/// one-id pricing goes through the same door the marginal query does.
fn closure(graph: &CellGraph<Owned>, id: SealedId) -> Option<RetentionPrice> {
    graph.unique_retentions(&[id]).remove(0)
}

/// The sealed cell minted most recently — ids are monotone and never reused, so this is the one the
/// release just before the call produced.
fn newest(graph: &CellGraph<Owned>) -> SealedId {
    graph
        .cells
        .sealed
        .ids()
        .max()
        .expect("the tier holds a sealed cell")
}

fn retained(graph: &CellGraph<Owned>, id: SealedId) -> usize {
    graph
        .cells
        .sealed_retained_bytes(id)
        .expect("the sealed cell is present")
}

/// A chain of three sealed cells, `s` → `a` → `b`, each holding the next through its frozen
/// aggregate.
///
/// Every link keeps a second live holder so the seal-time merge finds no count of one, and the head
/// refuses death-time absorption so it seals rather than folding into its keeper.
fn sealed_chain(graph: &mut CellGraph<Owned>) -> (SealedId, SealedId, SealedId) {
    let b = graph.create(None, None).unwrap();
    let a = graph.create(None, None).unwrap();
    let s = graph.create(None, None).unwrap();
    let keep_b = graph.create(None, None).unwrap();
    let keep_a = graph.create(None, None).unwrap();
    let keep_s = graph.create(None, None).unwrap();
    for cell in [b, a, s] {
        allocate(graph, cell);
    }
    hold(graph, a, b);
    hold(graph, s, a);
    hold(graph, keep_b, b);
    hold(graph, keep_a, a);
    hold(graph, keep_s, s);

    graph.release(b, ReleaseAbsorption::Refused).unwrap();
    let b_id = newest(graph);
    graph.release(a, ReleaseAbsorption::Refused).unwrap();
    let a_id = newest(graph);
    graph.release(s, ReleaseAbsorption::Refused).unwrap();
    let s_id = newest(graph);
    assert_eq!(graph.cells.sealed.len(), 3);
    (s_id, a_id, b_id)
}

#[test]
fn a_closure_prices_everything_a_hold_on_the_sealed_cell_reaches() {
    let mut graph: CellGraph<Owned> = CellGraph::new(8, pin);
    let (s_id, a_id, b_id) = sealed_chain(&mut graph);

    let whole = closure(&graph, s_id).unwrap();
    assert_eq!(
        whole.bytes,
        retained(&graph, s_id) + retained(&graph, a_id) + retained(&graph, b_id)
    );
    assert!(whole.frozen);
    // The tail of the chain reaches nothing, so it prices at its own storage and no more.
    assert_eq!(
        closure(&graph, b_id).unwrap(),
        RetentionPrice {
            bytes: retained(&graph, b_id),
            frozen: true,
        }
    );
}

#[test]
fn a_shared_sub_tier_is_billed_once_within_one_closure() {
    let mut graph: CellGraph<Owned> = CellGraph::new(8, pin);
    let c = graph.create(None, None).unwrap();
    let a = graph.create(None, None).unwrap();
    let b = graph.create(None, None).unwrap();
    let s = graph.create(None, None).unwrap();
    let keep_a = graph.create(None, None).unwrap();
    let keep_b = graph.create(None, None).unwrap();
    let keep_s = graph.create(None, None).unwrap();
    for cell in [c, a, b, s] {
        allocate(&mut graph, cell);
    }
    hold(&mut graph, a, c);
    hold(&mut graph, b, c);
    hold(&mut graph, s, a);
    hold(&mut graph, s, b);
    hold(&mut graph, keep_a, a);
    hold(&mut graph, keep_b, b);
    hold(&mut graph, keep_s, s);

    graph.release(c, ReleaseAbsorption::Refused).unwrap();
    let c_id = newest(&graph);
    graph.release(a, ReleaseAbsorption::Refused).unwrap();
    let a_id = newest(&graph);
    graph.release(b, ReleaseAbsorption::Refused).unwrap();
    let b_id = newest(&graph);
    graph.release(s, ReleaseAbsorption::Refused).unwrap();
    let s_id = newest(&graph);
    assert_eq!(graph.cells.sealed.len(), 4);

    // Price both arms first, so the walk from the head meets two memos that each contain `c`.
    let arm_a = closure(&graph, a_id).unwrap();
    let arm_b = closure(&graph, b_id).unwrap();
    assert!(arm_a.frozen && arm_b.frozen);

    // A memo is merged as a set, never added as a number: summing the arms would bill `c` twice.
    let head = closure(&graph, s_id).unwrap();
    assert_eq!(
        head.bytes,
        retained(&graph, s_id)
            + retained(&graph, a_id)
            + retained(&graph, b_id)
            + retained(&graph, c_id)
    );
    assert!(head.bytes < retained(&graph, s_id) + arm_a.bytes + arm_b.bytes);
}

#[test]
fn a_closure_naming_a_live_cell_is_not_frozen_and_freezes_when_it_seals() {
    let mut graph: CellGraph<Owned> = CellGraph::new(4, pin);
    let live = graph.create(None, None).unwrap();
    let s = graph.create(None, None).unwrap();
    let keep_s = graph.create(None, None).unwrap();
    allocate(&mut graph, live);
    allocate(&mut graph, s);
    hold(&mut graph, s, live);
    hold(&mut graph, keep_s, s);

    graph.release(s, ReleaseAbsorption::Refused).unwrap();
    let s_id = newest(&graph);
    let live_bytes = graph.region_bytes(live).unwrap();
    assert!(live_bytes > 0);

    // A live cell the aggregate names is retention in waiting: its region is priced, and the answer
    // cannot be memoized while the cell can still allocate.
    let open = closure(&graph, s_id).unwrap();
    assert!(!open.frozen);
    assert_eq!(open.bytes, retained(&graph, s_id) + live_bytes);
    assert!(graph.cells.sealed.get(s_id).unwrap().memo().is_none());

    // The cell's slab column is empty and its only namer is the sealed cell, so it seals into it.
    graph.release(live, ReleaseAbsorption::IntoHolder).unwrap();
    assert_eq!(graph.cells.sealed.len(), 1);

    let frozen = closure(&graph, s_id).unwrap();
    assert!(frozen.frozen);
    // The bytes moved within the closure, so the total did not move at all.
    assert_eq!(frozen.bytes, open.bytes);
    let memo = graph
        .cells
        .sealed
        .get(s_id)
        .unwrap()
        .memo()
        .unwrap()
        .to_vec();
    assert_eq!(memo, vec![s_id]);
    // The memo records the node set, and the set still prices to the same total.
    assert_eq!(
        memo.iter().map(|id| retained(&graph, *id)).sum::<usize>(),
        open.bytes
    );
}

/// The memo is region state, so the sealed cell's price counts it like any other chunk. A sealed
/// cell whose cell never allocated is where that shows plainly: nothing retains anything until the
/// price query writes the memo, and then the sealed cell retains exactly what its region does.
#[test]
fn priming_a_memo_costs_the_sealed_cell_the_bytes_it_writes() {
    let mut graph: CellGraph<Owned> = CellGraph::new(4, pin);
    let bare = graph.create(None, None).unwrap();
    let keep = graph.create(None, None).unwrap();
    let keep_too = graph.create(None, None).unwrap();
    // No `allocate`: the cell writes nothing, so the sealed cell it seals into starts with no
    // chunk.
    hold(&mut graph, keep, bare);
    hold(&mut graph, keep_too, bare);

    graph.release(bare, ReleaseAbsorption::Refused).unwrap();
    let id = newest(&graph);
    assert_eq!(retained(&graph, id), 0);
    assert_eq!(graph.cells.occupancy().retained_bytes, 0);

    let priced = closure(&graph, id).unwrap();
    assert!(priced.frozen, "nothing live is left in the closure");

    // The memo landed in the sealed cell's own region, so both the sealed cell's price and the
    // tier's total grew by exactly the chunk it minted.
    let after = retained(&graph, id);
    assert!(
        after > 0,
        "the sealed cell's price counts the memo it now holds"
    );
    assert_eq!(
        after,
        graph
            .cells
            .sealed
            .get(id)
            .unwrap()
            .storage
            .allocated_bytes()
    );
    assert_eq!(graph.cells.occupancy().retained_bytes, after);
    assert_eq!(graph.cells.sealed.get(id).unwrap().memo(), Some(&[id][..]));

    // A second query writes nothing, so the price does not move again.
    assert_eq!(closure(&graph, id).unwrap(), priced);
    assert_eq!(retained(&graph, id), after);
}

#[test]
fn a_frozen_closure_memoizes_and_the_memo_survives_holder_churn() {
    let mut graph: CellGraph<Owned> = CellGraph::new(8, pin);
    let a = graph.create(None, None).unwrap();
    let s = graph.create(None, None).unwrap();
    let keep_a = graph.create(None, None).unwrap();
    let keep_a_too = graph.create(None, None).unwrap();
    let keep_s = graph.create(None, None).unwrap();
    let keep_s_too = graph.create(None, None).unwrap();
    allocate(&mut graph, a);
    allocate(&mut graph, s);
    hold(&mut graph, s, a);
    hold(&mut graph, keep_a, a);
    hold(&mut graph, keep_a_too, a);
    hold(&mut graph, keep_s, s);
    hold(&mut graph, keep_s_too, s);

    graph.release(a, ReleaseAbsorption::Refused).unwrap();
    let a_id = newest(&graph);
    graph.release(s, ReleaseAbsorption::Refused).unwrap();
    let s_id = newest(&graph);

    let priced = closure(&graph, s_id).unwrap();
    assert!(priced.frozen);
    assert_eq!(
        graph.cells.sealed.get(s_id).unwrap().memo().unwrap(),
        [s_id, a_id]
    );

    // Holders come and go above the closure; nothing inside it moves, so the memo stays exact.
    graph
        .release(keep_a, ReleaseAbsorption::IntoHolder)
        .unwrap();
    graph
        .release(keep_s, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert_eq!(graph.cells.sealed.get(a_id).unwrap().holders, 2);
    assert_eq!(graph.cells.sealed.get(s_id).unwrap().holders, 1);
    assert_eq!(closure(&graph, s_id).unwrap(), priced);

    // And a walk that consults no memo at all agrees with what was recorded.
    let fresh = graph.cells.transitive_pins(GraphNode::Sealed(s_id), false);
    assert!(fresh.cells.is_empty());
    assert_eq!(graph.cells.bytes_of(&fresh, &graph.regions), priced.bytes);
}

#[test]
fn a_walk_that_reaches_a_memoized_sealed_cell_merges_its_set() {
    let mut graph: CellGraph<Owned> = CellGraph::new(8, pin);
    let b = graph.create(None, None).unwrap();
    let a = graph.create(None, None).unwrap();
    let s = graph.create(None, None).unwrap();
    let live = graph.create(None, None).unwrap();
    let keep_b = graph.create(None, None).unwrap();
    let keep_a = graph.create(None, None).unwrap();
    let keep_s = graph.create(None, None).unwrap();
    for cell in [b, a, s, live] {
        allocate(&mut graph, cell);
    }
    hold(&mut graph, a, b);
    hold(&mut graph, s, a);
    hold(&mut graph, s, live);
    hold(&mut graph, keep_b, b);
    hold(&mut graph, keep_a, a);
    hold(&mut graph, keep_s, s);

    graph.release(b, ReleaseAbsorption::Refused).unwrap();
    let b_id = newest(&graph);
    graph.release(a, ReleaseAbsorption::Refused).unwrap();
    let a_id = newest(&graph);
    // The middle of the chain is frozen and priced first, so it carries a memo the head will meet.
    assert!(closure(&graph, a_id).unwrap().frozen);
    graph.release(s, ReleaseAbsorption::Refused).unwrap();
    let s_id = newest(&graph);

    let open = closure(&graph, s_id).unwrap();
    assert!(!open.frozen);
    assert_eq!(
        open.bytes,
        retained(&graph, s_id)
            + retained(&graph, a_id)
            + retained(&graph, b_id)
            + graph.region_bytes(live).unwrap()
    );

    graph.release(live, ReleaseAbsorption::IntoHolder).unwrap();
    let frozen = closure(&graph, s_id).unwrap();
    assert!(frozen.frozen);
    assert_eq!(frozen.bytes, open.bytes);
    assert_eq!(
        graph.cells.sealed.get(s_id).unwrap().memo().unwrap(),
        [s_id, a_id, b_id]
    );
}

#[test]
fn unique_slices_do_not_double_bill_a_shared_sub_tier() {
    let mut graph: CellGraph<Owned> = CellGraph::new(10, pin);
    let c = graph.create(None, None).unwrap();
    let a = graph.create(None, None).unwrap();
    let b = graph.create(None, None).unwrap();
    let first = graph.create(None, None).unwrap();
    let second = graph.create(None, None).unwrap();
    let keep_a = graph.create(None, None).unwrap();
    let keep_b = graph.create(None, None).unwrap();
    let keep_first = graph.create(None, None).unwrap();
    let keep_second = graph.create(None, None).unwrap();
    for cell in [c, a, b, first, second] {
        allocate(&mut graph, cell);
    }
    hold(&mut graph, first, c);
    hold(&mut graph, first, a);
    hold(&mut graph, second, c);
    hold(&mut graph, second, b);
    hold(&mut graph, keep_a, a);
    hold(&mut graph, keep_b, b);
    hold(&mut graph, keep_first, first);
    hold(&mut graph, keep_second, second);

    graph.release(c, ReleaseAbsorption::Refused).unwrap();
    let c_id = newest(&graph);
    graph.release(a, ReleaseAbsorption::Refused).unwrap();
    let a_id = newest(&graph);
    graph.release(b, ReleaseAbsorption::Refused).unwrap();
    let b_id = newest(&graph);
    graph.release(first, ReleaseAbsorption::Refused).unwrap();
    let first_id = newest(&graph);
    graph.release(second, ReleaseAbsorption::Refused).unwrap();
    let second_id = newest(&graph);

    // The sub-tier both candidates reach is billed to neither: releasing one hold buys back only
    // the part the other cannot reach.
    let slices = graph.unique_retentions(&[first_id, second_id]);
    assert_eq!(
        slices,
        vec![
            Some(RetentionPrice {
                bytes: retained(&graph, first_id) + retained(&graph, a_id),
                frozen: true,
            }),
            Some(RetentionPrice {
                bytes: retained(&graph, second_id) + retained(&graph, b_id),
                frozen: true,
            }),
        ]
    );
    // The shared sealed cell is in neither slice, but it is in each whole closure.
    assert_eq!(
        closure(&graph, first_id).unwrap().bytes,
        slices[0].unwrap().bytes + retained(&graph, c_id)
    );

    // A repeated id is one candidate, answered the same way at every position naming it.
    let repeated = graph.unique_retentions(&[first_id, first_id]);
    assert_eq!(repeated[0], repeated[1]);
    assert_eq!(repeated[0], closure(&graph, first_id));
}

#[test]
fn a_candidate_inside_another_candidates_closure_is_shared_throughout() {
    let mut graph: CellGraph<Owned> = CellGraph::new(8, pin);
    let (s_id, a_id, _) = sealed_chain(&mut graph);

    // Releasing the head buys back only the head: everything below it the other candidate reaches
    // too, and releasing the inner candidate on its own buys back nothing at all.
    assert_eq!(
        graph.unique_retentions(&[s_id, a_id]),
        vec![
            Some(RetentionPrice {
                bytes: retained(&graph, s_id),
                frozen: true,
            }),
            Some(RetentionPrice {
                bytes: 0,
                frozen: true,
            }),
        ]
    );
}

#[test]
fn an_absent_id_prices_as_none() {
    let mut graph: CellGraph<Owned> = CellGraph::new(8, pin);
    let (s_id, ..) = sealed_chain(&mut graph);
    let gone = graph.create(None, None).unwrap();
    let keeper = graph.create(None, None).unwrap();
    allocate(&mut graph, gone);
    hold(&mut graph, keeper, gone);

    graph.release(gone, ReleaseAbsorption::Refused).unwrap();
    let gone_id = newest(&graph);
    assert!(closure(&graph, gone_id).is_some());

    // The last holder goes, the sealed cell retires, and the id prices as nothing rather than as
    // zero.
    graph
        .release(keeper, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert_eq!(closure(&graph, gone_id), None);
    assert_eq!(graph.cells.sealed_retained_bytes(gone_id), None);
    let slices = graph.unique_retentions(&[gone_id, s_id]);
    assert_eq!(slices[0], None);
    assert_eq!(slices[1], closure(&graph, s_id));
}

#[test]
fn occupancy_tracks_both_tiers() {
    let mut graph: CellGraph<Owned> = CellGraph::new(4, pin);
    let first = graph.create(None, None).unwrap();
    let second = graph.create(None, None).unwrap();
    let third = graph.create(None, None).unwrap();
    for cell in [first, second, third] {
        allocate(&mut graph, cell);
    }
    hold(&mut graph, second, first);
    hold(&mut graph, third, second);
    let first_bytes = graph.region_bytes(first).unwrap();
    let second_bytes = graph.region_bytes(second).unwrap();

    assert_eq!(
        graph.cells.occupancy(),
        Occupancy {
            occupied: 3,
            cap: 4,
            sealed_cells: 0,
            retained_bytes: 0,
        }
    );

    graph.release(first, ReleaseAbsorption::Refused).unwrap();
    let first_id = newest(&graph);
    assert_eq!(
        graph.cells.occupancy(),
        Occupancy {
            occupied: 2,
            cap: 4,
            sealed_cells: 1,
            retained_bytes: first_bytes,
        }
    );
    assert_eq!(retained(&graph, first_id), first_bytes);

    // The second seal absorbs the first sealed cell rather than minting beside it, so the tier's
    // byte total grows while its sealed-cell count does not.
    graph.release(second, ReleaseAbsorption::Refused).unwrap();
    assert_eq!(
        graph.cells.occupancy(),
        Occupancy {
            occupied: 1,
            cap: 4,
            sealed_cells: 1,
            retained_bytes: first_bytes + second_bytes,
        }
    );

    graph.release(third, ReleaseAbsorption::IntoHolder).unwrap();
    assert_eq!(
        graph.cells.occupancy(),
        Occupancy {
            occupied: 0,
            cap: 4,
            sealed_cells: 0,
            retained_bytes: 0,
        }
    );
    assert!(graph.is_empty());
}

#[test]
fn pricing_mutates_no_hold() {
    let mut graph: CellGraph<Owned> = CellGraph::new(10, pin);
    let (s_id, ..) = sealed_chain(&mut graph);
    // A sealed cell naming a live cell, so the sweep meets an unfrozen closure as well as a frozen
    // one.
    let live = graph.create(None, None).unwrap();
    let open = graph.create(None, None).unwrap();
    let keep_open = graph.create(None, None).unwrap();
    allocate(&mut graph, live);
    allocate(&mut graph, open);
    hold(&mut graph, open, live);
    hold(&mut graph, keep_open, open);
    graph.release(open, ReleaseAbsorption::Refused).unwrap();

    let handles: Vec<SlabHandle> = (0..10)
        .map(|slot| SlabHandle::new(slot, graph.cells.slots[slot as usize].generation))
        .filter(|handle| graph.is_live(*handle))
        .collect();
    let ids: Vec<SealedId> = graph.cells.sealed.ids().collect();

    let pins: Vec<Bits<1>> = (0..10).map(|slot| *graph.cells.pins.row(slot)).collect();
    let births: Vec<Bits<1>> = (0..10).map(|slot| *graph.cells.birth.row(slot)).collect();
    let sealed_holds: Vec<SealedSet> = graph.cells.sealed_holds.to_vec();
    let naming: Vec<SealedSet> = graph.cells.naming.to_vec();
    let sealed_cells: Vec<(u32, GraphReach<1>)> = ids
        .iter()
        .map(|id| {
            let sealed_cell = graph.cells.sealed.get(*id).unwrap();
            (sealed_cell.holders, sealed_cell.aggregate.clone())
        })
        .collect();

    for handle in &handles {
        let _ = graph.region_bytes(*handle).unwrap();
    }
    // The pin price walks from every live cell's whole hold set into every other live cell, so the
    // sweep covers both tiers, memoized and open closures alike.
    for handle in &handles {
        for other in &handles {
            let reach = GraphReach::from_parts(
                *graph.cells.pins.row(other.slot()),
                graph.cells.sealed_holds[other.slot() as usize].clone(),
            );
            let _ = graph.cells.pin_price(
                handle.slot(),
                &reach,
                &GraphReach::empty(),
                &graph.regions,
                graph.cells.scratch_at_rest(),
            );
        }
    }
    for id in &ids {
        let _ = closure(&graph, *id).unwrap();
    }
    let _ = graph.unique_retentions(&ids);
    let _ = graph.cells.occupancy();
    assert!(closure(&graph, s_id).unwrap().frozen);

    // The memo is the only mark a price query leaves, and a memo is not a hold.
    for slot in 0..10 {
        assert_eq!(graph.cells.pins.row(slot).to_owned(), pins[slot as usize]);
        assert_eq!(
            graph.cells.birth.row(slot).to_owned(),
            births[slot as usize]
        );
    }
    assert_eq!(graph.cells.sealed_holds.to_vec(), sealed_holds);
    assert_eq!(graph.cells.naming.to_vec(), naming);
    for (id, (holders, aggregate)) in ids.iter().zip(&sealed_cells) {
        let sealed_cell = graph.cells.sealed.get(*id).unwrap();
        assert_eq!(sealed_cell.holders, *holders);
        assert_eq!(&sealed_cell.aggregate, aggregate);
    }
}
