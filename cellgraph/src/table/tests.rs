mod absorption;
mod prices;
mod pricing;
mod properties;
mod scratch;
mod sealing;
mod tree;
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

/// The crossing verdict every test that is not about the crossing itself passes: pin, always. It
/// is the answer an embedder with no price to weigh gives, and it makes a placement's reach the
/// union of its operands' — what the hold-relation tests are written against.
fn pin(_: Prices) -> Verdict {
    Verdict::Pin
}

/// An operand at a stated copy cost — the half of the price the substrate cannot know.
fn operand_at<'a, 'b, V: Reattachable + DropFree>(
    carrier: &'a Ready<'b, V>,
    copy_bytes: usize,
) -> Operand<'a, 'b, V> {
    Operand {
        carrier,
        copy_bytes,
    }
}

/// An operand at no stated copy cost — what a test that never expects a `Copy` verdict passes.
fn operand<'a, 'b, V: Reattachable + DropFree>(carrier: &'a Ready<'b, V>) -> Operand<'a, 'b, V> {
    operand_at(carrier, 0)
}

/// The pinned view of a crossed operand. Every test that uses it runs under [`pin`], so the copy
/// arm is unreachable rather than merely unexpected.
fn pinned<'r, V: Reattachable>(view: &CrossedOperand<'r, '_, V>) -> V::At<'r>
where
    V::At<'r>: Copy,
{
    match view {
        CrossedOperand::Pinned(value) => *value,
        CrossedOperand::Copied(_) => unreachable!("the test's verdict always pins"),
    }
}

/// The deep copy a severed view allows and the embed a pinned one allows, in one build — what a
/// test that runs under both verdicts passes.
fn take<'r>(view: &CrossedOperand<'r, '_, Number>, writer: Writer<'r>) -> &'r u32 {
    match view {
        // Pinned: the borrow itself, embedded in the destination's storage.
        CrossedOperand::Pinned(value) => value,
        // Copied: severed, so the only thing that typechecks is a fresh allocation.
        CrossedOperand::Copied(value) => writer.value(**value),
    }
}

/// What a view reads, whichever brand it arrived at — for a build that only needs the number.
fn number(view: &CrossedOperand<'_, '_, Number>) -> u32 {
    match view {
        CrossedOperand::Pinned(value) => **value,
        CrossedOperand::Copied(value) => **value,
    }
}

/// Bytes the slab tier holds, across every occupied cell's region bundle.
///
/// A release may move these bytes between live cells (a merge into a holder), out to a sealed cell
/// (a seal), or nowhere at all (a reclaim) — so the total is non-increasing across one. An increase
/// would mean storage flowed back out of the sealed tier, which no path may do.
fn live_bytes<C: Reattachable>(table: &CellTable<C>, cap: u32) -> usize {
    (0..cap)
        .filter(|slot| table.slots[*slot as usize].state != SlabState::Free)
        .filter_map(|slot| table.slots[slot as usize].region.as_ref())
        .map(Region::allocated_bytes)
        .sum()
}

/// The reach of a cell's stored continuation, read out of the reach table entry it occupies. The
/// continuation is a dormant carrier like any other, so this is the same lookup a redeem performs.
fn continuation_reach_index<C: Reattachable>(
    table: &CellTable<C>,
    handle: SlabHandle,
) -> &GraphReach<1> {
    let cell = &table.slots[handle.slot() as usize];
    let index = cell
        .continuation_reach_index
        .expect("the cell stored a continuation over captures");
    cell.reaches
        .get(index)
        .expect("the entry the continuation names is in the table")
}

/// What a slot currently holds, by handle — the state assertions read the slab directly, since
/// residence is not observable through the public verbs.
fn state_of<C: Reattachable>(table: &CellTable<C>, handle: SlabHandle) -> SlabState {
    table.slots[handle.slot() as usize].state
}

#[test]
fn the_slab_refuses_past_its_cap_and_reuses_a_freed_slot() {
    let mut table: CellTable<Owned> = CellTable::new(4, pin);
    let cells: Vec<SlabHandle> = (0..4).map(|_| table.create(None, None).unwrap()).collect();
    assert_eq!(table.create(None, None), Err(CreateError::SlabFull));

    table
        .release(cells[1], ReleaseAbsorption::IntoHolder)
        .unwrap();
    let reused = table.create(None, None).unwrap();
    assert_eq!(reused.slot(), cells[1].slot());
    assert_eq!(reused.generation(), cells[1].generation() + 1);
    assert_eq!(table.create(None, None), Err(CreateError::SlabFull));
}

#[test]
fn a_cap_below_the_width_binds_admission_and_the_signal() {
    // The width is fixed by the table's type and the cap by its construction. A table two cells
    // deep over a 64-cell row is full at two, and the occupancy signal reports two.
    let mut table: CellTable<Owned> = CellTable::new(2, pin);
    let _ = table.create(None, None).unwrap();
    let _ = table.create(None, None).unwrap();
    assert_eq!(table.create(None, None), Err(CreateError::SlabFull));
    assert_eq!(table.occupancy().cap, 2);
}

