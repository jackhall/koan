//! Slot-array tests: bind-once, the read half, and a slot bound in a root's region read through a
//! view a tree child holds across a step — the erased read Miri checks.

use super::{SlotArray, SlotConflict, SlotView};
use crate::memory::substrate::{
    CellGraph, CrossedOperand, DropFree, Operand, ReleaseAbsorption, Verdict, Writer, covariant,
    reattachable,
};
use crate::memory::tests::in_cell;

/// A stand-in payload family borrowing its region — `Copy` and drop-free, the tier a slot admits.
struct Number;
reattachable!(Number => &'cell u32);
covariant!(Number);
impl DropFree for Number {}

fn one(writer: Writer<'_>, value: u32) -> &u32 {
    &writer.fill(1, |_| value)[0]
}

#[test]
fn slots_start_empty_and_bind_once() {
    in_cell(|writer| {
        let slots: SlotArray<'static, '_, Number> = SlotArray::new(writer, 3);
        assert_eq!(slots.len(), 3);
        assert!(slots.get(0).is_none());
        slots.bind(1, one(writer, 42)).expect("an empty slot binds");
        assert_eq!(slots.bind(1, one(writer, 43)), Err(SlotConflict));
        assert_eq!(
            slots.get(1).copied(),
            Some(42),
            "a refused bind changes nothing"
        );
        assert_eq!(slots.view().get(1).copied(), Some(42));
        assert!(slots.view().get(2).is_none());
    });
}

/// The array is `Copy` and its slots live in the region, so a bind through one copy is seen through
/// every other — what lets a continuation capture the array by value.
#[test]
fn copies_share_slots() {
    in_cell(|writer| {
        let slots: SlotArray<'static, '_, Number> = SlotArray::new(writer, 1);
        let captured = slots;
        captured
            .bind(0, one(writer, 7))
            .expect("an empty slot binds");
        assert_eq!(slots.get(0).copied(), Some(7));
    });
}

/// A view as a value that crosses into a child: its covariance witness is the check that a slot
/// view is covariant in its brand.
struct Views;
reattachable!(Views => SlotView<'graph, 'cell, Number>);
covariant!(Views);
impl DropFree for Views {}

/// A continuation holding a view at the executing cell's brand.
struct Viewing;
reattachable!(Viewing => Option<SlotView<'graph, 'cell, Number>>);

#[test]
fn a_tree_child_reads_a_slot_bound_in_its_root_through_a_view() {
    let mut graph: CellGraph<'static, Viewing> = CellGraph::new(1, |_| Verdict::Pin);
    let root = graph.create(None).unwrap();
    let kept = graph
        .enter(root, |context| {
            let writer = context.writer();
            let slots: SlotArray<'static, '_, Number> = SlotArray::new(writer, 2);
            slots.bind(0, one(writer, 12)).unwrap();
            let carrier = context.lift::<Views>(slots.view());
            context.keep(carrier)
        })
        .unwrap();
    let child = graph.create_tree(root, None).unwrap();
    graph
        .enter(child, |context| {
            let carrier = context.redeem(kept).unwrap();
            let view = context.alloc_here(
                &[Operand {
                    carrier: &carrier,
                    copy_bytes: usize::MAX,
                }],
                |_, views| match views[0] {
                    CrossedOperand::Pinned { view, .. } => view,
                    CrossedOperand::Copied { .. } => unreachable!("the verdict always pins"),
                },
            );
            context.store_successor(Some(view));
        })
        .unwrap();
    let read = graph
        .enter(child, |context| {
            let view = context.continuation().flatten().expect("the view was kept");
            (view.get(0).copied(), view.get(1).copied())
        })
        .unwrap();
    assert_eq!(read, (Some(12), None));
    graph.release_tree(child).unwrap();
    graph.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}
