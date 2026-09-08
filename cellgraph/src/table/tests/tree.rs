//! The tree cell: the third region habitat, for a call subtree whose liveness is a stack
//! discipline ([design/tree-cells.md](../../../design/tree-cells.md)).
//!
//! What these pin: the three verbs and their refusals; a chain deeper than the slab cap running to
//! completion on a two-slot slab; the ancestry rule's three answers and the pledge an upward pin
//! leaves; disposal by pledge, in any release order, cascading through dead-but-undisposed
//! ancestors into the slab's own walk; and a dormant carrier redeeming through the tombstone chain
//! after its home's bytes have moved — into a live cell, into a root that later absorbs, seals, or
//! reclaims.

use std::cell::RefCell;
use std::rc::Rc;

use super::super::*;
use super::{Borrowed, Number, Owned, number, operand_at, pin, state_of, take};
use crate::tree::TreeState;

/// An operand the embedder will never copy: at a cost above anything a pin can price, a verdict
/// that weighs the two always pins it.
fn kept_operand<'a, 'b, V: Reattachable + DropFree>(
    carrier: &'a Ready<'b, V>,
) -> Operand<'a, 'b, V> {
    operand_at(carrier, usize::MAX)
}

/// A verdict that records every crossing it is shown. The log is what says a forced copy consulted
/// nobody: an operand the rule copies leaves no entry at all.
fn recording(
    answer: impl Fn(Prices) -> Verdict + 'static,
) -> (
    impl FnMut(Prices) -> Verdict + 'static,
    Rc<RefCell<Vec<Prices>>>,
) {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let log = Rc::clone(&seen);
    let verdict = move |prices: Prices| {
        log.borrow_mut().push(prices);
        answer(prices)
    };
    (verdict, seen)
}

/// Build a number in the cell the step is running in.
fn number_in<'b, C: Reattachable>(
    context: &mut StepContext<'b, C>,
    value: u32,
) -> Ready<'b, Number> {
    context.alloc::<Number>(move |writer| writer.value(value))
}

#[test]
fn a_tree_cell_runs_its_three_verbs_without_taking_a_slab_slot() {
    let mut table: CellTable<Owned> = CellTable::new(1, pin);
    let root = table.create(None, None).unwrap();
    assert_eq!(table.create(None, None), Err(CreateError::SlabFull));

    // The pool takes no cap, so the door has no full refusal to give.
    let tree = table.create_tree(root, Some(String::from("next"))).unwrap();
    assert!(table.is_live(tree));
    assert_eq!(table.tree_children_of(root), 1);

    let (cell, continuation) = table
        .enter(tree, |context| {
            (
                context.cell(),
                context.continuation().map(Active::into_value),
            )
        })
        .unwrap();
    assert_eq!(cell, CellHandle::Tree(tree));
    assert_eq!(continuation.as_deref(), Some("next"));

    table.release_tree(tree).unwrap();
    assert!(!table.is_live(tree));
    assert_eq!(table.tree_children_of(root), 0);
    table.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(table.is_empty());
}

#[test]
fn the_tree_doors_refuse_a_name_kept_past_a_declared_death() {
    let mut table: CellTable<Owned> = CellTable::new(2, pin);
    let root = table.create(None, None).unwrap();
    let tree = table.create_tree(root, None).unwrap();
    table.release_tree(tree).unwrap();

    let Err(stale) = table.create_tree(tree, None) else {
        panic!("a dead tree parent must refuse");
    };
    assert_eq!(stale.name(), CellHandle::Tree(tree));

    let Err(EnterError::Stale(stale)) = table.enter(tree, |_| ()) else {
        panic!("a dead tree cell must refuse the step");
    };
    assert_eq!(stale.name(), CellHandle::Tree(tree));

    let Err(ReleaseTreeError::Stale(stale)) = table.release_tree(tree) else {
        panic!("a second release names a death already declared");
    };
    assert_eq!(stale.name(), tree);

    // And a slab parent whose own death was declared refuses to take a new tree child.
    table.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    let Err(stale) = table.create_tree(root, None) else {
        panic!("a dead root must refuse");
    };
    assert_eq!(stale.name(), CellHandle::Slab(root));
    assert!(table.is_empty());
}

