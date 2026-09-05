//! The liveness-matrix invariants, over random interleavings of the verbs. The hand-written tests
//! pin shapes; this one pins that no order of `create` / `hold` / `alloc_into` / `keep` / `read` /
//! `release` can break the conditions the whole model rests on
//! ([liveness-matrix.md § Invariants](../../../design/liveness-matrix.md#invariants)):
//!
//! - a recycled slot is named by nothing — no occupant's row in either relation, and no frozen
//!   aggregate;
//! - a cell resident after its declared death is named by an occupant's birth row, the one
//!   relation with no sealed half to convert into;
//! - every sealed record's holder count equals the number of hold sets that name it, and the
//!   reverse naming index is exactly the transpose of the aggregates;
//! - every bit and id of a stored mask is covered by a live cell or a live record — mask validity;
//! - no hold set names its own owner, and every present record has a holder — which together make
//!   a record that survives a wound-down run a ring by arithmetic, with no ring walk in the loop;
//! - the live tier never grows across a release, so storage that has sealed never re-enters it;
//! - a record that survives a wound-down run was named by two hold sets at some point — the
//!   universal the hand-written ring-dissolution tests are three instances of;
//! - every memoized closure still equals the walk that would recompute it, and no memo exists
//!   unless a price query put it there — the never-invalidated memo carried across every
//!   interleaving, and the substrate's own paths pricing nothing.

use proptest::prelude::*;

use super::super::*;
use super::{Borrowed, Number, live_bytes};

const CAP: u32 = 6;

/// One verb, with its operands as indices into the handles minted so far — so a generated run
/// names cells that may since have died, which is the point: a stale operand must be refused, not
/// mis-applied.
#[derive(Clone, Debug)]
enum Verb {
    Create { parent: Option<usize> },
    Hold { holder: usize, held: usize },
    Place { producer: usize, consumer: usize },
    Keep { cell: usize, over: usize },
    Read { cell: usize },
    Release { cell: usize, refuse: bool },
    Price { index: usize },
}

/// The verbs that change the table. What the merge-coverage test draws from: a read-only verb
/// reaches no merge, so mixing one in would only thin the corpus that has to reach all three.
fn state_verb() -> impl Strategy<Value = Verb> {
    prop_oneof![
        proptest::option::of(0..8usize).prop_map(|parent| Verb::Create { parent }),
        (0..8usize, 0..8usize).prop_map(|(holder, held)| Verb::Hold { holder, held }),
        (0..8usize, 0..8usize).prop_map(|(producer, consumer)| Verb::Place { producer, consumer }),
        (0..8usize, 0..8usize).prop_map(|(cell, over)| Verb::Keep { cell, over }),
        (0..8usize).prop_map(|cell| Verb::Read { cell }),
        (0..8usize, any::<bool>()).prop_map(|(cell, refuse)| Verb::Release { cell, refuse }),
    ]
}

/// Those plus the price query, which the invariant sweep needs interleaved among them to catch a
/// memo taken against a table that then kept moving.
fn verb() -> impl Strategy<Value = Verb> {
    prop_oneof![
        6 => state_verb(),
        1 => (0..8usize).prop_map(|index| Verb::Price { index }),
    ]
}

/// The tier's ids in id order, since a walk's answers must not depend on hash iteration order.
fn sorted_ids(table: &CellTable<Borrowed>) -> Vec<SealedId> {
    let mut ids: Vec<SealedId> = table.sealed.ids().collect();
    ids.sort();
    ids
}

