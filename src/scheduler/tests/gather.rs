//! Gathering across two parks: a cell collects values that rest in its own storage into a run laid
//! down in its scratch habitat, and the step that finally builds embeds them as they are.
//!
//! What this pins is the scratch state's two positions. The run is a `&'scratch [KValue<'graph,
//! 'here>]`: the slice lives in the habitat and goes when the bump does, while each element it
//! holds names storage at the executing cell's own brand and rides the park at that brand. So the
//! last step builds its result straight out of them — no `keep` at either park, no `redeem` at
//! either wake, and no copy.

use crate::knot::{KValue, KValueFamily};
use crate::memory::{Active, Receipt};
use crate::scheduler::tests::native::{record, recorded, reset, slab};
use crate::scheduler::{
    Action, Graph, Placement, Request, Scheduler, ScratchState, Slot, State, Step, StepError, Work,
};

/// The three texts the producers write, one per spawn, in the order the consumer asks for them.
const TEXTS: [&str; 3] = ["first", "second", "third"];

/// Accepts only values at the step's own `'here`, which is invariant: a run read back at
/// `'scratch` does not pass. What the final build asserts about where its result lives.
fn in_storage<'graph, 'here>(
    _: &Step<'_, 'graph, '_, 'here, '_>,
    _: &'here [KValue<'graph, 'here>],
) {
}

/// The text a producer was born holding, as a borrow of program storage.
fn born_text<'cell>(state: State<'_, 'cell>) -> Option<&'cell str> {
    match state {
        State::Value(KValue::Str(text)) => Some(text),
        _ => None,
    }
}

/// Ask for one producer of `text`, placed as a co-tenant so its result is built in this cell's own
/// storage and reaches it as a carrier homed here.
fn ask_for<'graph>(step: &mut Step<'_, 'graph, '_, '_, '_>, text: &'graph str) -> Slot {
    step.spawn(Request {
        placement: Placement::Shares,
        work: Work {
            step: produce,
            state: State::Value(KValue::Str(text)),
        },
    })
}

/// A producer: build the text it was born holding in the consumer's own region, keep it, and file
/// the carrier. Copied in shape from `tests::calls::place_in_storage`.
fn produce<'graph>(
    mut step: Step<'_, 'graph, '_, '_, '_>,
    state: State<'graph, '_>,
    _: Option<ScratchState<'graph, '_, '_>>,
) -> Action<'graph> {
    let (Some(text), Some(consumer)) = (born_text(state), step.consumer()) else {
        return step.failed(StepError::Undeliverable);
    };
    let Ok(placed) = step.alloc_into::<KValueFamily, KValueFamily>(consumer, &[], |writer, _| {
        let value: KValue<'graph, '_> = crate::values::text(writer, text);
        Active::new(value)
    }) else {
        return step.failed(StepError::Stale);
    };
    let carrier = step.keep(placed);
    step.deliver_carrier(carrier)
}

/// The consumer's first step: ask for two producers and park on them, carrying nothing in scratch.
fn ask_for_two<'graph>(
    mut step: Step<'_, 'graph, '_, '_, '_>,
    _: State<'graph, '_>,
    _: Option<ScratchState<'graph, '_, '_>>,
) -> Action<'graph> {
    ask_for(&mut step, TEXTS[0]);
    let asked = ask_for(&mut step, TEXTS[1]);
    step.park(asked, gather_two, State::Empty, None)
}

/// The second step: bring both results to this cell's `'here`, lay them down as a run in the
/// scratch habitat, ask for a third producer, and park carrying the run.
fn gather_two<'graph, 'here, 'scratch>(
    mut step: Step<'_, 'graph, '_, 'here, 'scratch>,
    _: State<'graph, 'here>,
    _: Option<ScratchState<'graph, 'here, 'scratch>>,
) -> Action<'graph> {
    let mut gathered = [KValue::Null; 2];
    for (slot, held) in gathered.iter_mut().enumerate() {
        let Ok(Receipt::Carrier(Ok(carrier))) = step.receipt(slot) else {
            return step.failed(StepError::Unredeemable);
        };
        // Homed in this very cell, so the verdict pins and nothing is copied: the value reads at
        // this step's `'here` over the bytes the producer wrote.
        *held = step.cross_here(&carrier);
        record(where_text(*held));
    }
    // The run lives in the habitat; each element it holds names storage at `'here`.
    let run: &[KValue<'graph, '_>] = step.scratch_writer().fill(2, |index| gathered[index]);

    let asked = ask_for(&mut step, TEXTS[2]);
    step.park(
        asked,
        build,
        State::Empty,
        Some(ScratchState::Gathered(run)),
    )
}

/// The last step: take the third result and build all three into this cell's storage, straight out
/// of the gathered run.
fn build<'graph, 'here>(
    mut step: Step<'_, 'graph, '_, 'here, '_>,
    _: State<'graph, 'here>,
    scratch: Option<ScratchState<'graph, 'here, '_>>,
) -> Action<'graph> {
    let Some(ScratchState::Gathered(run)) = scratch else {
        return step.failed(StepError::Unredeemable);
    };
    let Ok(Receipt::Carrier(Ok(carrier))) = step.receipt(0) else {
        return step.failed(StepError::Unredeemable);
    };
    let third = step.cross_here(&carrier);
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

/// A text value as "<text>@<address>": what it says, and the bytes it says it from, so a copy is
/// visible as a different address.
fn where_text(value: KValue<'_, '_>) -> String {
    match value {
        KValue::Str(text) => format!("{text}@{:?}", text.as_ptr()),
        _ => String::from("not text"),
    }
}

#[test]
fn a_cell_gathers_here_values_across_two_parks_and_builds_from_them_in_storage() {
    reset();
    let mut graph: Graph<'static> = Graph::new(8);
    let mut scheduler = Scheduler::over(&mut graph);
    scheduler.submit(slab(ask_for_two, State::Empty), 0);
    scheduler.run().expect("the drain runs to empty");

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

    assert!(scheduler.graph().is_empty());
}