#[test]
fn a_root_released_before_its_tree_child_waits_undisposed() {
    let mut table: CellTable<Owned> = CellTable::new(2, pin);
    let root = table.create(None, None).unwrap();
    let tree = table.create_tree(root, None).unwrap();

    table.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    assert_eq!(state_of(&table, root), SlabState::Dead);
    assert!(!table.is_empty(), "the root waits on its tree child");

    // The last tree child's disposal runs the slab's own cascade at the root.
    table.release_tree(tree).unwrap();
    assert!(table.is_empty());
}

#[test]
fn a_tree_parent_released_first_disposes_when_its_last_child_does() {
    let mut table: CellTable<Owned> = CellTable::new(2, pin);
    let root = table.create(None, None).unwrap();
    let grandparent = table.create_tree(root, None).unwrap();
    let parent = table.create_tree(grandparent, None).unwrap();
    let child = table.create_tree(parent, None).unwrap();

    table.release_tree(grandparent).unwrap();
    table.release_tree(parent).unwrap();
    assert_eq!(table.trees().state(grandparent.index()), TreeState::Dead);
    assert_eq!(table.trees().state(parent.index()), TreeState::Dead);
    assert_eq!(
        table.trees().occupied().count(),
        3,
        "a dead-but-undisposed ancestor keeps its region until its children are gone"
    );

    // One release cascades through both dead-but-undisposed ancestors and into the root's own walk.
    table.release_tree(child).unwrap();
    assert_eq!(table.trees().occupied().count(), 0);
    assert_eq!(table.tree_children_of(root), 0);
    table.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(table.is_empty());
}

#[test]
fn a_tree_cell_nothing_was_kept_in_leaves_no_tombstone() {
    let mut table: CellTable<Owned> = CellTable::new(2, pin);
    let root = table.create(None, None).unwrap();
    let tree = table.create_tree(root, None).unwrap();
    table
        .enter(tree, |context| {
            let value = number_in(context, 1);
            context
                .alloc_into::<Number, Number>(root, &[kept_operand(&value)], |writer, views| {
                    take(&views[0], writer)
                })
                .unwrap();
        })
        .unwrap();

    // The cell pledged its bump to the root, so its bytes moved — but no key names it, so its
    // identity recycles rather than staying behind to answer for them.
    table.release_tree(tree).unwrap();
    assert_eq!(table.trees().occupied().count(), 0);
    assert_eq!(table.tree_tombstones_of(root), None);
    table.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(table.is_empty());
}

#[test]
fn a_tree_homed_operand_crosses_by_where_the_destination_sits() {
    let (verdict, seen) = recording(|_| Verdict::Pin);
    let mut table: CellTable<Owned> = CellTable::new(4, verdict);
    let root = table.create(None, None).unwrap();
    let stranger = table.create(None, None).unwrap();
    let other_root = table.create(None, None).unwrap();
    let elsewhere = table.create_tree(other_root, None).unwrap();

    let parent = table.create_tree(root, None).unwrap();
    let home = table.create_tree(parent, None).unwrap();
    let child = table.create_tree(home, None).unwrap();
    let cousin = table.create_tree(parent, None).unwrap();

    let copied = table
        .enter(home, |context| {
            let value = number_in(context, 5);
            let mut copied = Vec::new();
            for dest in [
                CellHandle::Tree(home),
                CellHandle::Tree(child),
                CellHandle::Tree(parent),
                CellHandle::Slab(root),
                CellHandle::Tree(cousin),
                CellHandle::Tree(elsewhere),
                CellHandle::Slab(stranger),
            ] {
                context
                    .alloc_into::<Number, Number>(dest, &[kept_operand(&value)], |writer, views| {
                        copied.push(matches!(views[0], CrossedOperand::Copied(_)));
                        take(&views[0], writer)
                    })
                    .unwrap();
            }
            copied
        })
        .unwrap();

    // The home itself and a tree cell under it take the ordinary door; the two ancestors take the
    // upward one; the cousin, a cell under another root, and an unrelated slab cell are copies.
    assert_eq!(
        copied,
        vec![false, false, false, false, true, true, true],
        "only a destination off the home's chain forces a copy"
    );
    assert_eq!(
        seen.borrow().len(),
        4,
        "a forced copy consults the verdict for nothing"
    );
    let seen = seen.borrow();
    assert_eq!(seen[0].pin_bytes, 0, "into the home itself");
    assert_eq!(seen[1].pin_bytes, 0, "into a tree cell under the home");
    // Both upward pins are priced at the splice: the home's whole bundle into the parent, and the
    // same bundle again into the root, because the pledge it already carries does not reach that
    // far and the bytes have to travel the extra level.
    assert!(
        seen[2].pin_bytes > 0,
        "into the tree parent, at the splice price"
    );
    assert_eq!(
        seen[3].pin_bytes, seen[2].pin_bytes,
        "a pin past the standing pledge is priced for the further travel"
    );
}

