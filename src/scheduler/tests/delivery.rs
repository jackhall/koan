//! The delivery table, one test per cell: each end under each `Use`, read back at the brand the
//! table gives it, with the bytes the producer built recorded beside the bytes the consumer read, so
//! a copy would show as a different address.
//!
//! And a consumer parked on several producers: the count lives in the substrate's receipt run, one
//! producer's delivery completes it, and the consumer wakes exactly once.

use crate::knot::{KValue, KValueFamily};
use crate::memory::{Active, Writer};
use crate::scheduler::tests::bundle::{Native, TestGraph, TestStep};
use crate::scheduler::tests::native::{
    describe, fresh, record, recorded, reset, shares, where_text, work,
};
use crate::scheduler::{
    Action, Placement, Received, Request, Scheduler, Step, StepError, Taken, Use,
};

/// Which end a producer takes, encoded with the `Use` it is asked with into the number a test's
/// root work is born holding: a native step is a bare `fn`, so what it is to do reaches it as data.
const FINISH_FRESH: u32 = 10;
const FINISH_IN_HOME: u32 = 20;
const FINISH: u32 = 30;

fn use_of(code: u32) -> Use {
    match code % 10 {
        0 => Use::Reads,
        1 => Use::Keeps,
        _ => Use::Forwards,
    }
}

/// The number a root work is born holding, for a producer that ends by `end` under `use_`.
fn code_of(end: u32, use_: Use) -> KValue<'static, 'static> {
    let use_ = match use_ {
        Use::Reads => 0,
        Use::Keeps => 1,
        Use::Forwards => 2,
    };
    KValue::Number(f64::from(end + use_))
}

/// The code a step was born holding, beside the step it was taken from.
fn code<'a, 'graph, 'step, 'here, 'scratch>(
    step: Step<'a, 'graph, 'step, 'here, 'scratch, Native>,
) -> (
    Step<'a, 'graph, 'step, 'here, 'scratch, Native, Taken>,
    Option<u32>,
) {
    let (step, state) = step.state();
    match state {
        KValue::Number(code) => (step, Some(code as u32)),
        _ => (step, None),
    }
}

/// The one build every end shares: text in whatever region the door hands it, its address
/// recorded.
fn seven<'graph, 'their>(
    writer: Writer<'their>,
    _: &'their &'graph (),
) -> Active<'graph, 'their, KValueFamily> {
    let value: KValue<'graph, 'their> = crate::values::text(writer, "seven");
    record(format!("built {}", where_text(value)));
    Active::new(value)
}

/// A producer: end by the end its code names.
fn produce<'graph>(step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let (step, code) = code(step);
    let Some(code) = code else {
        return step.failed(StepError::Stale);
    };
    match code - code % 10 {
        FINISH_FRESH => step.finish_fresh(seven),
        FINISH_IN_HOME => step.finish_in_home([], |writer, [], witness| seven(writer, witness)),
        _ => {
            let value = crate::values::text(step.writer(), "seven");
            record(format!("built {}", where_text(value)));
            step.finish(value)
        }
    }
}

/// The consumer: ask for one producer with the `Use` its code names, at the placement the test
/// gives it, and park on its single slot.
fn consume<'graph>(
    step: Step<'_, 'graph, '_, '_, '_, Native>,
    placement: Placement,
) -> Action<'graph, Native> {
    let (mut step, code) = code(step);
    let Some(code) = code else {
        return step.failed(StepError::Stale);
    };
    let born = KValue::Number(f64::from(code));
    let request = match placement {
        Placement::Fresh => fresh(produce, use_of(code), born),
        Placement::Shares => shares(produce, use_of(code), born),
    };
    let asked = step.spawn(request);
    step.park(asked, read_back, KValue::Null, None)
}

fn consume_fresh<'graph>(step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    consume(step, Placement::Fresh)
}

fn consume_shares<'graph>(step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    consume(step, Placement::Shares)
}

/// Record one result as the arm it came back in and the bytes it reads from.
fn record_received(received: Result<Received<'_, '_, '_>, StepError>) -> Result<(), StepError> {
    match received? {
        Received::Scratch(value) => record(format!("read scratch {}", where_text(value))),
        Received::Here(value) => record(format!("read here {}", where_text(value))),
    }
    Ok(())
}

/// The consumer, woken: read its one slot back.
fn read_back<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    for received in step.results().collect::<Vec<_>>() {
        if let Err(error) = record_received(received) {
            return step.failed(error);
        }
    }
    step.done()
}

/// Run one consumer over one producer, and hand back what they recorded: the producer's build,
/// then the consumer's read.
fn one_call(end: u32, use_: Use, consumer: TestStep<'static>) -> Vec<String> {
    reset();
    let mut graph: TestGraph<'static> = TestGraph::new(4);
    let root = graph.root().expect("a fresh slab admits a root");
    let mut scheduler = Scheduler::over(&mut graph);
    scheduler
        .run(work(consumer, code_of(end, use_)), root, Placement::Fresh)
        .expect("the root work ends");
    assert!(
        scheduler.graph().is_live(root),
        "the root outlives the drain"
    );
    graph.release_root(root).expect("the root releases");
    assert!(graph.is_empty(), "the call leaves no cell behind");
    recorded()
}

