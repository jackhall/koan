//! The liveness-matrix invariants, over random interleavings of the verbs. The hand-written tests
//! pin shapes; this one pins that no order of `create` / `hold` / `alloc_into` / `release` can
//! break the two conditions the whole model rests on
//! ([liveness-matrix.md § Invariants](../../../design/liveness-matrix.md#invariants)):
//!
//! - a reclaimed slot is named by no occupied cell's row, in either relation;
//! - a cell resident after its declared death is named by at least one occupied cell's row —
//!   otherwise the reclaim gate missed it, and the slab leaks a slot.

use proptest::prelude::*;

use super::super::*;
use super::{Number, Owned};

const CAP: u32 = 6;

/// One verb, with its operands as indices into the handles minted so far — so a generated run
/// names cells that may since have died, which is the point: a stale operand must be refused, not
/// mis-applied.
#[derive(Clone, Debug)]
enum Verb {
    Create { parent: Option<usize> },
    Hold { holder: usize, held: usize },
    Place { producer: usize, consumer: usize },
    Release { cell: usize },
}

fn verb() -> impl Strategy<Value = Verb> {
    prop_oneof![
        proptest::option::of(0..8usize).prop_map(|parent| Verb::Create { parent }),
        (0..8usize, 0..8usize).prop_map(|(holder, held)| Verb::Hold { holder, held }),
        (0..8usize, 0..8usize).prop_map(|(producer, consumer)| Verb::Place { producer, consumer }),
        (0..8usize).prop_map(|cell| Verb::Release { cell }),
    ]
}

/// Every slot the slab currently considers reusable must be named by nothing occupied, and every
/// slot still resident after its death must be named by something occupied.
fn check_invariants(table: &CellTable<Owned>) {
    let occupied: Vec<u32> = (0..CAP)
        .filter(|slot| table.slots[*slot as usize].state != SlotState::Free)
        .collect();
    for slot in 0..CAP {
        let named = table.birth.held_by_any(occupied.iter().copied(), slot)
            || table.pins.held_by_any(occupied.iter().copied(), slot);
        match table.slots[slot as usize].state {
            SlotState::Free => {
                assert!(
                    !named,
                    "slot {slot} is free but an occupied row still names it"
                )
            }
            SlotState::Dead => {
                assert!(
                    named,
                    "slot {slot} is resident but no occupied row names it"
                )
            }
            SlotState::Live => {}
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
    fn no_interleaving_reclaims_a_named_cell_or_strands_an_unnamed_one(
        verbs in proptest::collection::vec(verb(), 1..40)
    ) {
        let mut table: CellTable<Owned> = CellTable::new(CAP);
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
                Verb::Release { cell } => {
                    if let Some(cell) = minted.get(cell).copied() {
                        let _ = table.release(cell);
                    }
                }
            }
            check_invariants(&table);
        }

        // Winding the run down: once every cell's death is declared, the cascade returns every
        // slot the run did not tie into a ring.
        for handle in &minted {
            let _ = table.release(*handle);
        }
        check_invariants(&table);
    }
}
