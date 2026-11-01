//! Faults ([README § Faults](../README.md#faults)): a tree cell that ends with one is kept, its
//! region header holding the link, and its consumer is handed a token naming it; a tenant is
//! released as ever. The requests a park asked for after a faulting child never run, kept cells
//! read back through `read_kept` at a brand of the call's and release in any order, a kept hop
//! keeps the loop cell over it, and a kept cell read beside a step is released once that step ends.

use std::cell::Cell;

use crate::knot::{KLink, KValue};
use crate::memory::{Bump, CellHandle, resident};
use crate::scheduler::tests::bundle::{Native, TestGraph};
use crate::scheduler::tests::native::{fresh, record, recorded, reset, work};
use crate::scheduler::{Action, Received, Scheduler, Step, StepError, Use, Work};
use crate::scope::{Fault, Inner, Line};
use crate::type_lattice::{KType, TypeNode};
use crate::values::{KeptCell, list_type};

thread_local! {
    /// The token the consumer was handed for the cell its child kept.
    static KEPT: Cell<Option<KeptCell>> = const { Cell::new(None) };
    /// The type the faulting step interned in its own region and named in its fault.
    static NAMED: Cell<Option<KType>> = const { Cell::new(None) };
}

/// A graph over a seeded root, with room for one root.
fn graph(types: &Bump) -> TestGraph<'_> {
    TestGraph::new(
        1,
        crate::values::tests::seeded_tiers(types),
        Default::default(),
    )
}

/// The trace line every faulting test step owes: an `EVAL`'s, at an extent no file holds.
fn line<'cell>() -> Option<Line<'cell>> {
    Some(Line {
        callee: None,
        call: crate::tests::nowhere(),
        hops: &[],
    })
}

/// End with a fault naming a list type interned in this cell's own region.
fn raise<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let named = list_type(&step.types(), &Bump::new(), [KType::NUMBER].into_iter());
    NAMED.set(Some(named));
    let fault = resident(step.writer(), Fault::NotASignature { ascribed: named });
    step.fault(line(), Inner::Fault(fault))
}

/// Record that it ran, and deliver nothing of note.
fn bystander<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    record(String::from("the second child ran"));
    step.finish_fresh(|_, _, _| crate::memory::Active::new(KValue::Null))
}

/// Ask for one child that faults, and park on it.
fn ask_for_one<'graph>(
    mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>,
) -> Action<'graph, Native> {
    let asked = step.spawn(fresh(raise, Use::Keeps, KValue::Null));
    step.park(asked, keep_token, KValue::Null, None)
}

/// Ask for a child that faults and one after it, and park on both.
fn ask_for_two<'graph>(
    mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>,
) -> Action<'graph, Native> {
    step.spawn(fresh(raise, Use::Keeps, KValue::Null));
    let asked = step.spawn(fresh(bystander, Use::Keeps, KValue::Null));
    step.park(asked, keep_token, KValue::Null, None)
}

/// Ask for a child that asks for the faulting one and passes its fault on.
fn ask_for_chain<'graph>(
    mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>,
) -> Action<'graph, Native> {
    let asked = step.spawn(fresh(ask_and_pass, Use::Keeps, KValue::Null));
    step.park(asked, keep_token, KValue::Null, None)
}

/// Ask for the faulting child, and park to pass its fault on.
fn ask_and_pass<'graph>(
    mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>,
) -> Action<'graph, Native> {
    let asked = step.spawn(fresh(raise, Use::Keeps, KValue::Null));
    step.park(asked, pass_on, KValue::Null, None)
}

/// End with the fault the child ended with, owing no line.
fn pass_on<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let Some(Ok(Received::Fault(inner))) = step.results().next() else {
        return step.failed(StepError::Unredeemable);
    };
    step.fault(None, inner)
}

/// Ask for a child that hops, threading, into a step that faults.
fn ask_for_hop<'graph>(
    mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>,
) -> Action<'graph, Native> {
    let asked = step.spawn(fresh(hop, Use::Keeps, KValue::Bool(true)));
    step.park(asked, keep_token, KValue::Null, None)
}

