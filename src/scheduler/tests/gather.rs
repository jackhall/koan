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
use crate::scheduler::tests::native::{record, recorded, reset};
use crate::scheduler::{
    Action, Context, Placement, Request, Resume, Scheduler, ScratchState, Spawns, State, StepError,
    Work,
};

/// The three texts the producers write, one per spawn, in the order the consumer asks for them.
const TEXTS: [&str; 3] = ["first", "second", "third"];

/// Accepts only values at the context's own `'here`, which is invariant: a run read back at
/// `'scratch` does not pass. What the final build asserts about where its result lives.
fn in_storage<'graph, 'here>(
    _: &Context<'graph, '_, 'here, '_>,
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
fn ask_for<'graph>(spawns: &mut Spawns<'graph>, text: &'graph str) -> crate::scheduler::Slot {
    spawns.push(Request {
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
    context: &mut Context<'graph, '_, '_, '_>,
    resume: Resume<'graph, '_, '_>,
    _: &mut Spawns<'graph>,
) -> Action<'graph> {
    let (Some(text), Some(consumer)) = (
        born_text(resume.state),
        resume.provenance.destination.map(|to| to.consumer),
    ) else {
        return Action::failed(StepError::Undeliverable);
    };
    let Ok(placed) =
        context.alloc_into::<KValueFamily, KValueFamily>(consumer, &[], |writer, _| {
            let value: KValue<'graph, '_> = crate::values::text(writer, text);
            Active::new(value)
        })
    else {
        return Action::failed(StepError::Stale);
    };
    let carrier = context.keep(placed);
    Action::deliver_carrier(context, &resume, carrier)
}

/// The consumer's first step: ask for two producers and park on them, carrying nothing in scratch.
fn ask_for_two<'graph>(
    context: &mut Context<'graph, '_, '_, '_>,
    resume: Resume<'graph, '_, '_>,
    spawns: &mut Spawns<'graph>,
) -> Action<'graph> {
    ask_for(spawns, TEXTS[0]);
    let asked = ask_for(spawns, TEXTS[1]);
    Action::park(
        context,
        &resume,
        spawns,
        asked,
        gather_two,
        State::Empty,
        None,
    )
}

/// The second step: bring both results to this cell's `'here`, lay them down as a run in the
/// scratch habitat, ask for a third producer, and park carrying the run.
fn gather_two<'graph, 'here, 'scratch>(
    context: &mut Context<'graph, '_, 'here, 'scratch>,
    resume: Resume<'graph, 'here, 'scratch>,
    spawns: &mut Spawns<'graph>,
) -> Action<'graph> {
    let mut gathered = [KValue::Null; 2];
    for (slot, held) in gathered.iter_mut().enumerate() {
        let Ok(Receipt::Carrier(Ok(carrier))) = context.receipt(slot) else {
            return Action::failed(StepError::Unredeemable);
        };
        // Homed in this very cell, so the verdict pins and nothing is copied: the value reads at
        // this step's `'here` over the bytes the producer wrote.
        *held = crate::values::cross_here(context, &carrier);
        record(where_text(*held));
    }
    // The run lives in the habitat; each element it holds names storage at `'here`.
    let run: &[KValue<'graph, '_>] = context.scratch_writer().fill(2, |index| gathered[index]);

    let asked = ask_for(spawns, TEXTS[2]);
    Action::park(
        context,
        &resume,
        spawns,
        asked,
        build,
        State::Empty,
        Some(ScratchState::Gathered(run)),
    )
}

/// The last step: take the third result and build all three into this cell's storage, straight out
/// of the gathered run.
fn build<'graph, 'here>(
    context: &mut Context<'graph, '_, 'here, '_>,
    resume: Resume<'graph, 'here, '_>,
    _: &mut Spawns<'graph>,
) -> Action<'graph> {
    let Some(ScratchState::Gathered(run)) = resume.scratch else {
        return Action::failed(StepError::Unredeemable);
    };
    let Ok(Receipt::Carrier(Ok(carrier))) = context.receipt(0) else {
        return Action::failed(StepError::Unredeemable);
    };
    let third = crate::values::cross_here(context, &carrier);
    // The gathered values go into storage as they are — the whole point of holding them at
    // `'here` across the park.
    let result = context
        .writer()
        .fill(3, |index| if index < 2 { run[index] } else { third });
    in_storage(context, result);
    for value in result {
        record(where_text(*value));
    }
    Action::done()
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
    let mut scheduler: Scheduler<'static> = Scheduler::new(8);
    scheduler
        .admit(ask_for_two, State::Empty)
        .expect("the slab admits");
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

    assert!(scheduler.is_empty());
}
