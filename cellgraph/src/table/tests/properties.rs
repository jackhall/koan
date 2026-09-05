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
//! - every bit and id of a stored mask is covered by a live cell or a live record — mask validity.

use proptest::prelude::*;

use super::super::*;
use super::{Borrowed, Number};

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
    Release { cell: usize },
}

fn verb() -> impl Strategy<Value = Verb> {
    prop_oneof![
        proptest::option::of(0..8usize).prop_map(|parent| Verb::Create { parent }),
        (0..8usize, 0..8usize).prop_map(|(holder, held)| Verb::Hold { holder, held }),
        (0..8usize, 0..8usize).prop_map(|(producer, consumer)| Verb::Place { producer, consumer }),
        (0..8usize, 0..8usize).prop_map(|(cell, over)| Verb::Keep { cell, over }),
        (0..8usize).prop_map(|cell| Verb::Read { cell }),
        (0..8usize).prop_map(|cell| Verb::Release { cell }),
    ]
}

fn check_invariants(table: &CellTable<Borrowed>) {
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
        let mut table: CellTable<Borrowed> = CellTable::new(CAP);
        let mut minted: Vec<Handle> = Vec::new();

        for step in verbs {
            match step {
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
                                .alloc_into::<Number, Number>(consumer, &[&value], |_w, views| {
                                    views[0]
                                })
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
                            if let Ok(value) = context
                                .alloc_into::<Number, Number>(over, &[], |w, _| w.value(1))
                            {
                                context.store_successor_capturing(&[&value], |_w, views| views[0]);
                            }
                        });
                    }
                }
                // Reading the kept continuation back is where a stale mask would surface: the
                // reach that comes out is derived through whatever the tier has since done to it.
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
                                prop_assert!(
                                    table.slots[named as usize].state != SlotState::Free,
                                    "a read handed back a mask naming the recycled slot {}",
                                    named
                                );
                            }
                            for named in reach.sealed().iter() {
                                prop_assert!(
                                    table.sealed.get(named).is_some(),
                                    "a read handed back a mask naming a retired record"
                                );
                            }
                        }
                    }
                }
                Verb::Release { cell } => {
                    if let Some(cell) = minted.get(cell).copied() {
                        let _ = table.release(cell);
                    }
                }
            }
            check_invariants(&table);
        }

        // Winding the run down: once every cell's death is declared, the cascade returns every
        // slot, and the tier retains only what a ring tied together.
        for handle in &minted {
            let _ = table.release(*handle);
        }
        check_invariants(&table);
        for slot in 0..CAP {
            prop_assert_eq!(table.slots[slot as usize].state, SlotState::Free);
        }
    }
}
