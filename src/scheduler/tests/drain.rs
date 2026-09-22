//! The drain's loop over native steps: a root work runs to its end over the ready stack, depth
//! first, with the cells live at any moment one path from the root.

use crate::knot::KValue;
use crate::memory::Active;
use crate::scheduler::tests::bundle::{Native, TestGraph};
use crate::scheduler::tests::native::{
    describe, fresh, record, recorded, reset, shares, where_text, work,
};
use crate::scheduler::{
    Action, DrainStalled, Placement, Received, Scheduler, Step, StepError, Use,
};

/// A step that records the number or text it holds, and finishes.
fn record_state<'graph>(step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let (step, state) = step.state();
    record(describe(state));
    step.done()
}

/// A step that refuses to proceed.
fn fail<'graph>(step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    step.failed(StepError::Stale)
}

#[test]
fn one_root_work_runs_dies_and_leaves_the_root_live() {
    reset();
    let mut graph: TestGraph<'static> = TestGraph::new(2);
    let root = graph.root().expect("a fresh slab admits a root");
    let mut scheduler = Scheduler::over(&mut graph);
    scheduler
        .run(
            work(record_state, KValue::Number(1.0)),
            root,
            Placement::Fresh,
        )
        .expect("the root work ends");
    assert_eq!(recorded(), ["1"]);
    assert_eq!(scheduler.peak_live_cells(), 1);
    assert!(
        !scheduler.graph().is_empty(),
        "the root is a cell of the graph"
    );
    assert!(scheduler.graph().is_live(root), "and no drain releases it");
    graph.release_root(root).expect("the root releases");
    assert!(graph.is_empty());
}

#[test]
fn a_birth_state_reaches_the_step_that_runs_the_root_work() {
    reset();
    let mut graph: TestGraph<'static> = TestGraph::new(2);
    let root = graph.root().expect("a fresh slab admits a root");
    Scheduler::over(&mut graph)
        .run(
            work(record_state, KValue::Str("born")),
            root,
            Placement::Shares,
        )
        .expect("the root work ends");
    assert_eq!(recorded(), ["born"]);
}

/// Hand a child a state built in this cell's own region, at `'here`.
fn hand_over<'graph>(
    mut step: Step<'_, 'graph, '_, '_, '_, Native>,
    placement: Placement,
) -> Action<'graph, Native> {
    let handed = crate::values::text(step.writer(), "handed over");
    record(format!("handed {}", where_text(handed)));
    let request = match placement {
        Placement::Fresh => fresh(take_over, Use::Reads, handed),
        Placement::Shares => shares(take_over, Use::Reads, handed),
    };
    let asked = step.spawn(request);
    step.park(asked, woken, KValue::Null, None)
}

fn hand_over_fresh<'graph>(step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    hand_over(step, Placement::Fresh)
}

fn hand_over_shares<'graph>(step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    hand_over(step, Placement::Shares)
}

/// The child: record the state it woke holding, and deliver nothing but the wake.
fn take_over<'graph>(step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let (step, state) = step.state();
    record(format!("woke {}", where_text(state)));
    step.finish_fresh(|_, _| Active::new(KValue::Null))
}

/// A consumer woken by its children, with nothing more to do.
fn woken<'graph>(step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    step.done()
}

/// The spawner is above the child and a tenant crosses nothing, so either way the child wakes over
/// the very bytes its spawner built.
#[test]
fn a_spawned_child_wakes_holding_the_state_its_spawner_handed_over() {
    for spawner in [hand_over_fresh as _, hand_over_shares as _] {
        reset();
        let mut graph: TestGraph<'static> = TestGraph::new(2);
        let root = graph.root().expect("a fresh slab admits a root");
        Scheduler::over(&mut graph)
            .run(work(spawner, KValue::Null), root, Placement::Fresh)
            .expect("the root work ends");
        let seen = recorded();
        assert_eq!(seen.len(), 2, "{seen:?}");
        assert!(seen[0].starts_with("handed handed over@"), "{seen:?}");
        assert_eq!(
            seen[0].strip_prefix("handed "),
            seen[1].strip_prefix("woke "),
            "the child woke over its spawner's bytes"
        );
    }
}

// ---- Depth first. ----

