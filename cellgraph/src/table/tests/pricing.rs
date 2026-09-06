//! The price queries an embedder decides copy-versus-hold with: what a hold on a record keeps
//! alive, the slice of that only one of several candidates reaches, what a live cart has accreted,
//! and how full the two tiers are. See [liveness-matrix.md § Bounding the two
//! tiers](../../../design/liveness-matrix.md#bounding-the-two-tiers).
//!
//! Every shape here keeps a record above the count a locality merge would absorb it at — an extra
//! live holder, or a refused release — because a merge that fires leaves nothing to price.

use super::super::*;
use super::{Number, Owned, pin};

/// Give a cell a region of its own, so it prices at more than nothing.
fn allocate(table: &mut CellTable<Owned>, cell: Handle) {
    table
        .enter(cell, |context| {
            context.alloc::<Number>(|writer| writer.value(1));
        })
        .unwrap();
}

/// Mint a bare pin hold from one live cell onto another.
fn hold(table: &mut CellTable<Owned>, holder: Handle, held: Handle) {
    table
        .enter(holder, |context| context.hold(held))
        .unwrap()
        .unwrap();
}

/// What a hold on one record keeps alive. A single candidate's slice is its whole closure, so
/// one-id pricing goes through the same door the marginal query does.
fn closure(table: &CellTable<Owned>, id: SealedId) -> Option<Closure> {
    table.unique_closures(&[id]).remove(0)
}

/// The record minted most recently — ids are monotone and never reused, so this is the one the
/// release just before the call produced.
fn newest(table: &CellTable<Owned>) -> SealedId {
    table.sealed.ids().max().expect("the tier holds a record")
}

fn retained(table: &CellTable<Owned>, id: SealedId) -> usize {
    table
        .sealed_retained_bytes(id)
        .expect("the record is present")
}

/// A chain of three records, `s` → `a` → `b`, each holding the next through its frozen aggregate.
///
/// Every link keeps a second live holder so the seal-time merge finds no count of one, and the head
/// refuses death-time absorption so it seals rather than folding into its keeper.
fn sealed_chain(table: &mut CellTable<Owned>) -> (SealedId, SealedId, SealedId) {
    let b = table.create(None, None).unwrap();
    let a = table.create(None, None).unwrap();
    let s = table.create(None, None).unwrap();
    let keep_b = table.create(None, None).unwrap();
    let keep_a = table.create(None, None).unwrap();
    let keep_s = table.create(None, None).unwrap();
    for cell in [b, a, s] {
        allocate(table, cell);
    }
    hold(table, a, b);
    hold(table, s, a);
    hold(table, keep_b, b);
    hold(table, keep_a, a);
    hold(table, keep_s, s);

    table.release(b, Absorption::Refused).unwrap();
    let b_id = newest(table);
    table.release(a, Absorption::Refused).unwrap();
    let a_id = newest(table);
    table.release(s, Absorption::Refused).unwrap();
    let s_id = newest(table);
    assert_eq!(table.sealed.len(), 3);
    (s_id, a_id, b_id)
}

#[test]
fn a_closure_prices_everything_a_hold_on_the_record_reaches() {
    let mut table: CellTable<Owned> = CellTable::new(8, pin);
    let (s_id, a_id, b_id) = sealed_chain(&mut table);

    let whole = closure(&table, s_id).unwrap();
    assert_eq!(
        whole.bytes,
        retained(&table, s_id) + retained(&table, a_id) + retained(&table, b_id)
    );
    assert!(whole.frozen);
    // The tail of the chain reaches nothing, so it prices at its own storage and no more.
    assert_eq!(
        closure(&table, b_id).unwrap(),
        Closure {
            bytes: retained(&table, b_id),
            frozen: true,
        }
    );
}

