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
crate::reattachable!(Borrowed => &'cell u32);

/// A family whose erased form has drop glue, so a reclaim's release of the slot is observable.
struct Counted;
crate::reattachable!(Counted => Rc<()>);

/// A value family: a borrow into region storage, so a carrier's reach is a real cross-cell edge.
struct Number;
crate::reattachable!(Number => &'cell u32);
impl DropFree for Number {}

/// A value nesting a borrow of storage outside the graph under a region borrow, so a keep and a
/// redeem carry a reference the retype must leave alone beside one it moves.
#[derive(Clone, Copy)]
struct Entry<'graph, 'cell> {
    program: &'graph u32,
    count: &'cell u32,
}

/// The family of an [`Entry`] behind a region borrow.
struct Listing;
crate::reattachable!(Listing => &'cell Entry<'graph, 'cell>);
impl DropFree for Listing {}

const ANCHOR: u32 = 7;

/// The crossing verdict every test that is not about the crossing itself passes: pin, always. It
/// is the answer an embedder with no price to weigh gives, and it makes a placement's reach the
/// union of its operands' — what the hold-relation tests are written against.
fn pin(_: Prices) -> Verdict {
    Verdict::Pin
}

/// An operand at a stated copy cost — the half of the price the substrate cannot know.
fn operand_at<'a, 'step, V: Reattachable<'static> + DropFree>(
    carrier: &'a Ready<'static, 'step, V>,
    copy_bytes: usize,
) -> Operand<'static, 'a, 'step, V> {
    Operand {
        carrier,
        copy_bytes,
    }
}

/// An operand at no stated copy cost — what a test that never expects a `Copy` verdict passes.
fn operand<'a, 'step, V: Reattachable<'static> + DropFree>(
    carrier: &'a Ready<'static, 'step, V>,
) -> Operand<'static, 'a, 'step, V> {
    operand_at(carrier, 0)
}

/// The pinned view of a crossed operand. Every test that uses it runs under [`pin`], so the copy
/// arm is unreachable rather than merely unexpected.
fn pinned<'cell, V: Reattachable<'static>>(
    view: &CrossedOperand<'static, 'cell, '_, V>,
) -> V::At<'cell>
where
    V::At<'cell>: Copy,
{
    match view {
        CrossedOperand::Pinned(value) => *value,
        CrossedOperand::Copied(_) => unreachable!("the test's verdict always pins"),
    }
}

/// The deep copy a severed view allows and the embed a pinned one allows, in one build — what a
/// test that runs under both verdicts passes.
fn take<'cell>(
    view: &CrossedOperand<'static, 'cell, '_, Number>,
    writer: Writer<'cell>,
) -> &'cell u32 {
    match view {
        // Pinned: the borrow itself, embedded in the destination's storage.
        CrossedOperand::Pinned(value) => value,
        // Copied: severed, so the only thing that typechecks is a fresh allocation.
        CrossedOperand::Copied(value) => one(writer, **value),
    }
}

/// One value, laid down through the writer's single run verb — the shape an embedder derives its
/// own one-value write from, and what these tests use in place of one.
fn one<'cell, T>(writer: Writer<'cell>, value: T) -> &'cell T {
    let mut value = Some(value);
    &writer.fill(1, |_| value.take().expect("a run of one fills once"))[0]
}

/// A `Number` carrier homed in the executing cell: the own-region write, then the bridge to a
/// carrier. What a test that wants a value living where the step runs does.
fn number_here<'step, C: Reattachable<'static>>(
    context: &StepContext<'static, 'step, '_, C>,
    value: u32,
) -> Ready<'static, 'step, Number> {
    context.lift::<Number>(one(context.writer(), value))
}

/// What a view reads, whichever brand it arrived at — for a build that only needs the number.
fn number(view: &CrossedOperand<'static, '_, '_, Number>) -> u32 {
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
fn live_bytes<C: Reattachable<'static>>(graph: &CellGraph<'static, C>, cap: u32) -> usize {
    (0..cap)
        .filter(|slot| graph.cells.slots[*slot as usize].state != SlabState::Free)
        .map(|slot| graph.regions.slab_bytes(slot))
        .sum()
}