/// A cell of the order test: record its name, ask for two children named after it when it is not
/// a leaf, and park on them.
fn named<'graph>(step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let (mut step, state) = step.state();
    let KValue::Str(name) = state else {
        return step.failed(StepError::Stale);
    };
    record(name.to_owned());
    if name.len() == 2 {
        return step.finish_fresh(|_, _| Active::new(KValue::Null));
    }
    let children: [&str; 2] = if name == "root" {
        ["A", "B"]
    } else {
        ["1", "2"]
    };
    let mut last = None;
    for child in children {
        let label = if name == "root" {
            child.to_owned()
        } else {
            format!("{name}{child}")
        };
        let text = crate::values::text(step.writer(), &label);
        last = Some(step.spawn(fresh(named, Use::Reads, text)));
    }
    let asked = last.expect("two children asked");
    let text = crate::values::text(step.writer(), name);
    step.park(asked, named_woken, text, None)
}

/// A cell of the order test, woken: record that it woke, and deliver to its own spawner.
fn named_woken<'graph>(step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let (step, state) = step.state();
    let KValue::Str(name) = state else {
        return step.failed(StepError::Stale);
    };
    record(format!("{name} woken"));
    if name == "root" {
        return step.done();
    }
    step.finish_fresh(|_, _| Active::new(KValue::Null))
}

#[test]
fn two_children_asked_in_one_park_run_one_subtree_after_the_other() {
    reset();
    let mut graph: TestGraph<'static> = TestGraph::new(2);
    let root = graph.root().expect("a fresh slab admits a root");
    Scheduler::over(&mut graph)
        .run(work(named, KValue::Str("root")), root, Placement::Fresh)
        .expect("the root work ends");
    assert_eq!(
        recorded(),
        [
            "root",
            "A",
            "A1",
            "A2",
            "A woken",
            "B",
            "B1",
            "B2",
            "B woken",
            "root woken"
        ],
        "the child asked first runs to completion before the second is born"
    );
}

/// How deep the recursion goes below its root work.
const DEPTH: f64 = 8.0;

/// One level of a recursion that asks for two children at every level, down to a leaf.
fn node<'graph>(step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let (mut step, state) = step.state();
    let KValue::Number(depth) = state else {
        return step.failed(StepError::Stale);
    };
    if depth == 0.0 {
        return step.finish_fresh(|_, _| Active::new(KValue::Number(1.0)));
    }
    step.spawn(fresh(node, Use::Reads, KValue::Number(depth - 1.0)));
    let asked = step.spawn(fresh(node, Use::Reads, KValue::Number(depth - 1.0)));
    step.park(asked, node_woken, KValue::Number(depth), None)
}

/// One level, woken: count the leaves below it and hand the count up, or record it at the top.
fn node_woken<'graph>(step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let (mut step, state) = step.state();
    let KValue::Number(depth) = state else {
        return step.failed(StepError::Stale);
    };
    let mut leaves = 0.0;
    for received in step.results().collect::<Vec<_>>() {
        let Ok(Received::Scratch(KValue::Number(count))) = received else {
            return step.failed(StepError::Unredeemable);
        };
        leaves += count;
    }
    if depth == DEPTH {
        record(leaves.to_string());
        return step.done();
    }
    step.finish_fresh(move |_, _| Active::new(KValue::Number(leaves)))
}

#[test]
fn a_recursion_asking_two_children_per_level_peaks_at_its_depth() {
    reset();
    let mut graph: TestGraph<'static> = TestGraph::new(1);
    let root = graph.root().expect("a fresh slab admits a root");
    let mut scheduler = Scheduler::over(&mut graph);
    scheduler
        .run(work(node, KValue::Number(DEPTH)), root, Placement::Fresh)
        .expect("the root work ends");
    assert_eq!(recorded(), ["256"], "every leaf answered");
    // The running leaf and its parked ancestors, up to and including the root work: a sibling
    // whose turn has not come has no cell.
    assert_eq!(scheduler.peak_live_cells(), DEPTH as usize + 1);
}

/// How many siblings one park asks for.
const SIBLINGS: usize = 100;

/// Ask for a hundred children in one park.
fn ask_a_hundred<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let mut last = None;
    for index in 0..SIBLINGS {
        last = Some(step.spawn(fresh(sibling, Use::Reads, KValue::Number(index as f64))));
    }
    step.park(
        last.expect("a hundred asked"),
        count_them,
        KValue::Null,
        None,
    )
}

/// One sibling: record its index, and deliver it.
fn sibling<'graph>(step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let (step, state) = step.state();
    let KValue::Number(index) = state else {
        return step.failed(StepError::Stale);
    };
    record(index.to_string());
    step.finish_fresh(move |_, _| Active::new(KValue::Number(index)))
}

/// The spawner, woken: count what arrived.
fn count_them<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let arrived = step.results().filter(Result::is_ok).count();
    record(format!("{arrived} arrived"));
    step.done()
}