/// `memoized` carries the ids that already held a memo before this step, and is refreshed to the
/// current set on the way out. Only a price query may write one, so unless the step just run was a
/// `Price` verb, an id outside that set carrying a memo is one a mint or a release left behind.
fn check_invariants(table: &CellTable<Borrowed>, memoized: &mut Vec<SealedId>, priced: bool) {
    let occupied: Vec<u32> = (0..CAP)
        .filter(|slot| table.slots[*slot as usize].state != SlotState::Free)
        .collect();

    for slot in 0..CAP {
        let by_birth = table.birth.held_by_any(occupied.iter().copied(), slot);
        let by_pins = table.pins.held_by_any(occupied.iter().copied(), slot);
        let by_aggregate = !table.naming[slot as usize].is_empty();
        match table.slots[slot as usize].state {
            SlotState::Free => {
                assert!(
                    !by_birth && !by_pins && !by_aggregate,
                    "slot {slot} is free but something still names it"
                );
                assert!(
                    table.sealed_holds[slot as usize].is_empty(),
                    "slot {slot} is free but kept a sealed hold"
                );
            }
            SlotState::Dead => assert!(
                by_birth,
                "slot {slot} is resident but no birth row names it, so it should have left the slab"
            ),
            SlotState::Live => {}
        }
    }

    // Quiescence spans both tiers: the slab being clear is only half of it, and a table that
    // reports itself empty while a record survives would hide exactly the ring this test hunts.
    assert_eq!(
        table.is_empty(),
        occupied.is_empty() && table.sealed.is_empty(),
        "is_empty disagrees with the two tiers it summarizes"
    );

    let records: Vec<SealedId> = table.sealed.ids().collect();
    for id in &records {
        let record = table.sealed.get(*id).unwrap();
        let from_cells = occupied
            .iter()
            .filter(|slot| table.sealed_holds[**slot as usize].contains(*id))
            .count();
        let from_records = records
            .iter()
            .filter(|other| *other != id)
            .filter(|other| {
                table
                    .sealed
                    .get(**other)
                    .unwrap()
                    .aggregate
                    .names_sealed(*id)
            })
            .count();
        assert_eq!(
            record.holders as usize,
            from_cells + from_records,
            "record {id:?} counts holders that do not name it, or misses ones that do"
        );
        // A count of zero reclaims a record on the spot, so a record still in the tier at zero is
        // stranded storage nothing can ever release.
        assert!(
            record.holders >= 1,
            "record {id:?} is present with no holder"
        );
        assert!(
            !record.aggregate.names_sealed(*id),
            "record {id:?} names itself"
        );
        for named in record.aggregate.slab_slots(CAP) {
            assert!(
                table.naming[named as usize].contains(*id),
                "record {id:?} names slot {named} without registering in the naming index"
            );
        }
        for named in record.aggregate.sealed().iter() {
            assert!(
                records.contains(&named),
                "record {id:?} names a retired record"
            );
        }
    }

    for slot in 0..CAP {
        assert!(
            !table.pins.test(slot, slot),
            "slot {slot} holds itself, so its count could never reach zero"
        );
        for id in table.naming[slot as usize].iter() {
            let record = table
                .sealed
                .get(id)
                .expect("the naming index names a live record");
            assert!(
                record.aggregate.names(slot),
                "the naming index claims record {id:?} names slot {slot}"
            );
        }
        for id in table.sealed_holds[slot as usize].iter() {
            assert!(records.contains(&id), "slot {slot} holds a retired record");
        }
        // Mask validity: every bit and id of the cell's one stored mask is covered.
        if let Some(stored) = &table.slots[slot as usize].continuation {
            for named in stored.reach.slab_slots(CAP) {
                assert!(
                    table.slots[named as usize].state != SlotState::Free,
                    "slot {slot} stores a mask naming the recycled slot {named}"
                );
            }
            for named in stored.reach.sealed().iter() {
                assert!(
                    records.contains(&named),
                    "slot {slot} stores a mask naming a retired record"
                );
            }
        }
    }

    // The occupancy signal is a maintained total, not a scan, so it has to agree with one.
    let scanned: usize = records
        .iter()
        .map(|id| table.sealed.get(*id).unwrap().retained_bytes())
        .sum();
    let occupancy = table.occupancy();
    assert_eq!(
        occupancy.retained_bytes, scanned,
        "the tier's running byte total drifted from what its records retain"
    );
    assert_eq!(occupancy.records, records.len());
    assert_eq!(occupancy.occupied as usize, occupied.len());
    assert_eq!(occupancy.cap, CAP);

    let mut now_memoized = Vec::new();
    for id in &records {
        let record = table.sealed.get(*id).unwrap();
        let Some(memo) = record.closure.get() else {
            continue;
        };
        now_memoized.push(*id);
        assert!(
            priced || memoized.contains(id),
            "record {id:?} carries a memo no price query asked for"
        );
        // Recomputed from scratch, consulting no memo at all: a closure memoized as frozen still
        // names no live cell, spans the same records, and prices at the same bytes. Nothing inside
        // a frozen closure changes, and this is the check that says so for every interleaving.
        let fresh = table.reached_from(Node::Sealed(*id), false);
        assert!(
            fresh.cells.is_empty(),
            "the memoized closure of {id:?} has since named a live cell"
        );
        let mut walked = fresh.records.clone();
        walked.sort();
        let mut memoized = memo.records.clone();
        memoized.sort();
        assert_eq!(walked, memoized, "the memoized closure of {id:?} drifted");
        assert_eq!(
            table.bytes_of(&fresh),
            memo.bytes,
            "the memoized bytes of {id:?} drifted"
        );
    }
    *memoized = now_memoized;
}

