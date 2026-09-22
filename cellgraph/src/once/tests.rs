//! The once-written run: a slot set once and refused a second time, and a value set in a root's
//! region read through a view a tree child and a tenant of that root hold across a step — the
//! reattach a read makes at a brand shorter than the one the value was set at, which Miri checks.
//! [`Storage`] is exercised here too: a graph built over it borrows it as `'graph`.

use super::*;
use crate::{
    CellGraph, CellHandle, CrossedOperand, Dormant, DropFree, Operand, Prices, ReleaseAbsorption,
    SlabHandle, Storage, Verdict, covariant, reattachable,
};

/// A value family borrowing its region.
struct Number;
reattachable!(Number => &'cell u32);
covariant!(Number);
impl DropFree for Number {}

/// A view over a run of numbers, as a value that crosses into a child — so it takes the covariance
/// witness, which is the check that a view is covariant in its brand.
struct Views;
reattachable!(Views => OnceView<'graph, 'cell, Number>);
covariant!(Views);
impl DropFree for Views {}

/// A continuation holding a view at the executing cell's brand.
struct Viewing;
reattachable!(Viewing => Option<OnceView<'graph, 'cell, Number>>);

fn pin(_: Prices) -> Verdict {
    Verdict::Pin
}

fn one(writer: Writer<'_>, value: u32) -> &u32 {
    &writer.fill(1, |_| value)[0]
}

#[test]
fn a_slot_is_set_once_and_reads_back_through_the_view() {
    let storage = Storage::new();
    let writer = storage.writer();
    let run = writer.once_run::<Number>(3);
    assert_eq!(run.len(), 3);
    assert!(run.view().get(0).is_none());
    run.set(0, one(writer, 7)).unwrap();
    assert_eq!(run.set(0, one(writer, 8)), Err(Written));
    assert_eq!(run.view().get(0).copied(), Some(7));
    assert!(run.view().get(1).is_none());
    assert!(writer.once_run::<Number>(0).is_empty());
}

/// Lay a run down in `root`, set its first slot, and keep its view at rest in the root.
fn a_run_in(graph: &mut CellGraph<'static, Viewing>, root: SlabHandle) -> Dormant<'static, Views> {
    graph
        .enter(root, |context| {
            let writer = context.writer();
            let run = writer.once_run::<Number>(2);
            run.set(0, one(writer, 12)).unwrap();
            let carrier = context.lift::<Views>(run.view());
            context.keep(carrier)
        })
        .unwrap()
}

/// Redeem the view in `cell`, keep it in the continuation, and read it back a step later.
fn read_across_a_step(
    graph: &mut CellGraph<'static, Viewing>,
    cell: impl Into<CellHandle> + Copy,
    kept: Dormant<'static, Views>,
) -> (Option<u32>, Option<u32>) {
    graph
        .enter(cell, |context| {
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
    graph
        .enter(cell, |context| {
            let view = context
                .continuation()
                .flatten()
                .expect("the successor holds the view");
            (view.get(0).copied(), view.get(1).copied())
        })
        .unwrap()
}

#[test]
fn a_tree_child_reads_a_run_set_in_its_root_across_a_step() {
    let mut graph: CellGraph<'static, Viewing> = CellGraph::new(1, pin);
    let root = graph.create(None).unwrap();
    let kept = a_run_in(&mut graph, root);
    let child = graph.create_tree(root, None).unwrap();
    assert_eq!(
        read_across_a_step(&mut graph, child, kept),
        (Some(12), None)
    );
    graph.release_tree(child).unwrap();
    graph.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

#[test]
fn a_tenant_reads_a_run_set_in_its_host_across_a_step() {
    let mut graph: CellGraph<'static, Viewing> = CellGraph::new(1, pin);
    let root = graph.create(None).unwrap();
    let kept = a_run_in(&mut graph, root);
    let tenant = graph.create_tenant(root, None).unwrap();
    assert_eq!(
        read_across_a_step(&mut graph, tenant, kept),
        (Some(12), None)
    );
    graph.release_tenant(tenant).unwrap();
    graph.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

/// A continuation borrowing storage outside the graph.
struct Program<'graph>(std::marker::PhantomData<&'graph u32>);
reattachable!(Program<'graph> => &'graph u32);

#[test]
fn storage_outlives_the_graph_built_over_it() {
    let storage = Storage::new();
    let table = storage.writer().fill(2, |index| index as u32 + 40);
    {
        let mut graph: CellGraph<'_, Program<'_>> = CellGraph::new(1, pin);
        let cell = graph.create(Some(&table[1])).unwrap();
        graph
            .enter(cell, |context| {
                let program = context.continuation().expect("born holding the table");
                context.store_successor(program);
            })
            .unwrap();
        let read = graph
            .enter(cell, |context| *context.continuation().unwrap())
            .unwrap();
        assert_eq!(read, 41);
    }
    assert_eq!(table, [40, 41]);
}