#[test]
fn a_hundred_siblings_asked_in_one_park_run_one_at_a_time() {
    reset();
    let mut graph: TestGraph<'static> = TestGraph::new(1);
    let root = graph.root().expect("a fresh slab admits a root");
    let mut scheduler = Scheduler::over(&mut graph);
    scheduler
        .run(work(ask_a_hundred, KValue::Null), root, Placement::Fresh)
        .expect("the root work ends");
    let mut order: Vec<String> = (0..SIBLINGS).map(|index| index.to_string()).collect();
    order.push(format!("{SIBLINGS} arrived"));
    assert_eq!(recorded(), order, "in the order they were asked");
    assert_eq!(
        scheduler.peak_live_cells(),
        2,
        "the spawner and one sibling"
    );
}

// ---- What the drain refuses. ----

#[test]
fn a_step_that_cannot_proceed_stalls_the_drain() {
    reset();
    let mut graph: TestGraph<'static> = TestGraph::new(2);
    let root = graph.root().expect("a fresh slab admits a root");
    let mut scheduler = Scheduler::over(&mut graph);
    assert_eq!(
        scheduler
            .run(work(fail, KValue::Null), root, Placement::Fresh)
            .err(),
        Some(DrainStalled::Step(StepError::Stale))
    );
}

/// Ask for a child that ends without delivering, and park on it.
fn park_on_a_silent_child<'graph>(
    mut step: Step<'_, 'graph, '_, '_, '_, Native>,
) -> Action<'graph, Native> {
    let asked = step.spawn(fresh(record_state, Use::Reads, KValue::Str("silent")));
    step.park(asked, woken, KValue::Null, None)
}

#[test]
fn a_child_that_ends_without_delivering_leaves_the_drain_unfinished() {
    reset();
    let mut graph: TestGraph<'static> = TestGraph::new(2);
    let root = graph.root().expect("a fresh slab admits a root");
    let mut scheduler = Scheduler::over(&mut graph);
    assert_eq!(
        scheduler
            .run(
                work(park_on_a_silent_child, KValue::Null),
                root,
                Placement::Fresh
            )
            .err(),
        Some(DrainStalled::Unfinished),
        "the spawner is parked on a run nothing will fill"
    );
    assert_eq!(recorded(), ["silent"]);
}

/// Ask for a child, then end without parking on it.
fn ask_and_leave<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    step.spawn(fresh(record_state, Use::Reads, KValue::Null));
    step.done()
}

#[test]
fn a_step_that_asks_and_does_not_park_is_refused() {
    reset();
    let mut graph: TestGraph<'static> = TestGraph::new(2);
    let root = graph.root().expect("a fresh slab admits a root");
    let mut scheduler = Scheduler::over(&mut graph);
    assert_eq!(
        scheduler
            .run(work(ask_and_leave, KValue::Null), root, Placement::Fresh)
            .err(),
        Some(DrainStalled::Step(StepError::Unparked))
    );
    assert!(recorded().is_empty(), "the child was never born");
}

// ---- Views. ----

#[test]
fn two_schedulers_run_beside_each_other_sharing_nothing() {
    reset();
    let mut first_graph: TestGraph<'static> = TestGraph::new(1);
    let first_root = first_graph.root().expect("a fresh slab admits a root");
    let mut first = Scheduler::over(&mut first_graph);
    let mut second_graph: TestGraph<'static> = TestGraph::new(1);
    let second_root = second_graph.root().expect("a fresh slab admits a root");
    let mut second = Scheduler::over(&mut second_graph);

    // Each drain runs with the other alive beside it, and neither touches the other's graph.
    first
        .run(
            work(record_state, KValue::Number(1.0)),
            first_root,
            Placement::Fresh,
        )
        .expect("the first drain ends");
    second
        .run(
            work(record_state, KValue::Number(2.0)),
            second_root,
            Placement::Fresh,
        )
        .expect("the second drain ends");
    first
        .run(
            work(record_state, KValue::Number(3.0)),
            first_root,
            Placement::Shares,
        )
        .expect("the first drain ends again");
    assert_eq!(
        recorded(),
        ["1", "2", "3"],
        "each cell ran in its own graph's turn"
    );
    assert!(first.graph().is_live(first_root));
    assert!(second.graph().is_live(second_root));
}

#[test]
fn two_views_over_one_graph_run_in_turn() {
    reset();
    let mut graph: TestGraph<'static> = TestGraph::new(1);
    let root = graph.root().expect("a fresh slab admits a root");
    Scheduler::over(&mut graph)
        .run(
            work(record_state, KValue::Number(1.0)),
            root,
            Placement::Fresh,
        )
        .expect("the first view's root work ends");
    // A second view over the same graph finds the root the first left, and runs a second root work
    // under it.
    Scheduler::over(&mut graph)
        .run(
            work(record_state, KValue::Number(2.0)),
            root,
            Placement::Fresh,
        )
        .expect("the second view's root work ends");
    assert_eq!(recorded(), ["1", "2"]);
    graph.release_root(root).expect("the root releases");
    assert!(graph.is_empty());
}