/// Drive one generated run to its end — every verb, then a wind-down that releases everything —
/// checking the invariants after every step. Reports the merges the run performed, which is what
/// tells a generated corpus that reaches all three shapes from one that only claims to.
fn run(verbs: &[Verb]) -> Merges {
    let mut table: CellTable<Borrowed> = CellTable::new(CAP);
    let mut minted: Vec<Handle> = Vec::new();
    // Nothing has been priced yet, so no record may carry a memo.
    let mut memoized: Vec<SealedId> = Vec::new();

    for step in verbs {
        match *step {
            Verb::Create { parent } => {
                let parent = parent.and_then(|index| minted.get(index).copied());
                if let Ok(handle) = table.create(parent, None) {
                    minted.push(handle);
                }
            }
            Verb::Hold { holder, held } => {
                if let (Some(holder), Some(held)) =
                    (minted.get(holder).copied(), minted.get(held).copied())
                    && table.is_live(holder)
                {
                    let _ = table.enter(holder, |context| context.hold(held));
                }
            }
            Verb::Place { producer, consumer } => {
                if let (Some(producer), Some(consumer)) =
                    (minted.get(producer).copied(), minted.get(consumer).copied())
                    && table.is_live(producer)
                {
                    let _ = table.enter(producer, |context| {
                        let value = context.alloc::<Number>(|writer| writer.value(1));
                        context
                            .alloc_into::<Number, Number>(consumer, &[&value], |_w, views| views[0])
                            .map(|_| ())
                    });
                }
            }
            // A continuation kept over a value homed elsewhere is the one stored mask a cell
            // owns, and the only thing the seal transition has to rewrite.
            Verb::Keep { cell, over } => {
                if let (Some(cell), Some(over)) =
                    (minted.get(cell).copied(), minted.get(over).copied())
                    && table.is_live(cell)
                {
                    let _ = table.enter(cell, |context| {
                        if let Ok(value) =
                            context.alloc_into::<Number, Number>(over, &[], |w, _| w.value(1))
                        {
                            context.store_successor_capturing(&[&value], |_w, views| views[0]);
                        }
                    });
                }
            }
            // Reading the kept continuation back is where a stale mask would surface: the reach
            // that comes out is derived through whatever the tier has since done to it.
            Verb::Read { cell } => {
                if let Some(cell) = minted.get(cell).copied()
                    && table.is_live(cell)
                {
                    let reach = table
                        .enter(cell, |context| {
                            context.continuation().map(|opened| opened.reach().clone())
                        })
                        .unwrap();
                    if let Some(reach) = reach {
                        for named in reach.slab_slots(CAP) {
                            assert!(
                                table.slots[named as usize].state != SlotState::Free,
                                "a read handed back a mask naming the recycled slot {named}"
                            );
                        }
                        for named in reach.sealed().iter() {
                            assert!(
                                table.sealed.get(named).is_some(),
                                "a read handed back a mask naming a retired record"
                            );
                        }
                    }
                }
            }
            // Both dispositions are generated, so a run reaches the sealed shapes a merge would
            // otherwise have collapsed as well as the merges themselves.
            Verb::Release { cell, refuse } => {
                let absorption = if refuse {
                    Absorption::Refused
                } else {
                    Absorption::IntoHolder
                };
                if let Some(cell) = minted.get(cell).copied() {
                    let before = live_bytes(&table, CAP);
                    let _ = table.release(cell, absorption);
                    // A merge moves bytes between live cells and a seal moves them out, but nothing
                    // moves them back in: storage that has sealed never re-enters the live tier.
                    assert!(
                        live_bytes(&table, CAP) <= before,
                        "a release grew the live tier, so sealed storage re-entered it"
                    );
                }
            }
            // Pricing is read-only: it changes no hold, and the invariant sweep after every step
            // is what says so. What it does write is a memo, and the sweep re-derives every one.
            Verb::Price { index } => {
                let ids = sorted_ids(&table);
                if !ids.is_empty() {
                    let id = ids[index % ids.len()];
                    let whole = table.closure(id).expect("the id came out of the tier");
                    // One candidate shares its closure with nobody, so its slice is the whole of it.
                    assert_eq!(table.unique_closures(&[id]), vec![Some(whole)]);

                    let slices = table.unique_closures(&ids);
                    let mut total = 0;
                    for (id, slice) in ids.iter().zip(&slices) {
                        let slice = slice.expect("every id came out of the tier");
                        let whole = table.closure(*id).expect("the id came out of the tier");
                        assert!(
                            slice.bytes <= whole.bytes,
                            "the unique slice of {id:?} outprices its whole closure"
                        );
                        assert_eq!(slice.frozen, whole.frozen);
                        total += slice.bytes;
                    }
                    // The slices partition part of one graph, so together they cannot outprice it.
                    assert!(
                        total <= table.occupancy().retained_bytes + live_bytes(&table, CAP),
                        "the unique slices together outprice both tiers"
                    );
                }
            }
        }
        check_invariants(&table, &mut memoized, matches!(*step, Verb::Price { .. }));
    }

    // Winding the run down: once every cell's death is declared, the cascade returns every slot,
    // and the tier retains only what a ring no merge met tied together.
    for handle in &minted {
        let _ = table.release(*handle, Absorption::IntoHolder);
    }
    check_invariants(&table, &mut memoized, false);
    for slot in 0..CAP {
        assert_eq!(table.slots[slot as usize].state, SlotState::Free);
    }
    // Every surviving record is a ring by the invariants above; this says which rings can survive.
    // A region no more than one hold set ever named is one a merge reaches — its sole holder either
    // absorbs it, seals and folds it in, or drops the last hold — so a survivor was shared once.
    for id in table.sealed.ids() {
        let record = table.sealed.get(id).unwrap();
        assert!(
            record.peak_holders >= 2,
            "record {id:?} survived the wind-down having never had a second holder"
        );
    }
    assert_eq!(table.is_empty(), table.sealed.is_empty());
    table.merges
}