#[test]
fn a_shared_sub_tier_is_billed_once_within_one_closure() {
    let mut table: CellTable<Owned> = CellTable::new(8, pin);
    let c = table.create(None, None).unwrap();
    let a = table.create(None, None).unwrap();
    let b = table.create(None, None).unwrap();
    let s = table.create(None, None).unwrap();
    let keep_a = table.create(None, None).unwrap();
    let keep_b = table.create(None, None).unwrap();
    let keep_s = table.create(None, None).unwrap();
    for cell in [c, a, b, s] {
        allocate(&mut table, cell);
    }
    hold(&mut table, a, c);
    hold(&mut table, b, c);
    hold(&mut table, s, a);
    hold(&mut table, s, b);
    hold(&mut table, keep_a, a);
    hold(&mut table, keep_b, b);
    hold(&mut table, keep_s, s);

    table.release(c, Absorption::Refused).unwrap();
    let c_id = newest(&table);
    table.release(a, Absorption::Refused).unwrap();
    let a_id = newest(&table);
    table.release(b, Absorption::Refused).unwrap();
    let b_id = newest(&table);
    table.release(s, Absorption::Refused).unwrap();
    let s_id = newest(&table);
    assert_eq!(table.sealed.len(), 4);

    // Price both arms first, so the walk from the head meets two memos that each contain `c`.
    let arm_a = closure(&table, a_id).unwrap();
    let arm_b = closure(&table, b_id).unwrap();
    assert!(arm_a.frozen && arm_b.frozen);

    // A memo is merged as a set, never added as a number: summing the arms would bill `c` twice.
    let head = closure(&table, s_id).unwrap();
    assert_eq!(
        head.bytes,
        retained(&table, s_id)
            + retained(&table, a_id)
            + retained(&table, b_id)
            + retained(&table, c_id)
    );
    assert!(head.bytes < retained(&table, s_id) + arm_a.bytes + arm_b.bytes);
}

#[test]
fn a_closure_naming_a_live_cell_is_not_frozen_and_freezes_when_it_seals() {
    let mut table: CellTable<Owned> = CellTable::new(4, pin);
    let live = table.create(None, None).unwrap();
    let s = table.create(None, None).unwrap();
    let keep_s = table.create(None, None).unwrap();
    allocate(&mut table, live);
    allocate(&mut table, s);
    hold(&mut table, s, live);
    hold(&mut table, keep_s, s);

    table.release(s, Absorption::Refused).unwrap();
    let s_id = newest(&table);
    let live_bytes = table.region_bytes(live).unwrap();
    assert!(live_bytes > 0);

    // A live cell the aggregate names is retention in waiting: its region is priced, and the answer
    // cannot be memoized while the cell can still allocate.
    let open = closure(&table, s_id).unwrap();
    assert!(!open.frozen);
    assert_eq!(open.bytes, retained(&table, s_id) + live_bytes);
    assert!(table.sealed.get(s_id).unwrap().closure.get().is_none());

    // The cell's slab column is empty and its only namer is the record, so it seals into it.
    table.release(live, Absorption::IntoHolder).unwrap();
    assert_eq!(table.sealed.len(), 1);

    let frozen = closure(&table, s_id).unwrap();
    assert!(frozen.frozen);
    // The bytes moved within the closure, so the total did not move at all.
    assert_eq!(frozen.bytes, open.bytes);
    let memo = table.sealed.get(s_id).unwrap().closure.get().unwrap();
    assert_eq!(memo.records, vec![s_id]);
    // The memo records the node set, and the set still prices to the same total.
    assert_eq!(
        memo.records
            .iter()
            .map(|id| retained(&table, *id))
            .sum::<usize>(),
        open.bytes
    );
}