/// Park on two children, the first of which refuses to proceed: the drain stalls on it with the
/// second still unborn on the stack.
fn park_behind_a_failure<'graph>(
    mut step: Step<'_, 'graph, '_, '_, '_, Native>,
) -> Action<'graph, Native> {
    step.spawn(fresh(fail, Use::Reads, KValue::Null));
    let asked = step.spawn(fresh(record_state, Use::Reads, KValue::Str("queued")));
    step.park(asked, woken, KValue::Null, None)
}

#[test]
fn a_view_dropped_with_a_queued_cell_leaves_cells_the_roots_release_does_not_empty() {
    reset();
    let mut graph: TestGraph<'static> = TestGraph::new(2);
    let root = graph.root().expect("a fresh slab admits a root");
    assert_eq!(
        Scheduler::over(&mut graph)
            .run(
                work(park_behind_a_failure, KValue::Null),
                root,
                Placement::Fresh
            )
            .err(),
        Some(DrainStalled::Step(StepError::Stale))
    );
    assert!(
        recorded().is_empty(),
        "the queued sibling went with the view that dropped it"
    );
    let _ = graph.release_root(root);
    assert!(
        !graph.is_empty(),
        "the parked root work and the failed child are the graph's still"
    );
}

/// A root work that writes text in its home and leaves it at rest.
fn write_and_leave<'graph>(step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let left = crate::values::text(step.writer(), "left at rest");
    record(format!("left {}", where_text(left)));
    step.leave(left)
}

/// A later root work that records what the earlier one left.
fn read_what_was_left<'graph>(
    step: Step<'_, 'graph, '_, '_, '_, Native>,
) -> Action<'graph, Native> {
    let (step, state) = step.state();
    record(format!("left {}", where_text(state)));
    step.done()
}

/// Leave text at rest from a root work at `placement`, then resume a second root work from it,
/// through a second view over the same graph.
fn leave_then_resume(placement: Placement) -> Vec<String> {
    reset();
    let mut graph: TestGraph<'static> = TestGraph::new(2);
    let root = graph.root().expect("a fresh slab admits a root");
    let resting = Scheduler::over(&mut graph)
        .run(work(write_and_leave, KValue::Null), root, placement)
        .expect("the root work ends")
        .expect("it left a birth at rest");
    let after = Scheduler::over(&mut graph)
        .resume(read_what_was_left, resting, root, Placement::Shares)
        .expect("the resumed root work ends");
    assert!(after.is_none(), "the second root work left nothing");
    assert!(graph.is_live(root));
    graph.release_root(root).expect("the root releases");
    assert!(graph.is_empty());
    recorded()
}

#[test]
fn a_root_work_leaves_a_state_a_later_root_work_resumes_from() {
    let seen = leave_then_resume(Placement::Shares);
    assert_eq!(seen.len(), 2);
    assert_eq!(
        seen[0], seen[1],
        "a tenant of the root reads it where it lies"
    );
}

#[test]
fn a_fresh_root_work_leaves_a_state_its_root_keeps() {
    let seen = leave_then_resume(Placement::Fresh);
    assert_eq!(seen.len(), 2);
    // The birth crossed into the root at the verdict's price: short text copies, so the second
    // root work reads the root's copy of it.
    assert!(seen[1].starts_with("left left at rest@"));
}

/// A child that tries to leave: only a root work reports to nobody.
fn leave_from_a_child<'graph>(
    step: Step<'_, 'graph, '_, '_, '_, Native>,
) -> Action<'graph, Native> {
    step.leave(KValue::Null)
}

/// A root work whose child tries to leave.
fn spawn_a_leaver<'graph>(
    mut step: Step<'_, 'graph, '_, '_, '_, Native>,
) -> Action<'graph, Native> {
    let asked = step.spawn(fresh(leave_from_a_child, Use::Reads, KValue::Null));
    step.park(asked, record_state, KValue::Null, None)
}

#[test]
fn a_cell_that_reports_to_somebody_cannot_leave() {
    reset();
    let mut graph: TestGraph<'static> = TestGraph::new(2);
    let root = graph.root().expect("a fresh slab admits a root");
    assert_eq!(
        Scheduler::over(&mut graph)
            .run(work(spawn_a_leaver, KValue::Null), root, Placement::Fresh)
            .err(),
        Some(DrainStalled::Step(StepError::Undeliverable))
    );
}
