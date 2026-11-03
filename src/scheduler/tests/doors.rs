//! The doors a step evaluating in place places a value through: `built_as` builds one and
//! `crossed_as` crosses one where a child the step asked with a given `Use` would have delivered
//! it — the cell, its home or its loop cell — and each hands it back kept there, ending nothing.

use std::cell::Cell;

use crate::knot::KValue;
use crate::memory::{Active, Bump};
use crate::scheduler::tests::bundle::{AnyStep, Native, TestGraph};
use crate::scheduler::tests::native::{fresh, record, recorded, reset, where_text, work};
use crate::scheduler::{
    Action, Held, PASSED, Placement, Received, Scheduler, Step, StepError, Use, Work,
};

/// What each door places: a value with bytes in a region, so a copy shows as a new address.
const TEXT: &str = "a value with bytes in it, placed by a door";

thread_local! {
    /// The child the root work asks for, and the `Use` it asks with: a native step is a bare `fn`.
    static CHILD: Cell<Option<(AnyStep, Use)>> = const { Cell::new(None) };
}

/// The root work: ask for the child, park on it.
fn root<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let (child, use_) = CHILD.with(Cell::get).expect("a test names its child");
    let asked = step.spawn(fresh(child, use_, KValue::Null));
    step.park(asked, received, KValue::Null, None)
}

/// The root work woken: record where the child's value lies.
fn received<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let first = step.results().next();
    match first {
        Some(Ok(Received::Here(held))) => record(where_text(held.value)),
        _ => return step.failed(StepError::Unredeemable),
    }
    step.done()
}

/// Run `child`, asked with `use_`, under a root work, and hand back what it recorded.
fn ran(child: AnyStep, use_: Use) -> Vec<String> {
    reset();
    CHILD.with(|at| at.set(Some((child, use_))));
    let type_region = Bump::new();
    let mut graph: TestGraph<'_> = TestGraph::new(
        1,
        crate::values::tests::seeded_tiers(&type_region),
        Default::default(),
    );
    let root_cell = graph.root().expect("a fresh slab admits a root");
    Scheduler::over(&mut graph)
        .run(work(root, KValue::Null), root_cell, Placement::Fresh)
        .expect("the root work ends");
    graph.release_root(root_cell).expect("the root releases");
    assert!(graph.is_empty(), "the doors leave no cell behind");
    recorded()
}

/// Whether `value` is the text a door placed, at `address`.
fn at(value: KValue<'_, '_>, address: &str) -> bool {
    where_text(value) == address
}

fn built_kept<'graph>(
    mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>,
) -> Action<'graph, Native> {
    let before = step.writer().in_use();
    let Some(held) = step.built_as(Use::Keeps, &[], None, |writer, _, _, _| {
        Active::new(crate::values::text(writer, TEXT))
    }) else {
        return step.failed(StepError::Stale);
    };
    let grew = step.writer().in_use() - before >= TEXT.len();
    record(format!("kept {} here {grew}", held.kept.is_some()));
    step.finish(held)
}

#[test]
fn a_value_built_as_kept_lands_in_the_cell() {
    let seen = ran(built_kept, Use::Keeps);
    assert_eq!(seen[0], "kept true here true");
}

fn built_forwarded<'graph>(
    mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>,
) -> Action<'graph, Native> {
    let before = step.writer().in_use();
    let Some(held) = step.built_as(Use::Forwards, &[], None, |writer, _, _, _| {
        Active::new(crate::values::text(writer, TEXT))
    }) else {
        return step.failed(StepError::Stale);
    };
    let grew = step.writer().in_use() - before >= TEXT.len();
    record(format!("kept {} here {grew}", held.kept.is_some()));
    record(where_text(held.value));
    step.finish(held)
}

/// The child's home is the root work, which asked it with `Keeps`: the value is built there, and
/// the root work receives the same bytes.
#[test]
fn a_value_built_as_forwarded_lands_in_the_home_with_no_copy() {
    let seen = ran(built_forwarded, Use::Keeps);
    assert_eq!(seen[0], "kept true here false");
    assert_eq!(
        seen[1], seen[2],
        "the home holds the bytes the child built there"
    );
}

/// The loop's head: hop into [`threaded`] under a loop cell, which a birth holding `true` asks
/// for.
fn head<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    step.tail(Work {
        step: threaded,
        state: KValue::Bool(true),
    })
}

/// A hop under its loop cell: a value built as threaded over an operand homed in the hop.
fn threaded<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let (mut step, _) = step.state();
    let operand = crate::values::text(step.writer(), TEXT);
    let before = step.writer().in_use();
    let operands = [Held::bare(operand)];
    let Some(held) = step.built_as(Use::Threads, &operands, None, |_, _, values, _| {
        Active::new(values[0])
    }) else {
        return step.failed(StepError::Stale);
    };
    let grew = step.writer().in_use() - before >= TEXT.len();
    record(format!("kept {} here {grew}", held.kept.is_some()));
    record(format!("copied {}", !at(held.value, &where_text(operand))));
    step.finish(held)
}

/// A loop cell copies whatever a hop homes, so the operand is copied into it, never pinned.
#[test]
fn a_value_built_as_threaded_lands_in_the_loop_cell_copying_what_the_hop_homes() {
    let seen = ran(head, Use::Keeps);
    assert_eq!(seen[..2], ["kept true here false", "copied true"]);
}

fn crossed_forwarded<'graph>(
    mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>,
) -> Action<'graph, Native> {
    let value = crate::values::text(step.writer(), TEXT);
    let Some(crossed) = step.crossed_as(Use::Forwards, Held::bare(value), None) else {
        return step.failed(StepError::Stale);
    };
    let passed = PASSED.with(Cell::get);
    let Some(again) = step.crossed_as(Use::Forwards, crossed, None) else {
        return step.failed(StepError::Stale);
    };
    record(format!("kept {}", crossed.kept.is_some()));
    record(format!(
        "unchanged {} passed {}",
        at(again.value, &where_text(crossed.value)),
        PASSED.with(Cell::get) - passed
    ));
    step.finish(again)
}

/// A bare value crosses into the home and comes back kept there; crossed again, it is handed on
/// as it rests.
#[test]
fn a_value_crossed_as_forwarded_is_kept_at_the_home_and_handed_on_unchanged_after() {
    let seen = ran(crossed_forwarded, Use::Keeps);
    assert_eq!(seen[..2], ["kept true", "unchanged true passed 1"]);
}

fn crossed_kept<'graph>(
    mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>,
) -> Action<'graph, Native> {
    let value = crate::values::text(step.writer(), TEXT);
    let Some(held) = step.crossed_as(Use::Keeps, Held::bare(value), None) else {
        return step.failed(StepError::Stale);
    };
    record(format!(
        "kept {} unchanged {}",
        held.kept.is_some(),
        at(held.value, &where_text(value))
    ));
    step.finish(held)
}

#[test]
fn a_bare_value_crossed_as_kept_is_kept_at_the_cell() {
    let seen = ran(crossed_kept, Use::Keeps);
    assert_eq!(seen[0], "kept true unchanged true");
}