#[test]
fn an_upward_pin_pledges_the_home_and_every_intermediate() {
    let mut table: CellTable<Owned> = CellTable::new(2, pin);
    let root = table.create(None, None).unwrap();
    let parent = table.create_tree(root, None).unwrap();
    let home = table.create_tree(parent, None).unwrap();

    // A grandchild pinned straight into its grandparent: the parent is carried too, or the
    // grandparent's bundle would be left borrowing bytes the parent's reclaim took away.
    let bytes = table
        .enter(home, |context| {
            let value = number_in(context, 9);
            context
                .alloc_into::<Number, Number>(root, &[kept_operand(&value)], |writer, views| {
                    take(&views[0], writer)
                })
                .unwrap();
            context.cell()
        })
        .unwrap();
    assert_eq!(bytes, CellHandle::Tree(home));
    let home_bytes = table.trees().region_bytes(home.index());
    assert!(home_bytes > 0);

    table.release_tree(home).unwrap();
    // The home's bump landed in the root, not in the parent it passed through.
    assert_eq!(table.region_bytes(root).unwrap(), home_bytes);
    assert_eq!(table.trees().region_bytes(parent.index()), 0);

    table.release_tree(parent).unwrap();
    table.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(table.is_empty());
}

#[test]
fn the_shallowest_pledge_wins_and_the_splice_price_is_marginal() {
    let (verdict, seen) = recording(|_| Verdict::Pin);
    let mut table: CellTable<Owned> = CellTable::new(2, verdict);
    let root = table.create(None, None).unwrap();
    let parent = table.create_tree(root, None).unwrap();
    let home = table.create_tree(parent, None).unwrap();

    table
        .enter(home, |context| {
            let first = number_in(context, 1);
            let second = number_in(context, 2);
            // Two operands from one home in one placement: the first carries the whole splice
            // price and the second is shown the margin, which is nothing.
            context
                .alloc_into::<Number, Number>(
                    parent,
                    &[kept_operand(&first), kept_operand(&second)],
                    |writer, views| take(&views[0], writer),
                )
                .unwrap();
            // And then a pin further up. The shallower destination wins, so the bump goes to the
            // root rather than stopping at the parent it was first pledged to.
            context
                .alloc_into::<Number, Number>(root, &[kept_operand(&first)], |writer, views| {
                    take(&views[0], writer)
                })
                .unwrap();
        })
        .unwrap();

    let seen = seen.borrow();
    assert_eq!(seen.len(), 3);
    assert!(
        seen[0].pin_bytes > 0,
        "the first operand carries the splice"
    );
    assert_eq!(seen[1].pin_bytes, 0, "the second is priced at the margin");
    assert!(
        seen[2].pin_bytes > 0,
        "a pin past the pledge is priced again: the bytes travel further"
    );
    drop(seen);

    let home_bytes = table.trees().region_bytes(home.index());
    table.release_tree(home).unwrap();
    assert_eq!(table.region_bytes(root).unwrap(), home_bytes);
    assert_eq!(table.trees().region_bytes(parent.index()), 0);
    table.release_tree(parent).unwrap();
    table.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(table.is_empty());
}

#[test]
fn a_slab_step_placing_into_a_tree_cell_mints_its_root_the_hold() {
    let mut table: CellTable<Owned> = CellTable::new(3, pin);
    let root = table.create(None, None).unwrap();
    let source = table.create(None, None).unwrap();
    let tree = table.create_tree(root, None).unwrap();

    table
        .enter(source, |context| {
            let value = number_in(context, 4);
            context
                .alloc_into::<Number, Number>(tree, &[kept_operand(&value)], |writer, views| {
                    writer.value(number(&views[0]) + 1)
                })
                .unwrap();
        })
        .unwrap();

    // No relation names the tree cell, so the hold the placement takes is the root's — and the
    // bytes went into the tree cell's own bundle, not the root's.
    assert!(table.holds(root, source));
    assert!(table.trees().region_bytes(tree.index()) > 0);
    assert_eq!(table.region_bytes(root).unwrap(), 0);

    table.release_tree(tree).unwrap();
    table
        .release(source, ReleaseAbsorption::IntoHolder)
        .unwrap();
    table.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(table.is_empty());
}