/// Hop into the faulting step, the birth threading, so the hop is born under a loop cell.
fn hop<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    step.tail(Work {
        step: raise,
        state: KValue::Bool(true),
    })
}

/// Read every result: exactly one, the fault, whose token the test reads back.
fn keep_token<'graph>(
    mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>,
) -> Action<'graph, Native> {
    let results: Vec<_> = step.results().collect();
    let [Ok(Received::Fault(Inner::Below { kept, .. }))] = results[..] else {
        return step.failed(StepError::Unredeemable);
    };
    KEPT.set(Some(kept));
    step.done()
}

/// The kept cell's handle, to watch its liveness.
fn handle(kept: KeptCell) -> CellHandle {
    CellHandle::Tree(kept.opened().0)
}

/// What a kept link holds: whether it owes a line, and the kept cell below or the fault inside.
fn inside(link: &KLink<'_, '_>) -> (bool, Option<KeptCell>) {
    let below = match link.inner {
        Inner::Below { kept, .. } => Some(kept),
        _ => None,
    };
    (link.line.is_some(), below)
}

#[test]
fn a_tree_cell_that_faults_stays_live_and_its_link_reads_back() {
    let types = Bump::new();
    let mut graph = graph(&types);
    let root = graph.root().expect("a fresh slab admits a root");
    Scheduler::over(&mut graph)
        .run(
            work(ask_for_one, KValue::Null),
            root,
            crate::scheduler::Placement::Fresh,
        )
        .expect("the root work ends");
    let kept = KEPT.take().expect("the consumer was handed the kept cell");
    assert!(
        graph.is_live(handle(kept)),
        "the faulting cell outlives its consumer"
    );
    let named = NAMED.take().expect("the fault named a type");
    let read = graph.read_kept(kept, |link, types| {
        let Inner::Fault(Fault::NotASignature { ascribed }) = link.inner else {
            return None;
        };
        // The type the faulting cell interned in its own region resolves in the reader's chain.
        let resolves = matches!(types.node(*ascribed), TypeNode::List { .. });
        Some((link.line.is_some(), *ascribed == named && resolves))
    });
    assert_eq!(read, Some(Some((true, true))));
    graph.release_kept(kept).expect("a kept cell releases");
    graph.release_root(root).expect("the root releases");
    assert!(graph.is_empty());
}

#[test]
fn a_fault_in_a_parks_first_child_leaves_the_second_unborn() {
    reset();
    let types = Bump::new();
    let mut graph = graph(&types);
    let root = graph.root().expect("a fresh slab admits a root");
    Scheduler::over(&mut graph)
        .run(
            work(ask_for_two, KValue::Null),
            root,
            crate::scheduler::Placement::Fresh,
        )
        .expect("the root work ends");
    assert!(recorded().is_empty(), "the second child never ran");
    let kept = KEPT.take().expect("the consumer woke on the fault");
    graph.release_kept(kept).expect("a kept cell releases");
    graph.release_root(root).expect("the root releases");
    assert!(graph.is_empty());
}

/// Run the chain — a consumer, a cell passing its child's fault on, the faulting child — and hand
/// back the two kept cells, outermost first.
fn kept_chain(graph: &mut TestGraph<'_>, root: crate::memory::SlabHandle) -> (KeptCell, KeptCell) {
    Scheduler::over(graph)
        .run(
            work(ask_for_chain, KValue::Null),
            root,
            crate::scheduler::Placement::Fresh,
        )
        .expect("the root work ends");
    let outer = KEPT
        .take()
        .expect("the consumer was handed the passing cell");
    let (owes, below) = graph
        .read_kept(outer, |link, _| inside(link))
        .expect("the passing cell holds a link");
    assert!(!owes, "a cell passing a fault on owes no line here");
    let inner = below.expect("its link names the kept cell below");
    let (owes, below) = graph
        .read_kept(inner, |link, _| inside(link))
        .expect("the faulting cell holds a link");
    assert!(
        owes && below.is_none(),
        "the innermost link holds the fault"
    );
    (outer, inner)
}

