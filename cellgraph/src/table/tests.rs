mod absorption;
mod pricing;
mod properties;
mod sealing;
mod values;

use super::*;
use std::rc::Rc;

/// A family with nothing borrowed, for the tests that only care about the slab.
struct Owned;
crate::reattachable!(Owned => String);

/// A family that borrows, so the erase-and-re-anchor round trip moves a real reference.
struct Borrowed;
crate::reattachable!(Borrowed => &'r u32);

/// A family whose erased form has drop glue, so a reclaim's release of the slot is observable.
struct Counted;
crate::reattachable!(Counted => Rc<()>);

/// A value family: a borrow into region storage, so a carrier's reach is a real cross-cell edge.
struct Number;
crate::reattachable!(Number => &'r u32);
impl DropFree for Number {}

const ANCHOR: u32 = 7;

/// Bytes the slab tier holds, across every occupied cell's region bundle.
///
/// A release may move these bytes between live cells (a merge into a holder), out to a record (a
/// seal), or nowhere at all (a reclaim) — so the total is non-increasing across one. An increase
/// would mean storage flowed back out of the sealed tier, which no path may do.
fn live_bytes<C: Reattachable>(table: &CellTable<C>, cap: u32) -> usize {
    (0..cap)
        .filter(|slot| table.slots[*slot as usize].state != SlotState::Free)
        .filter_map(|slot| table.slots[slot as usize].region.as_ref())
        .map(Region::allocated_bytes)
        .sum()
}

/// What a slot currently holds, by handle — the state assertions read the slab directly, since
/// residence is not observable through the public verbs.
fn state_of<C: Reattachable>(table: &CellTable<C>, handle: Handle) -> SlotState {
    table.slots[handle.slot() as usize].state
}

#[test]
fn the_slab_refuses_past_its_cap_and_reuses_a_freed_slot() {
    let mut table: CellTable<Owned> = CellTable::new(4);
    let cells: Vec<Handle> = (0..4).map(|_| table.create(None, None).unwrap()).collect();
    assert_eq!(table.create(None, None), Err(CreateError::SlabFull));

    table.release(cells[1], Absorption::IntoHolder).unwrap();
    let reused = table.create(None, None).unwrap();
    assert_eq!(reused.slot(), cells[1].slot());
    assert_eq!(reused.generation(), cells[1].generation() + 1);
    assert_eq!(table.create(None, None), Err(CreateError::SlabFull));
}

#[test]
fn every_verb_rejects_a_stale_handle() {
    let mut table: CellTable<Owned> = CellTable::new(1);
    let first = table.create(None, None).unwrap();
    table.release(first, Absorption::IntoHolder).unwrap();
    let second = table.create(None, None).unwrap();

    assert_eq!(second.slot(), first.slot());
    assert!(!table.is_live(first));
    assert_eq!(
        table.enter(first, |_| ()),
        Err(EnterError::Stale(StaleHandle(first)))
    );
    assert_eq!(
        table.release(first, Absorption::IntoHolder),
        Err(ReleaseError::Stale(StaleHandle(first)))
    );
    assert_eq!(
        table.create(Some(first), None),
        Err(CreateError::StaleParent(StaleHandle(first)))
    );
    assert!(table.is_live(second));
}

#[test]
fn a_birth_row_contains_the_parent_chain_and_outlives_the_middle_cell() {
    let mut table: CellTable<Owned> = CellTable::new(4);
    let a = table.create(None, None).unwrap();
    let b = table.create(Some(a), None).unwrap();
    let c = table.create(Some(b), None).unwrap();

    assert!(table.birth.row_contains(b.slot(), a.slot()));
    assert!(table.birth.test(b.slot(), a.slot()));
    assert!(table.birth.row_contains(c.slot(), b.slot()));
    assert!(table.birth.test(c.slot(), b.slot()));
    assert!(table.birth.test(c.slot(), a.slot()));

    table.release(b, Absorption::IntoHolder).unwrap();
    assert!(!table.is_live(b));
    assert_eq!(table.slots[b.slot() as usize].state, SlotState::Dead);
    assert!(table.birth.test(c.slot(), a.slot()));
    assert!(table.is_live(a));

    table.release(c, Absorption::IntoHolder).unwrap();
    assert_eq!(table.slots[c.slot() as usize].state, SlotState::Free);
    assert_eq!(table.slots[b.slot() as usize].state, SlotState::Free);
    assert!(table.is_live(a));

    table.release(a, Absorption::IntoHolder).unwrap();
    assert_eq!(table.free.len(), 4);
}

#[test]
fn a_cell_without_a_continuation_is_storage_only() {
    let mut table: CellTable<Owned> = CellTable::new(2);
    let cell = table.create(None, None).unwrap();
    let seen = table
        .enter(cell, |context| {
            assert!(context.continuation().is_none());
            context.handle()
        })
        .unwrap();
    assert_eq!(seen, cell);
}

#[test]
fn the_continuation_comes_back_re_anchored_at_the_step_brand() {
    let mut table: CellTable<Borrowed> = CellTable::new(2);
    let cell = table.create(None, Some(&ANCHOR)).unwrap();

    let read = table
        .enter(cell, |context| *context.continuation().unwrap().value())
        .unwrap();
    assert_eq!(read, 7);

    let emptied = table
        .enter(cell, |context| context.continuation().is_none())
        .unwrap();
    assert!(emptied);
}

#[test]
fn a_step_stores_the_successor_the_next_step_receives() {
    let mut table: CellTable<Owned> = CellTable::new(2);
    let cell = table.create(None, None).unwrap();

    table
        .enter(cell, |context| {
            context.store_successor(String::from("second"));
        })
        .unwrap();
    let next = table
        .enter(cell, |context| {
            context.continuation().map(|opened| opened.into_value())
        })
        .unwrap();
    assert_eq!(next.as_deref(), Some("second"));
}

#[test]
fn a_cell_is_entered_by_one_step_at_a_time() {
    let mut table: CellTable<Owned> = CellTable::new(2);
    let cell = table.create(None, None).unwrap();

    table.begin(cell).unwrap();
    assert_eq!(table.begin(cell), Err(EnterError::AlreadyExecuting));
    assert_eq!(
        table.release(cell, Absorption::IntoHolder),
        Err(ReleaseError::Executing)
    );

    table.executing.clear(cell.slot());
    assert!(table.enter(cell, |_| ()).is_ok());
}

#[test]
fn the_executing_flag_falls_when_a_step_panics() {
    let mut table: CellTable<Owned> = CellTable::new(1);
    let cell = table.create(None, None).unwrap();

    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = table.enter(cell, |_| panic!("the step gives up"));
    }));
    std::panic::set_hook(hook);

    assert!(outcome.is_err());
    assert!(!table.executing.test(cell.slot()));
    assert!(table.enter(cell, |_| ()).is_ok());
}

#[test]
fn reclaiming_a_slot_drops_the_continuation_it_held() {
    let anchor = Rc::new(());
    let mut table: CellTable<Counted> = CellTable::new(1);
    let cell = table.create(None, Some(Rc::clone(&anchor))).unwrap();
    assert_eq!(Rc::strong_count(&anchor), 2);

    table.release(cell, Absorption::IntoHolder).unwrap();
    assert_eq!(Rc::strong_count(&anchor), 1);
}