#[test]
fn a_dormant_carrier_kept_in_a_tree_cell_redeems_from_anywhere_under_the_same_root() {
    let mut table: CellTable<Owned> = CellTable::new(3, pin);
    let root = table.create(None, None).unwrap();
    let outsider = table.create(None, None).unwrap();
    let home = table.create_tree(root, None).unwrap();
    let sibling = table.create_tree(root, None).unwrap();

    let kept = table
        .enter(home, |context| {
            let value = number_in(context, 12);
            context.keep(value)
        })
        .unwrap();

    // The home itself, a cousin under the same root, and the root: all entitled by root identity.
    for cell in [CellHandle::Tree(home), CellHandle::Tree(sibling)] {
        let CellHandle::Tree(handle) = cell else {
            unreachable!("the loop names tree cells")
        };
        let read = table
            .enter(handle, |context| {
                *context.read(&context.redeem(kept).unwrap()).value()
            })
            .unwrap();
        assert_eq!(read, 12);
    }
    let read = table
        .enter(root, |context| {
            *context.read(&context.redeem(kept).unwrap()).value()
        })
        .unwrap();
    assert_eq!(read, 12);

    // A cell under no root of this one's is refused.
    let error = table
        .enter(outsider, |context| match context.redeem(kept) {
            Err(error) => error,
            Ok(_) => panic!("a cell under another root has no claim on the home"),
        })
        .unwrap();
    assert_eq!(error, RedeemError::Unheld);

    table.release_tree(home).unwrap();
    table.release_tree(sibling).unwrap();
    table
        .release(outsider, ReleaseAbsorption::IntoHolder)
        .unwrap();
    table.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(table.is_empty());
}

#[test]
fn a_dormant_carrier_whose_home_reclaimed_answers_gone() {
    let mut table: CellTable<Owned> = CellTable::new(2, pin);
    let root = table.create(None, None).unwrap();
    let home = table.create_tree(root, None).unwrap();

    // Kept but never pinned upward, so the cell pledges nothing and its bytes go with it.
    let kept = table
        .enter(home, |context| {
            let value = number_in(context, 3);
            context.keep(value)
        })
        .unwrap();
    table.release_tree(home).unwrap();
    assert_eq!(
        table.trees().occupied().count(),
        0,
        "a reclaim leaves no tombstone"
    );

    let error = table
        .enter(root, |context| match context.redeem(kept) {
            Err(error) => error,
            Ok(_) => panic!("the home's bytes are gone"),
        })
        .unwrap();
    assert_eq!(error, RedeemError::Gone);
    table.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(table.is_empty());
}

/// Build a two-level tree under `root`, keep a value in the deeper one, and splice both bumps into
/// the root — the shape every tombstone-through-the-slab assertion starts from.
fn spliced_into_root(
    table: &mut CellTable<Owned>,
    root: SlabHandle,
) -> (Dormant<Number>, TreeHandle, TreeHandle) {
    let parent = table.create_tree(root, None).unwrap();
    let home = table.create_tree(parent, None).unwrap();
    let kept = table
        .enter(home, |context| {
            let value = number_in(context, 21);
            let kept = context.keep(value.clone());
            context
                .alloc_into::<Number, Number>(root, &[kept_operand(&value)], |writer, views| {
                    take(&views[0], writer)
                })
                .unwrap();
            kept
        })
        .unwrap();
    (kept, parent, home)
}

#[test]
fn a_dormant_carrier_whose_home_was_absorbed_redeems_from_the_destination() {
    let mut table: CellTable<Owned> = CellTable::new(2, pin);
    let root = table.create(None, None).unwrap();
    let (kept, parent, home) = spliced_into_root(&mut table, root);

    table.release_tree(home).unwrap();
    // The home is gone and something still names it, so its identity stays as a tombstone
    // pointing at the root, whose bundle now holds its bump.
    assert_eq!(table.trees().state(home.index()), TreeState::Absorbed);
    assert_eq!(
        table.trees().tombstone_target(home.index()),
        Some(CellHandle::Slab(root))
    );
    assert_eq!(table.tree_tombstones_of(root), Some(home.index()));

    let read = table
        .enter(root, |context| {
            *context.read(&context.redeem(kept).unwrap()).value()
        })
        .unwrap();
    assert_eq!(read, 21);

    table.release_tree(parent).unwrap();
    table.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(table.is_empty(), "the reclaim took the tombstone with it");
}