#[test]
#[should_panic(expected = "does not fit a 64-cell slab")]
fn a_cap_above_the_width_is_refused_at_construction() {
    let _: CellTable<Owned> = CellTable::new(65, pin);
}

#[test]
fn a_two_word_table_names_slots_across_the_chunk_boundary() {
    // The shape that exercises the matrices' chunk arithmetic: a birth chain and a pin whose ends
    // sit in different chunks of the same row.
    let mut table: CellTable<Owned, 2> = CellTable::new(128, pin);
    let root = table.create(None, None).unwrap();
    let cells: Vec<SlabHandle> = (1..128)
        .map(|_| table.create(Some(root), None).unwrap())
        .collect();
    assert_eq!(table.create(None, None), Err(CreateError::SlabFull));

    // A child born in the high chunk inherits the row of a parent in the low one.
    let high = cells[99];
    assert_eq!(high.slot(), 100);
    assert!(table.birth.test(high.slot(), root.slot()));

    // And a pin crosses the boundary the other way.
    let low = cells[2];
    table
        .enter(low, |context| context.hold(high).unwrap())
        .unwrap();
    assert!(table.holds(low, high));

    // Releasing the holder drops the whole row, both chunks of it, so the held cell reclaims.
    table.release(low, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(!table.holds(low, high));
    table.release(high, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(!table.is_live(high));
    assert_eq!(table.occupancy().sealed_cells, 0);
}

#[test]
fn every_verb_rejects_a_stale_handle() {
    let mut table: CellTable<Owned> = CellTable::new(1, pin);
    let first = table.create(None, None).unwrap();
    table.release(first, ReleaseAbsorption::IntoHolder).unwrap();
    let second = table.create(None, None).unwrap();

    assert_eq!(second.slot(), first.slot());
    assert!(!table.is_live(first));
    assert_eq!(
        table.enter(first, |_| ()),
        Err(EnterError::Stale(Stale(CellHandle::Slab(first))))
    );
    assert_eq!(
        table.release(first, ReleaseAbsorption::IntoHolder),
        Err(ReleaseError::Stale(Stale(first)))
    );
    assert_eq!(
        table.create(Some(first), None),
        Err(CreateError::StaleParent(Stale(first)))
    );
    assert!(table.is_live(second));
}

#[test]
fn a_birth_row_contains_the_parent_chain_and_outlives_the_middle_cell() {
    let mut table: CellTable<Owned> = CellTable::new(4, pin);
    let a = table.create(None, None).unwrap();
    let b = table.create(Some(a), None).unwrap();
    let c = table.create(Some(b), None).unwrap();

    assert!(table.birth.row_contains(b.slot(), a.slot()));
    assert!(table.birth.test(b.slot(), a.slot()));
    assert!(table.birth.row_contains(c.slot(), b.slot()));
    assert!(table.birth.test(c.slot(), b.slot()));
    assert!(table.birth.test(c.slot(), a.slot()));

    table.release(b, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(!table.is_live(b));
    assert_eq!(table.slots[b.slot() as usize].state, SlabState::Dead);
    assert!(table.birth.test(c.slot(), a.slot()));
    assert!(table.is_live(a));

    table.release(c, ReleaseAbsorption::IntoHolder).unwrap();
    assert_eq!(table.slots[c.slot() as usize].state, SlabState::Free);
    assert_eq!(table.slots[b.slot() as usize].state, SlabState::Free);
    assert!(table.is_live(a));

    table.release(a, ReleaseAbsorption::IntoHolder).unwrap();
    assert_eq!(table.free.len(), 4);
}

#[test]
fn a_cell_without_a_continuation_is_storage_only() {
    let mut table: CellTable<Owned> = CellTable::new(2, pin);
    let cell = table.create(None, None).unwrap();
    let seen = table
        .enter(cell, |context| {
            assert!(context.continuation().is_none());
            context.cell()
        })
        .unwrap();
    assert_eq!(seen, CellHandle::Slab(cell));
}

#[test]
fn the_continuation_comes_back_re_anchored_at_the_step_brand() {
    let mut table: CellTable<Borrowed> = CellTable::new(2, pin);
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
    let mut table: CellTable<Owned> = CellTable::new(2, pin);
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
    let mut table: CellTable<Owned> = CellTable::new(2, pin);
    let cell = table.create(None, None).unwrap();

    table.begin(CellHandle::Slab(cell)).unwrap();
    assert_eq!(
        table.begin(CellHandle::Slab(cell)),
        Err(EnterError::AlreadyExecuting)
    );
    assert_eq!(
        table.release(cell, ReleaseAbsorption::IntoHolder),
        Err(ReleaseError::Executing)
    );

    table.executing.clear(cell.slot());
    assert!(table.enter(cell, |_| ()).is_ok());
}

#[test]
fn the_executing_flag_falls_when_a_step_panics() {
    let mut table: CellTable<Owned> = CellTable::new(1, pin);
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
    let mut table: CellTable<Counted> = CellTable::new(1, pin);
    let cell = table.create(None, Some(Rc::clone(&anchor))).unwrap();
    assert_eq!(Rc::strong_count(&anchor), 2);

    table.release(cell, ReleaseAbsorption::IntoHolder).unwrap();
    assert_eq!(Rc::strong_count(&anchor), 1);
}