#[test]
fn a_frozen_closure_memoizes_and_the_memo_survives_holder_churn() {
    let mut table: CellTable<Owned> = CellTable::new(8, pin);
    let a = table.create(None, None).unwrap();
    let s = table.create(None, None).unwrap();
    let keep_a = table.create(None, None).unwrap();
    let keep_a_too = table.create(None, None).unwrap();
    let keep_s = table.create(None, None).unwrap();
    let keep_s_too = table.create(None, None).unwrap();
    allocate(&mut table, a);
    allocate(&mut table, s);
    hold(&mut table, s, a);
    hold(&mut table, keep_a, a);
    hold(&mut table, keep_a_too, a);
    hold(&mut table, keep_s, s);
    hold(&mut table, keep_s_too, s);

    table.release(a, Absorption::Refused).unwrap();
    let a_id = newest(&table);
    table.release(s, Absorption::Refused).unwrap();
    let s_id = newest(&table);

    let priced = closure(&table, s_id).unwrap();
    assert!(priced.frozen);
    assert_eq!(
        table
            .sealed
            .get(s_id)
            .unwrap()
            .closure
            .get()
            .unwrap()
            .records,
        vec![s_id, a_id]
    );

    // Holders come and go above the closure; nothing inside it moves, so the memo stays exact.
    table.release(keep_a, Absorption::IntoHolder).unwrap();
    table.release(keep_s, Absorption::IntoHolder).unwrap();
    assert_eq!(table.sealed.get(a_id).unwrap().holders, 2);
    assert_eq!(table.sealed.get(s_id).unwrap().holders, 1);
    assert_eq!(closure(&table, s_id).unwrap(), priced);

    // And a walk that consults no memo at all agrees with what was recorded.
    let fresh = table.reached_from(Node::Sealed(s_id), false);
    assert!(fresh.cells.is_empty());
    assert_eq!(table.bytes_of(&fresh), priced.bytes);
}

#[test]
fn a_walk_that_reaches_a_memoized_record_merges_its_set() {
    let mut table: CellTable<Owned> = CellTable::new(8, pin);
    let b = table.create(None, None).unwrap();
    let a = table.create(None, None).unwrap();
    let s = table.create(None, None).unwrap();
    let live = table.create(None, None).unwrap();
    let keep_b = table.create(None, None).unwrap();
    let keep_a = table.create(None, None).unwrap();
    let keep_s = table.create(None, None).unwrap();
    for cell in [b, a, s, live] {
        allocate(&mut table, cell);
    }
    hold(&mut table, a, b);
    hold(&mut table, s, a);
    hold(&mut table, s, live);
    hold(&mut table, keep_b, b);
    hold(&mut table, keep_a, a);
    hold(&mut table, keep_s, s);

    table.release(b, Absorption::Refused).unwrap();
    let b_id = newest(&table);
    table.release(a, Absorption::Refused).unwrap();
    let a_id = newest(&table);
    // The middle of the chain is frozen and priced first, so it carries a memo the head will meet.
    assert!(closure(&table, a_id).unwrap().frozen);
    table.release(s, Absorption::Refused).unwrap();
    let s_id = newest(&table);

    let open = closure(&table, s_id).unwrap();
    assert!(!open.frozen);
    assert_eq!(
        open.bytes,
        retained(&table, s_id)
            + retained(&table, a_id)
            + retained(&table, b_id)
            + table.region_bytes(live).unwrap()
    );

    table.release(live, Absorption::IntoHolder).unwrap();
    let frozen = closure(&table, s_id).unwrap();
    assert!(frozen.frozen);
    assert_eq!(frozen.bytes, open.bytes);
    assert_eq!(
        table
            .sealed
            .get(s_id)
            .unwrap()
            .closure
            .get()
            .unwrap()
            .records,
        vec![s_id, a_id, b_id]
    );
}