/// The reach a kept carrier interned, read out of the entry it landed in — the same lookup a
/// redeem performs, minus the entitlement check.
///
/// A continuation carries no mask of its own, so a `keep` over the same storage is the specimen
/// the seal transition's rewrite and every merge's mask maintenance are read through.
fn kept_reach<'a, C: Reattachable<'static>, T: Reattachable<'static> + DropFree>(
    graph: &'a CellGraph<'static, C>,
    dormant: &Dormant<'static, T>,
) -> &'a GraphReach<1> {
    let key = dormant.key();
    let CellHandle::Slab(home) = key.home else {
        panic!("a value homed in a tree cell interns no reach")
    };
    let Some(SlabForward::Slab { slot, first_index }) = graph.cells.locate(home) else {
        panic!("the kept carrier's home is still in the slab")
    };
    graph.cells.slots[slot as usize]
        .reaches
        .get(first_index + key.index)
        .expect("a relocated key names an entry of the reach table it landed in")
}

/// What a slot currently holds, by handle — the state assertions read the slab directly, since
/// residence is not observable through the public verbs.
fn state_of<'graph, C: Reattachable<'graph>>(
    graph: &CellGraph<'graph, C>,
    handle: SlabHandle,
) -> SlabState {
    graph.cells.slots[handle.slot() as usize].state
}

#[test]
fn the_slab_refuses_past_its_cap_and_reuses_a_freed_slot() {
    let mut graph: CellGraph<'static, Owned> = CellGraph::new(4, pin);
    let cells: Vec<SlabHandle> = (0..4).map(|_| graph.create(None, None).unwrap()).collect();
    assert_eq!(graph.create(None, None), Err(CreateError::SlabFull));

    graph
        .release(cells[1], ReleaseAbsorption::IntoHolder)
        .unwrap();
    let reused = graph.create(None, None).unwrap();
    assert_eq!(reused.slot(), cells[1].slot());
    assert_eq!(reused.generation(), cells[1].generation() + 1);
    assert_eq!(graph.create(None, None), Err(CreateError::SlabFull));
}

#[test]
fn a_cap_below_the_width_binds_admission_and_the_signal() {
    // The width is fixed by the graph's type and the cap by its construction. A graph two cells
    // deep over a 64-cell row is full at two, and the occupancy signal reports two.
    let mut graph: CellGraph<'static, Owned> = CellGraph::new(2, pin);
    let _ = graph.create(None, None).unwrap();
    let _ = graph.create(None, None).unwrap();
    assert_eq!(graph.create(None, None), Err(CreateError::SlabFull));
    assert_eq!(graph.cells.occupancy().cap, 2);
}

#[test]
#[should_panic(expected = "does not fit a 64-cell slab")]
fn a_cap_above_the_width_is_refused_at_construction() {
    let _: CellGraph<'static, Owned> = CellGraph::new(65, pin);
}