#[test]
fn a_tree_value_spliced_into_a_root_survives_the_root_absorbing_into_its_holder() {
    let mut table: CellTable<Owned> = CellTable::new(3, pin);
    let holder = table.create(None, None).unwrap();
    let root = table.create(None, None).unwrap();
    let (kept, parent, home) = spliced_into_root(&mut table, root);
    table.release_tree(home).unwrap();
    table.release_tree(parent).unwrap();

    // The holder takes a hold on the root, so the root's death folds its whole bundle — spliced
    // tree bumps included — into the holder rather than sealing it.
    table
        .enter(holder, |context| context.hold(root).unwrap())
        .unwrap();
    table.release(root, ReleaseAbsorption::IntoHolder).unwrap();

    let read = table
        .enter(holder, |context| {
            *context.read(&context.redeem(kept).unwrap()).value()
        })
        .unwrap();
    assert_eq!(read, 21);

    table
        .release(holder, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert!(table.is_empty());
    assert_eq!(table.relocations(), 0);
}

#[test]
fn a_tree_root_that_seals_carries_its_spliced_bumps() {
    let mut table: CellTable<Owned> = CellTable::new(4, pin);
    let first = table.create(None, None).unwrap();
    let second = table.create(None, None).unwrap();
    let root = table.create(None, None).unwrap();
    let (kept, parent, home) = spliced_into_root(&mut table, root);
    table.release_tree(home).unwrap();
    table.release_tree(parent).unwrap();

    // Two holders, so the root seals rather than folding into either.
    for holder in [first, second] {
        table
            .enter(holder, |context| context.hold(root).unwrap())
            .unwrap();
    }
    table.release(root, ReleaseAbsorption::IntoHolder).unwrap();

    let read = table
        .enter(first, |context| {
            *context.read(&context.redeem(kept).unwrap()).value()
        })
        .unwrap();
    assert_eq!(read, 21);

    table.release(first, ReleaseAbsorption::IntoHolder).unwrap();
    table
        .release(second, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert!(table.is_empty());
    assert_eq!(table.relocations(), 0);
}

#[test]
fn a_tree_value_whose_root_reclaimed_answers_gone() {
    let mut table: CellTable<Owned> = CellTable::new(2, pin);
    let root = table.create(None, None).unwrap();
    let (kept, parent, home) = spliced_into_root(&mut table, root);
    table.release_tree(home).unwrap();
    table.release_tree(parent).unwrap();
    table.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(table.is_empty(), "the root's reclaim freed the tombstone");

    let bystander = table.create(None, None).unwrap();
    let error = table
        .enter(bystander, |context| match context.redeem(kept) {
            Err(error) => error,
            Ok(_) => panic!("the root's reclaim took the bytes"),
        })
        .unwrap();
    assert_eq!(error, RedeemError::Gone);
    table
        .release(bystander, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert!(table.is_empty());
}

#[test]
fn a_chain_deeper_than_the_slab_cap_runs_to_completion() {
    // Deep enough to dwarf any slab, and trimmed under Miri, where every level is interpreted.
    const DEPTH: usize = if cfg!(miri) { 24 } else { 200 };

    let mut table: CellTable<Owned> = CellTable::new(2, pin);
    let root = table.create(None, None).unwrap();
    let filler = table.create(None, None).unwrap();
    assert_eq!(table.create(None, None), Err(CreateError::SlabFull));

    // The whole chain lives in the pool: not one level takes a slab slot.
    let mut chain = Vec::with_capacity(DEPTH);
    let mut parent = CellHandle::Slab(root);
    for _ in 0..DEPTH {
        let cell = table.create_tree(parent, None).unwrap();
        chain.push(cell);
        parent = CellHandle::Tree(cell);
    }
    assert_eq!(
        table.create(None, None),
        Err(CreateError::SlabFull),
        "the slab never grew"
    );

    // Innermost first: build a number, pin it into the parent, keep it, and die. Each level up
    // redeems what its child left, adds one, and does the same.
    let mut carried: Option<Dormant<Number>> = None;
    let mut spliced = 0;
    for level in (0..DEPTH).rev() {
        let cell = chain[level];
        let up = match level {
            0 => CellHandle::Slab(root),
            _ => CellHandle::Tree(chain[level - 1]),
        };
        let taken = carried.take();
        carried = Some(
            table
                .enter(cell, |context| {
                    let value = match taken {
                        None => number_in(context, 0),
                        Some(dormant) => {
                            let redeemed = context.redeem(dormant).unwrap();
                            let seen = *context.read(&redeemed).value();
                            number_in(context, seen + 1)
                        }
                    };
                    let placed = context
                        .alloc_into::<Number, Number>(
                            up,
                            &[kept_operand(&value)],
                            |writer, views| take(&views[0], writer),
                        )
                        .unwrap();
                    context.keep(placed)
                })
                .unwrap(),
        );
        table.release_tree(cell).unwrap();
        // Every level's bump ends up further up the chain, and the root's bundle only grows.
        let now = table.region_bytes(root).unwrap();
        assert!(now >= spliced);
        spliced = now;
    }

    let read = table
        .enter(root, |context| {
            *context
                .read(&context.redeem(carried.unwrap()).unwrap())
                .value()
        })
        .unwrap();
    assert_eq!(read as usize, DEPTH - 1);
    assert!(spliced > 0, "the levels' storage arrived in the root");

    table
        .release(filler, ReleaseAbsorption::IntoHolder)
        .unwrap();
    table.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(table.is_empty(), "the tombstones went with the root");
    assert_eq!(table.relocations(), 0);
}

#[test]
fn a_splice_keeps_a_borrow_the_destination_already_holds() {
    let mut table: CellTable<Borrowed> = CellTable::new(2, pin);
    let root = table.create(None, None).unwrap();
    let destination = table.create_tree(root, None).unwrap();
    let home = table.create_tree(destination, None).unwrap();

    let kept = table
        .enter(home, |context| {
            let value = number_in(context, 33);
            context.keep(value)
        })
        .unwrap();

    // The destination stores a continuation capturing a value that lives in its child's bump. The
    // pin pledges the child, so the bytes the continuation reads move into this very bundle when
    // the child dies rather than going away with it.
    table
        .enter(destination, |context| {
            let carrier = context.redeem(kept).unwrap();
            context.store_successor_capturing(&[kept_operand(&carrier)], |writer, views| {
                take(&views[0], writer)
            });
        })
        .unwrap();

    table.release_tree(home).unwrap();
    let read = table
        .enter(destination, |context| {
            *context
                .continuation()
                .expect("the successor is still in the slot")
                .value()
        })
        .unwrap();
    assert_eq!(read, 33);

    table.release_tree(destination).unwrap();
    table.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(table.is_empty());
}

#[test]
fn a_reinstall_inside_a_tree_copies_the_hop_and_reclaims_the_old_one() {
    let mut table: CellTable<Owned> = CellTable::new(2, pin);
    let root = table.create(None, None).unwrap();
    let hop = table.create_tree(root, None).unwrap();
    let next = table.create_tree(root, None).unwrap();

    // The next hop's arguments are built into a sibling, which is off this cell's chain — so the
    // ancestry rule forces the copy, and the old hop's storage is free to go.
    let kept = table
        .enter(hop, |context| {
            let value = number_in(context, 6);
            let placed = context
                .alloc_into::<Number, Number>(next, &[kept_operand(&value)], |writer, views| {
                    assert!(
                        matches!(views[0], CrossedOperand::Copied(_)),
                        "a sibling is off the chain"
                    );
                    take(&views[0], writer)
                })
                .unwrap();
            context.keep(placed)
        })
        .unwrap();

    table.release_tree(hop).unwrap();
    assert_eq!(
        table.trees().occupied().count(),
        1,
        "the old hop pledged nothing, so its bump went with it"
    );

    let read = table
        .enter(next, |context| {
            *context.read(&context.redeem(kept).unwrap()).value()
        })
        .unwrap();
    assert_eq!(read, 6);

    table.release_tree(next).unwrap();
    table.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(table.is_empty());
}