#[test]
fn unique_slices_do_not_double_bill_a_shared_sub_tier() {
    let mut table: CellTable<Owned> = CellTable::new(10, pin);
    let c = table.create(None, None).unwrap();
    let a = table.create(None, None).unwrap();
    let b = table.create(None, None).unwrap();
    let first = table.create(None, None).unwrap();
    let second = table.create(None, None).unwrap();
    let keep_a = table.create(None, None).unwrap();
    let keep_b = table.create(None, None).unwrap();
    let keep_first = table.create(None, None).unwrap();
    let keep_second = table.create(None, None).unwrap();
    for cell in [c, a, b, first, second] {
        allocate(&mut table, cell);
    }
    hold(&mut table, first, c);
    hold(&mut table, first, a);
    hold(&mut table, second, c);
    hold(&mut table, second, b);
    hold(&mut table, keep_a, a);
    hold(&mut table, keep_b, b);
    hold(&mut table, keep_first, first);
    hold(&mut table, keep_second, second);

    table.release(c, Absorption::Refused).unwrap();
    let c_id = newest(&table);
    table.release(a, Absorption::Refused).unwrap();
    let a_id = newest(&table);
    table.release(b, Absorption::Refused).unwrap();
    let b_id = newest(&table);
    table.release(first, Absorption::Refused).unwrap();
    let first_id = newest(&table);
    table.release(second, Absorption::Refused).unwrap();
    let second_id = newest(&table);

    // The sub-tier both candidates reach is billed to neither: releasing one hold buys back only
    // the part the other cannot reach.
    let slices = table.unique_closures(&[first_id, second_id]);
    assert_eq!(
        slices,
        vec![
            Some(Closure {
                bytes: retained(&table, first_id) + retained(&table, a_id),
                frozen: true,
            }),
            Some(Closure {
                bytes: retained(&table, second_id) + retained(&table, b_id),
                frozen: true,
            }),
        ]
    );
    // The shared record is in neither slice, but it is in each whole closure.
    assert_eq!(
        closure(&table, first_id).unwrap().bytes,
        slices[0].unwrap().bytes + retained(&table, c_id)
    );

    // A repeated id is one candidate, answered the same way at every position naming it.
    let repeated = table.unique_closures(&[first_id, first_id]);
    assert_eq!(repeated[0], repeated[1]);
    assert_eq!(repeated[0], closure(&table, first_id));
}

#[test]
fn a_candidate_inside_another_candidates_closure_is_shared_throughout() {
    let mut table: CellTable<Owned> = CellTable::new(8, pin);
    let (s_id, a_id, _) = sealed_chain(&mut table);

    // Releasing the head buys back only the head: everything below it the other candidate reaches
    // too, and releasing the inner candidate on its own buys back nothing at all.
    assert_eq!(
        table.unique_closures(&[s_id, a_id]),
        vec![
            Some(Closure {
                bytes: retained(&table, s_id),
                frozen: true,
            }),
            Some(Closure {
                bytes: 0,
                frozen: true,
            }),
        ]
    );
}

#[test]
fn an_absent_id_prices_as_none() {
    let mut table: CellTable<Owned> = CellTable::new(8, pin);
    let (s_id, ..) = sealed_chain(&mut table);
    let gone = table.create(None, None).unwrap();
    let keeper = table.create(None, None).unwrap();
    allocate(&mut table, gone);
    hold(&mut table, keeper, gone);

    table.release(gone, Absorption::Refused).unwrap();
    let gone_id = newest(&table);
    assert!(closure(&table, gone_id).is_some());

    // The last holder goes, the record retires, and the id prices as nothing rather than as zero.
    table.release(keeper, Absorption::IntoHolder).unwrap();
    assert_eq!(closure(&table, gone_id), None);
    assert_eq!(table.sealed_retained_bytes(gone_id), None);
    let slices = table.unique_closures(&[gone_id, s_id]);
    assert_eq!(slices[0], None);
    assert_eq!(slices[1], closure(&table, s_id));
}