#[test]
fn a_two_word_graph_names_slots_across_the_chunk_boundary() {
    // The shape that exercises the matrices' chunk arithmetic: a birth chain and a pin whose ends
    // sit in different chunks of the same row.
    let mut graph: CellGraph<'static, Owned, 2> = CellGraph::new(128, pin);
    let root = graph.create(None, None).unwrap();
    let cells: Vec<SlabHandle> = (1..128)
        .map(|_| graph.create(Some(root), None).unwrap())
        .collect();
    assert_eq!(graph.create(None, None), Err(CreateError::SlabFull));

    // A child born in the high chunk inherits the row of a parent in the low one.
    let high = cells[99];
    assert_eq!(high.slot(), 100);
    assert!(graph.cells.birth.test(high.slot(), root.slot()));

    // And a pin crosses the boundary the other way.
    let low = cells[2];
    graph
        .enter(low, |context| context.hold(high).unwrap())
        .unwrap();
    assert!(graph.cells.holds(low, high));

    // Releasing the holder drops the whole row, both chunks of it, so the held cell reclaims.
    graph.release(low, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(!graph.cells.holds(low, high));
    graph.release(high, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(!graph.is_live(high));
    assert_eq!(graph.cells.occupancy().sealed_cells, 0);
}

#[test]
fn every_verb_rejects_a_stale_handle() {
    let mut graph: CellGraph<'static, Owned> = CellGraph::new(1, pin);
    let first = graph.create(None, None).unwrap();
    graph.release(first, ReleaseAbsorption::IntoHolder).unwrap();
    let second = graph.create(None, None).unwrap();

    assert_eq!(second.slot(), first.slot());
    assert!(!graph.is_live(first));
    assert_eq!(
        graph.enter(first, |_| ()),
        Err(EnterError::Stale(Stale(CellHandle::Slab(first))))
    );
    assert_eq!(
        graph.release(first, ReleaseAbsorption::IntoHolder),
        Err(ReleaseError::Stale(Stale(first)))
    );
    assert_eq!(
        graph.create(Some(first), None),
        Err(CreateError::StaleParent(Stale(first)))
    );
    assert!(graph.is_live(second));
}

#[test]
fn a_birth_row_contains_the_parent_chain_and_outlives_the_middle_cell() {
    let mut graph: CellGraph<'static, Owned> = CellGraph::new(4, pin);
    let a = graph.create(None, None).unwrap();
    let b = graph.create(Some(a), None).unwrap();
    let c = graph.create(Some(b), None).unwrap();

    assert!(graph.cells.birth.row_contains(b.slot(), a.slot()));
    assert!(graph.cells.birth.test(b.slot(), a.slot()));
    assert!(graph.cells.birth.row_contains(c.slot(), b.slot()));
    assert!(graph.cells.birth.test(c.slot(), b.slot()));
    assert!(graph.cells.birth.test(c.slot(), a.slot()));

    graph.release(b, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(!graph.is_live(b));
    assert_eq!(graph.cells.slots[b.slot() as usize].state, SlabState::Dead);
    assert!(graph.cells.birth.test(c.slot(), a.slot()));
    assert!(graph.is_live(a));

    graph.release(c, ReleaseAbsorption::IntoHolder).unwrap();
    assert_eq!(graph.cells.slots[c.slot() as usize].state, SlabState::Free);
    assert_eq!(graph.cells.slots[b.slot() as usize].state, SlabState::Free);
    assert!(graph.is_live(a));

    graph.release(a, ReleaseAbsorption::IntoHolder).unwrap();
    assert_eq!(graph.cells.free.len(), 4);
}

#[test]
fn a_cell_without_a_continuation_is_storage_only() {
    let mut graph: CellGraph<'static, Owned> = CellGraph::new(2, pin);
    let cell = graph.create(None, None).unwrap();
    let seen = graph
        .enter(cell, |context| {
            assert!(context.continuation().is_none());
            context.cell()
        })
        .unwrap();
    assert_eq!(seen, CellHandle::Slab(cell));
}

#[test]
fn the_continuation_comes_back_re_anchored_at_the_step_brand() {
    let mut graph: CellGraph<'static, Borrowed> = CellGraph::new(2, pin);
    let cell = graph.create(None, Some(&ANCHOR)).unwrap();

    let read = graph
        .enter(cell, |context| *context.continuation().unwrap())
        .unwrap();
    assert_eq!(read, 7);

    let emptied = graph
        .enter(cell, |context| context.continuation().is_none())
        .unwrap();
    assert!(emptied);
}

#[test]
fn a_step_stores_the_successor_the_next_step_receives() {
    let mut graph: CellGraph<'static, Owned> = CellGraph::new(2, pin);
    let cell = graph.create(None, None).unwrap();

    graph
        .enter(cell, |context| {
            context.store_successor(String::from("second"));
        })
        .unwrap();
    let next = graph.enter(cell, |context| context.continuation()).unwrap();
    assert_eq!(next.as_deref(), Some("second"));
}

#[test]
fn a_cell_is_entered_by_one_step_at_a_time() {
    let mut graph: CellGraph<'static, Owned> = CellGraph::new(2, pin);
    let cell = graph.create(None, None).unwrap();

    graph.cells.begin(CellHandle::Slab(cell)).unwrap();
    assert_eq!(
        graph.cells.begin(CellHandle::Slab(cell)),
        Err(EnterError::AlreadyExecuting)
    );
    assert_eq!(
        graph.release(cell, ReleaseAbsorption::IntoHolder),
        Err(ReleaseError::Executing)
    );

    graph.cells.executing.clear(cell.slot());
    assert!(graph.enter(cell, |_| ()).is_ok());
}

#[test]
fn the_executing_flag_falls_when_a_step_panics() {
    let mut graph: CellGraph<'static, Owned> = CellGraph::new(1, pin);
    let cell = graph.create(None, None).unwrap();

    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = graph.enter(cell, |_| panic!("the step gives up"));
    }));
    std::panic::set_hook(hook);

    assert!(outcome.is_err());
    assert!(!graph.cells.executing.test(cell.slot()));
    assert!(graph.enter(cell, |_| ()).is_ok());
}

#[test]
fn reclaiming_a_slot_drops_the_continuation_it_held() {
    let anchor = Rc::new(());
    let mut graph: CellGraph<'static, Counted> = CellGraph::new(1, pin);
    let cell = graph.create(None, Some(Rc::clone(&anchor))).unwrap();
    assert_eq!(Rc::strong_count(&anchor), 2);

    graph.release(cell, ReleaseAbsorption::IntoHolder).unwrap();
    assert_eq!(Rc::strong_count(&anchor), 1);
}