proptest! {
    // No failure-persistence file: a regression file would record generated cases into the source
    // tree, and resolving its path calls `getcwd`, which Miri's isolation refuses. Under Miri the
    // case count drops to what a slate run can afford — the shapes are what matter there, not the
    // breadth, which the native run already covers.
    #![proptest_config(ProptestConfig {
        cases: if cfg!(miri) { 8 } else { ProptestConfig::default().cases },
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn no_interleaving_strands_a_slot_or_desynchronizes_the_two_tiers(
        verbs in proptest::collection::vec(verb(), 1..40)
    ) {
        run(&verbs);
    }
}

/// The generated corpus reaches every locality merge, rather than only being able to.
///
/// An interleaving invariant test is only worth what its runs cover: without this, a merge that
/// never fired would look exactly like a merge that always held. Miri skips the assertion — four
/// cases is what a slate run affords, and that is too few to reach all three shapes reliably.
#[test]
fn each_merge_fires_across_generated_interleavings() {
    use proptest::strategy::{Strategy, ValueTree};
    use proptest::test_runner::TestRunner;

    let cases = if cfg!(miri) { 4 } else { 256 };
    let strategy = proptest::collection::vec(state_verb(), 1..40);
    let mut runner = TestRunner::deterministic();
    let mut total = Merges::default();

    for _ in 0..cases {
        let verbs = strategy.new_tree(&mut runner).unwrap().current();
        let merges = run(&verbs);
        total.into_cell += merges.into_cell;
        total.at_seal += merges.at_seal;
        total.into_namer += merges.into_namer;
    }

    if !cfg!(miri) {
        assert!(
            total.into_cell > 0,
            "no run absorbed a cell into its holder"
        );
        assert!(
            total.at_seal > 0,
            "no run absorbed a count-1 record at a seal"
        );
        assert!(total.into_namer > 0, "no run sealed a cell into its namer");
    }
}