/// The bytes a record names, after its label.
fn bytes(entry: &str) -> &str {
    entry.rsplit(' ').next().expect("a record names its bytes")
}

/// Assert that the consumer read, in `arm`, the very bytes the producer built.
fn read_as_built(seen: &[String], arm: &str) {
    assert_eq!(seen.len(), 2, "one build and one read: {seen:?}");
    assert!(seen[0].starts_with("built seven@"), "{seen:?}");
    assert!(
        seen[1].starts_with(&format!("read {arm} seven@")),
        "read back in the {arm} arm: {seen:?}"
    );
    assert_eq!(
        bytes(&seen[0]),
        bytes(&seen[1]),
        "the same bytes, not a copy"
    );
}

#[test]
fn a_fresh_build_asked_to_be_read_comes_back_from_scratch() {
    read_as_built(
        &one_call(FINISH_FRESH, Use::Reads, consume_fresh),
        "scratch",
    );
}

#[test]
fn a_fresh_build_asked_to_be_kept_comes_back_here_unmoved() {
    read_as_built(&one_call(FINISH_FRESH, Use::Keeps, consume_fresh), "here");
}

#[test]
fn a_build_in_home_asked_to_be_read_comes_back_here_unmoved() {
    read_as_built(&one_call(FINISH_IN_HOME, Use::Reads, consume_fresh), "here");
}

#[test]
fn a_build_in_home_asked_to_be_kept_comes_back_here_unmoved() {
    read_as_built(&one_call(FINISH_IN_HOME, Use::Keeps, consume_fresh), "here");
}

/// A tenant's value is already in its host, which is its home, so its `finish` crosses nothing.
#[test]
fn a_finished_value_asked_to_be_read_comes_back_here() {
    read_as_built(&one_call(FINISH, Use::Reads, consume_shares), "here");
}

#[test]
fn a_finished_value_asked_to_be_kept_comes_back_here() {
    read_as_built(&one_call(FINISH, Use::Keeps, consume_shares), "here");
}

/// A `Shares` child asked with `Keeps`: its result is built in the caller's own storage from the
/// start and reaches the caller as a carrier homed there.
#[test]
fn a_shares_call_returns_its_result_through_the_callers_storage() {
    read_as_built(&one_call(FINISH_FRESH, Use::Keeps, consume_shares), "here");
}

#[test]
fn both_placements_compute_the_same_value() {
    let from_fresh = one_call(FINISH_FRESH, Use::Reads, consume_fresh);
    let from_shares = one_call(FINISH_FRESH, Use::Keeps, consume_shares);
    // The hints move where the bytes are, never what the program computes.
    let text = |seen: &[String]| bytes(&seen[1]).split('@').next().map(str::to_owned);
    assert_eq!(text(&from_fresh), text(&from_shares));
    assert_eq!(text(&from_fresh).as_deref(), Some("seven"));
}

// ---- Forwards: three levels, so the home the leaf builds in is its spawner's own home. ----

/// The grandparent: lay a marker down in its own region, ask for the middle with `Keeps`, and park.
/// The middle's home is therefore this cell, and a leaf the middle forwards to builds here too.
fn grandparent<'graph>(step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let (mut step, state) = step.state();
    record(format!(
        "marker {}",
        where_text(crate::values::text(step.writer(), "marker"))
    ));
    let asked = step.spawn(fresh(middle, Use::Keeps, state));
    step.park(asked, grandparent_woken, KValue::Null, None)
}

/// The middle: ask for the leaf with `Forwards`, and park.
fn middle<'graph>(step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let (mut step, state) = step.state();
    let asked = step.spawn(fresh(produce, Use::Forwards, state));
    step.park(asked, middle_woken, KValue::Null, None)
}

/// The middle, woken: read the leaf's result and hand it on, kept, to the grandparent.
fn middle_woken<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let Some(Ok(Received::Here(value))) = step.results().next() else {
        return step.failed(StepError::Unredeemable);
    };
    record(format!("middle read here {}", where_text(value)));
    step.finish(value)
}

/// The grandparent, woken: a second marker, then the result.
fn grandparent_woken<'graph>(
    mut step: Step<'_, 'graph, '_, '_, '_, Native>,
) -> Action<'graph, Native> {
    record(format!(
        "marker {}",
        where_text(crate::values::text(step.writer(), "marker"))
    ));
    let Some(Ok(Received::Here(value))) = step.results().next() else {
        return step.failed(StepError::Unredeemable);
    };
    record(format!("grandparent read here {}", describe(value)));
    step.done()
}

/// An address a record names.
fn address(entry: &str) -> usize {
    let hex = bytes(entry)
        .split('@')
        .nth(1)
        .expect("an address after the at sign");
    usize::from_str_radix(hex.trim_start_matches("0x"), 16).expect("a hex address")
}

