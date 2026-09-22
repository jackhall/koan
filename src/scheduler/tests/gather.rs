//! Gathering across two parks: a cell collects values that rest in its own storage into a run laid
//! down in its scratch habitat, and the step that finally builds embeds them as they are.
//!
//! What this pins is the scratch state's two positions. The run is a `&'scratch [KValue<'graph,
//! 'here>]`: the slice lives in the habitat and goes when the bump does, while each element it
//! holds names storage at the executing cell's own brand and rides the park at that brand. So the
//! last step builds its result straight out of them — no `keep` at either park, no `redeem` at
//! either wake, and no copy.

use crate::knot::KValue;
use crate::memory::Active;
use crate::scheduler::tests::bundle::{Native, ScratchState, TestGraph};
use crate::scheduler::tests::native::{record, recorded, reset, shares, where_text, work};
use crate::scheduler::{Action, Placement, Received, Scheduler, Slot, Step, StepError, Use};

/// The three texts the producers write, one per spawn, in the order the consumer asks for them.
const TEXTS: [&str; 3] = ["first", "second", "third"];

/// Accepts only values at the step's own `'here`, which is invariant: a run read back at
/// `'scratch` does not pass. What the final build asserts about where its result lives.
fn in_storage<'graph, 'here>(
    _: &Step<'_, 'graph, '_, 'here, '_, Native>,
    _: &'here [KValue<'graph, 'here>],
) {
}

/// Ask for one producer of `text`, placed as a co-tenant and asked with `Keeps`, so its result is
/// built in this cell's own storage and reaches it as a carrier homed here.
fn ask_for<'graph>(step: &mut Step<'_, 'graph, '_, '_, '_, Native>, text: &'graph str) -> Slot {
    step.spawn(shares(produce, Use::Keeps, KValue::Str(text)))
}

/// A producer: build the text it was born holding. Asked with `Keeps`, the build lands in the
/// consumer's storage and is filed as a carrier.
fn produce<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let KValue::Str(text) = step.state() else {
        return step.failed(StepError::Stale);
    };
    step.finish_fresh(move |writer, _| {
        let value: KValue<'graph, '_> = crate::values::text(writer, text);
        Active::new(value)
    })
}

/// The consumer's first step: ask for two producers and park on them, carrying nothing in scratch.
fn ask_for_two<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    ask_for(&mut step, TEXTS[0]);
    let asked = ask_for(&mut step, TEXTS[1]);
    step.park(asked, gather_two, KValue::Null, None)
}

/// The second step: take both results at this cell's `'here`, lay them down as a run in the
/// scratch habitat, ask for a third producer, and park carrying the run.
fn gather_two<'graph, 'here, 'scratch>(
    mut step: Step<'_, 'graph, '_, 'here, 'scratch, Native>,
) -> Action<'graph, Native> {
    let mut gathered = [KValue::Null; 2];
    for (held, received) in gathered.iter_mut().zip(step.results().collect::<Vec<_>>()) {
        // Homed in this very cell, so the verdict pins and nothing is copied: the value reads at
        // this step's `'here` over the bytes the producer wrote.
        let Ok(Received::Here(value)) = received else {
            return step.failed(StepError::Unredeemable);
        };
        *held = value;
        record(where_text(value));
    }
    // The run lives in the habitat; each element it holds names storage at `'here`.
    let run: &[KValue<'graph, '_>] = step.scratch_writer().fill(2, |index| gathered[index]);

    let asked = ask_for(&mut step, TEXTS[2]);
    step.park(
        asked,
        build,
        KValue::Null,
        Some(ScratchState::Gathered(run)),
    )
}

/// The last step: take the third result and build all three into this cell's storage, straight out
/// of the gathered run.
fn build<'graph, 'here>(
    mut step: Step<'_, 'graph, '_, 'here, '_, Native>,
) -> Action<'graph, Native> {
    let Some(ScratchState::Gathered(run)) = step.scratch() else {
        return step.failed(StepError::Unredeemable);
    };
    let Some(Ok(Received::Here(third))) = step.results().next() else {
        return step.failed(StepError::Unredeemable);
    };
    // The gathered values go into storage as they are — the whole point of holding them at
    // `'here` across the park.
    let result = step
        .writer()
        .fill(3, |index| if index < 2 { run[index] } else { third });
    in_storage(&step, result);
    for value in result {
        record(where_text(*value));
    }
    step.done()
}

#[test]
fn a_cell_gathers_here_values_across_two_parks_and_builds_from_them_in_storage() {
    reset();
    let mut graph: TestGraph<'static> = TestGraph::new(8);
    let root = graph.root().expect("a fresh slab admits a root");
    Scheduler::over(&mut graph)
        .run(work(ask_for_two, KValue::Null), root, Placement::Fresh)
        .expect("the root work ends");

    // Two recorded where they were gathered, then three where the result was built.
    let seen = recorded();
    assert_eq!(seen.len(), 5, "two gathered, three built");
    let texts: Vec<&str> = seen
        .iter()
        .map(|entry| entry.split('@').next().expect("one at sign"))
        .collect();
    assert_eq!(texts, ["first", "second", "first", "second", "third"]);
    // Address and all: the two gathered values reached the build unmoved.
    assert_eq!((&seen[0], &seen[1]), (&seen[2], &seen[3]));

    graph.release_root(root).expect("the root releases");
    assert!(graph.is_empty());
}
