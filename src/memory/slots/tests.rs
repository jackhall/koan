//! Slot-array tests: the three-state transitions, the live-claim counter, and the region-resident
//! shape.

use std::cell::Cell;

use super::{SlotArray, SlotConflict, SlotState};
use crate::memory::tests::in_cell;

/// A stand-in payload and producer — both `Copy` and drop-free, the tier a region-resident slot
/// admits.
type Cells<'cell> = SlotArray<'cell, u32, u8>;

#[test]
fn cells_start_empty_and_unclaimed() {
    in_cell(|writer| {
        let cells: Cells<'_> = SlotArray::new(writer, 3);
        assert_eq!(cells.len(), 3);
        assert_eq!(cells.claimed_count(), 0);
        assert!(cells.iter().all(|(_, cell)| cell.bound().is_none()));
    });
}

/// The ordinary life of a slot: claimed by a binder, then bound by that binder's commit — which
/// retires the claim by replacing it, so the count returns to zero with nothing removed.
#[test]
fn a_commit_retires_its_own_claim() {
    in_cell(|writer| {
        let cells: Cells<'_> = SlotArray::new(writer, 2);

        cells.claim(1, 7).expect("an empty slot admits a claim");
        assert_eq!(cells.claimed_count(), 1);
        assert_eq!(cells.get(1).claimed_by(), Some(7));

        cells.bind(1, 42).expect("a claimed slot admits its bind");
        assert_eq!(cells.claimed_count(), 0);
        assert_eq!(cells.get(1).bound(), Some(42));
        assert_eq!(cells.get(1).claimed_by(), None);
    });
}

/// A binder that terminalizes without committing drops its claim, and the slot is writable again.
#[test]
fn an_unsatisfied_claim_retires_to_empty() {
    in_cell(|writer| {
        let cells: Cells<'_> = SlotArray::new(writer, 1);
        cells.claim(0, 7).expect("an empty slot admits a claim");
        cells.retire_claim(0);
        assert_eq!(cells.claimed_count(), 0);
        assert!(cells.get(0).claimed_by().is_none());
        cells
            .claim(0, 8)
            .expect("a retired slot is claimable again");
        assert_eq!(cells.claimed_count(), 1);
    });
}

/// Retiring a *bound* slot is a no-op: its commit already retired the claim it satisfied, and the
/// binding stands.
#[test]
fn retiring_a_bound_slot_keeps_the_binding() {
    in_cell(|writer| {
        let cells: Cells<'_> = SlotArray::new(writer, 1);
        cells.bind(0, 42).expect("an empty slot admits a bind");
        cells.retire_claim(0);
        assert_eq!(cells.get(0).bound(), Some(42));
        assert_eq!(cells.claimed_count(), 0);
    });
}

/// Both conflicts, each carrying what a caller rules on: a standing claim hands back its producer
/// (so a same-binder re-entry is distinguishable from a collision), and a bound slot is bind-once.
#[test]
fn conflicts_name_what_stands() {
    in_cell(|writer| {
        let cells: Cells<'_> = SlotArray::new(writer, 2);

        cells.claim(0, 7).expect("an empty slot admits a claim");
        assert_eq!(cells.claim(0, 9), Err(SlotConflict::Claimed(7)));
        assert_eq!(cells.claimed_count(), 1, "a refused claim adds none");

        cells.bind(1, 42).expect("an empty slot admits a bind");
        assert_eq!(cells.bind(1, 43), Err(SlotConflict::Bound));
        assert_eq!(cells.claim(1, 7), Err(SlotConflict::Bound));
        assert_eq!(
            cells.get(1).bound(),
            Some(42),
            "a refused write changes nothing"
        );
    });
}

/// A slot is drop-free whatever it holds — the fact the region-resident array rests on, stated
/// against the slot type the way the array's own constructor asserts it.
#[test]
fn cells_carry_no_drop_glue() {
    assert!(!std::mem::needs_drop::<Cell<SlotState<u32, u8>>>());
    assert!(!std::mem::needs_drop::<Cells<'static>>());
}

/// The array is `Copy` and its counter lives in the region, so a write through one copy is seen
/// through every other — what lets a continuation capture the array by value.
#[test]
fn copies_share_slots_and_counter() {
    in_cell(|writer| {
        let cells: Cells<'_> = SlotArray::new(writer, 1);
        let captured = cells;
        captured.claim(0, 7).expect("an empty slot admits a claim");
        assert_eq!(cells.claimed_count(), 1);
        assert_eq!(cells.get(0).claimed_by(), Some(7));
    });
}