/// Run the three levels with the leaf ending by `end`, and assert the leaf built in the
/// grandparent's region — between the two markers it laid down there, one before the leaf ran and
/// one after — and the result reached the top as the same text.
fn forwards(end: u32) {
    reset();
    let mut graph: TestGraph<'static> = TestGraph::new(4);
    let root = graph.root().expect("a fresh slab admits a root");
    let mut scheduler = Scheduler::over(&mut graph);
    scheduler
        .run(
            work(grandparent, code_of(end, Use::Forwards)),
            root,
            Placement::Fresh,
        )
        .expect("the root work ends");
    let seen = recorded();
    let entry = |prefix: &str| {
        seen.iter()
            .filter(|entry| entry.starts_with(prefix))
            .cloned()
            .collect::<Vec<_>>()
    };
    let markers = entry("marker");
    let built = entry("built");
    assert_eq!((markers.len(), built.len()), (2, 1), "{seen:?}");
    let (low, high) = {
        let (first, second) = (address(&markers[0]), address(&markers[1]));
        (first.min(second), first.max(second))
    };
    let leaf = address(&built[0]);
    assert!(
        low < leaf && leaf < high,
        "the leaf built between the grandparent's two markers, in its region: {seen:?}"
    );
    assert_eq!(
        entry("middle read here").len(),
        1,
        "the middle read the leaf's result at its own brand: {seen:?}"
    );
    assert_eq!(
        entry("grandparent read here seven"),
        ["grandparent read here seven"]
    );
}

#[test]
fn a_fresh_build_forwarded_is_built_in_the_spawners_home() {
    forwards(FINISH_FRESH);
}

#[test]
fn a_build_in_home_forwarded_is_built_in_the_spawners_home() {
    forwards(FINISH_IN_HOME);
}

/// The leaf's value is built in its own region and crossed into the grandparent's at the verdict's
/// price, so what is checked is that it arrives, at each brand the table names.
#[test]
fn a_finished_value_forwarded_reaches_the_spawners_home() {
    reset();
    let mut graph: TestGraph<'static> = TestGraph::new(4);
    let root = graph.root().expect("a fresh slab admits a root");
    let mut scheduler = Scheduler::over(&mut graph);
    scheduler
        .run(
            work(grandparent, code_of(FINISH, Use::Forwards)),
            root,
            Placement::Fresh,
        )
        .expect("the root work ends");
    let seen = recorded();
    assert!(
        seen.iter()
            .any(|entry| entry.starts_with("middle read here seven@")),
        "{seen:?}"
    );
    assert!(
        seen.iter()
            .any(|entry| entry == "grandparent read here seven"),
        "{seen:?}"
    );
}

// ---- Several producers: one wake. ----

/// How many producers the consumer parks on. Three, so a delivery that is neither the first nor the
/// last has to answer `Outstanding` too.
const PRODUCERS: usize = 3;

/// The consumer: ask for a producer per slot, and park on the run they fill.
fn park_on_three<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    for slot in 0..PRODUCERS - 1 {
        step.spawn(producer(slot));
    }
    // The last spawn hands back the slot the park waits on; the spawns before it went to the slots
    // ahead of it, in that order.
    let asked = step.spawn(producer(PRODUCERS - 1));
    step.park(asked, drain_the_run, KValue::Null, None)
}

/// One producer, born holding the number it scales. Which slot it fills is its spawn position.
fn producer<'graph, 'here>(slot: usize) -> Request<'graph, 'here, Native> {
    fresh(scale, Use::Reads, KValue::Number(slot as f64))
}

/// One producer: fill the slot the drain gave it with ten times the number it was born holding.
fn scale<'graph>(step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    // The number itself, not the value: a value at this producer's brand is not one the consumer's
    // scratch can hold, so what crosses is the word inside it.
    let (step, state) = step.state();
    let KValue::Number(born) = state else {
        return step.failed(StepError::Stale);
    };
    step.finish_fresh(move |_, _| Active::new(KValue::Number(born * 10.0)))
}

/// The consumer, woken: read every slot of the run it registered.
fn drain_the_run<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let results: Vec<_> = step.results().collect();
    record(format!("woke on {}", results.len()));
    for received in results {
        match received {
            Ok(Received::Scratch(value)) => record(describe(value)),
            _ => return step.failed(StepError::Unredeemable),
        }
    }
    step.done()
}

#[test]
fn a_consumer_parked_on_three_producers_wakes_once_when_the_last_slot_fills() {
    reset();
    let mut graph: TestGraph<'static> = TestGraph::new(4);
    let root = graph.root().expect("a fresh slab admits a root");
    let mut scheduler = Scheduler::over(&mut graph);
    scheduler
        .run(work(park_on_three, KValue::Null), root, Placement::Fresh)
        .expect("the root work ends");

    // One wake line, then one line per slot: the consumer ran again exactly once, and only after
    // every producer had filled its slot.
    assert_eq!(
        recorded(),
        ["woke on 3", "0", "10", "20"],
        "the consumer wakes once, with every slot already filled"
    );
    assert!(scheduler.graph().is_live(root));
}