#[test]
fn occupancy_tracks_both_tiers() {
    let mut table: CellTable<Owned> = CellTable::new(4, pin);
    let first = table.create(None, None).unwrap();
    let second = table.create(None, None).unwrap();
    let third = table.create(None, None).unwrap();
    for cell in [first, second, third] {
        allocate(&mut table, cell);
    }
    hold(&mut table, second, first);
    hold(&mut table, third, second);
    let first_bytes = table.region_bytes(first).unwrap();
    let second_bytes = table.region_bytes(second).unwrap();

    assert_eq!(
        table.occupancy(),
        Occupancy {
            occupied: 3,
            cap: 4,
            records: 0,
            retained_bytes: 0,
        }
    );

    table.release(first, Absorption::Refused).unwrap();
    let first_id = newest(&table);
    assert_eq!(
        table.occupancy(),
        Occupancy {
            occupied: 2,
            cap: 4,
            records: 1,
            retained_bytes: first_bytes,
        }
    );
    assert_eq!(retained(&table, first_id), first_bytes);

    // The second seal absorbs the first record rather than minting beside it, so the tier's byte
    // total grows while its record count does not.
    table.release(second, Absorption::Refused).unwrap();
    assert_eq!(
        table.occupancy(),
        Occupancy {
            occupied: 1,
            cap: 4,
            records: 1,
            retained_bytes: first_bytes + second_bytes,
        }
    );

    table.release(third, Absorption::IntoHolder).unwrap();
    assert_eq!(
        table.occupancy(),
        Occupancy {
            occupied: 0,
            cap: 4,
            records: 0,
            retained_bytes: 0,
        }
    );
    assert!(table.is_empty());
}

#[test]
fn pricing_mutates_no_hold() {
    let mut table: CellTable<Owned> = CellTable::new(10, pin);
    let (s_id, ..) = sealed_chain(&mut table);
    // A record naming a live cell, so the sweep meets an unfrozen closure as well as a frozen one.
    let live = table.create(None, None).unwrap();
    let open = table.create(None, None).unwrap();
    let keep_open = table.create(None, None).unwrap();
    allocate(&mut table, live);
    allocate(&mut table, open);
    hold(&mut table, open, live);
    hold(&mut table, keep_open, open);
    table.release(open, Absorption::Refused).unwrap();

    let handles: Vec<Handle> = (0..10)
        .map(|slot| Handle::new(slot, table.slots[slot as usize].generation))
        .filter(|handle| table.is_live(*handle))
        .collect();
    let ids: Vec<SealedId> = table.sealed.ids().collect();

    let pins: Vec<Bits<1>> = (0..10).map(|slot| *table.pins.row(slot)).collect();
    let births: Vec<Bits<1>> = (0..10).map(|slot| *table.birth.row(slot)).collect();
    let sealed_holds: Vec<SealedSet> = table.sealed_holds.to_vec();
    let naming: Vec<SealedSet> = table.naming.to_vec();
    let records: Vec<(u32, Mask<1>)> = ids
        .iter()
        .map(|id| {
            let record = table.sealed.get(*id).unwrap();
            (record.holders, record.aggregate.clone())
        })
        .collect();

    for handle in &handles {
        let _ = table.region_bytes(*handle).unwrap();
    }
    // The pin price walks from every live cell's whole hold set into every other live cell, so the
    // sweep covers both tiers, memoized and open closures alike.
    for handle in &handles {
        for other in &handles {
            let reach = Mask::from_parts(
                *table.pins.row(other.slot()),
                table.sealed_holds[other.slot() as usize].clone(),
            );
            let _ = table.pin_price(
                handle.slot(),
                &reach,
                &Mask::empty(),
                table.scratch_at_rest(),
            );
        }
    }
    for id in &ids {
        let _ = closure(&table, *id).unwrap();
    }
    let _ = table.unique_closures(&ids);
    let _ = table.occupancy();
    assert!(closure(&table, s_id).unwrap().frozen);

    // The memo is the only mark a price query leaves, and a memo is not a hold.
    for slot in 0..10 {
        assert_eq!(table.pins.row(slot).to_owned(), pins[slot as usize]);
        assert_eq!(table.birth.row(slot).to_owned(), births[slot as usize]);
    }
    assert_eq!(table.sealed_holds.to_vec(), sealed_holds);
    assert_eq!(table.naming.to_vec(), naming);
    for (id, (holders, aggregate)) in ids.iter().zip(&records) {
        let record = table.sealed.get(*id).unwrap();
        assert_eq!(record.holders, *holders);
        assert_eq!(&record.aggregate, aggregate);
    }
}