#[test]
fn kept_cells_release_parent_first() {
    let types = Bump::new();
    let mut graph = graph(&types);
    let root = graph.root().expect("a fresh slab admits a root");
    let (outer, inner) = kept_chain(&mut graph, root);
    graph
        .release_kept(outer)
        .expect("the parent releases first");
    assert!(
        graph.is_live(handle(inner)),
        "the child outlives its parent's release"
    );
    graph.release_kept(inner).expect("then the child");
    graph.release_root(root).expect("the root releases");
    assert!(graph.is_empty());
}

#[test]
fn kept_cells_release_child_first() {
    let types = Bump::new();
    let mut graph = graph(&types);
    let root = graph.root().expect("a fresh slab admits a root");
    let (outer, inner) = kept_chain(&mut graph, root);
    graph.release_kept(inner).expect("the child releases first");
    graph.release_kept(outer).expect("then the parent");
    graph.release_root(root).expect("the root releases");
    assert!(graph.is_empty());
}

#[test]
fn a_kept_hop_keeps_its_loop_cell_until_it_is_released() {
    let types = Bump::new();
    let mut graph = graph(&types);
    let root = graph.root().expect("a fresh slab admits a root");
    let mut scheduler = Scheduler::over(&mut graph);
    scheduler
        .run(
            work(ask_for_hop, KValue::Null),
            root,
            crate::scheduler::Placement::Fresh,
        )
        .expect("the root work ends");
    // The root work, the hop's predecessor, the loop cell and the hop.
    assert_eq!(
        scheduler.peak_live_cells(),
        4,
        "the hop ran under a loop cell"
    );
    drop(scheduler);
    let kept = KEPT.take().expect("the consumer was handed the kept hop");
    graph.release_root(root).expect("the root releases");
    assert!(!graph.is_empty(), "the kept hop holds its loop cell");
    graph.release_kept(kept).expect("the hop releases");
    assert!(graph.is_empty(), "and the loop cell goes with it");
}

thread_local! {
    /// Whether the step entered beside the kept cell read its fault, and its named type resolved
    /// in the step's own chain once adopted.
    static READ_BESIDE: Cell<Option<bool>> = const { Cell::new(None) };
}

/// Ask for the faulting child, and park to read its kept cell beside a step of its own.
fn ask_to_read_beside<'graph>(
    mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>,
) -> Action<'graph, Native> {
    let asked = step.spawn(fresh(raise, Use::Keeps, KValue::Null));
    step.park(asked, enter_beside, KValue::Null, None)
}

/// End by asking to be entered again beside the kept cell the child's fault names.
fn enter_beside<'graph>(
    mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>,
) -> Action<'graph, Native> {
    let Some(Ok(Received::Fault(Inner::Below { kept, .. }))) = step.results().next() else {
        return step.failed(StepError::Unredeemable);
    };
    KEPT.set(Some(kept));
    step.read_beside(kept, read_beside, KValue::Null, None)
}

/// Read the kept cell's fault beside this step, adopting the type it names into this step's chain.
fn read_beside<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let types = step.types();
    let read = step.beside(|beside| {
        let Inner::Fault(Fault::NotASignature { ascribed }) = beside.link().inner else {
            return false;
        };
        beside.adopt_into(&types, *ascribed);
        matches!(types.node(*ascribed), TypeNode::List { .. })
    });
    READ_BESIDE.set(read);
    step.done()
}

#[test]
fn a_kept_cell_read_beside_is_released_after_the_step() {
    let types = Bump::new();
    let mut graph = graph(&types);
    let root = graph.root().expect("a fresh slab admits a root");
    Scheduler::over(&mut graph)
        .run(
            work(ask_to_read_beside, KValue::Null),
            root,
            crate::scheduler::Placement::Fresh,
        )
        .expect("the root work ends");
    assert_eq!(
        READ_BESIDE.take(),
        Some(true),
        "the step read the fault beside it and adopted what it names"
    );
    let kept = KEPT.take().expect("the consumer was handed the kept cell");
    assert!(
        !graph.is_live(handle(kept)),
        "the drain released the kept cell after the step"
    );
    graph.release_root(root).expect("the root releases");
    assert!(graph.is_empty());
}
