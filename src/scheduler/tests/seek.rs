//! Seeking a settlement: the drain finds a frame's settlement in the build arena before the frame
//! is born, or builds it first, and the frame runs under it, whatever its placement. A missed
//! settlement's binding image is relayed into the agent tier, so a frame finds its nodes after the
//! asker is gone. Every run cell borrows the arena node it runs under from its birth to its
//! release, so a miss past the budget evicts only settlements no cell holds.

use std::cell::RefCell;
use std::num::NonZeroUsize;

use smallvec::smallvec;

use crate::knot::KValue;
use crate::memory::{Active, Bump, SlabHandle};
use crate::scheduler::tests::bundle::{AnyStep, Native, TestGraph};
use crate::scheduler::tests::native::{fresh, shares, work};
use crate::scheduler::{
    Action, Placement, Scheduler, Seek, Step, StepError, TIERS, Taken, Tiers, Use, Work,
};
use crate::type_lattice::{KType, TypeNode};
use crate::values::digest::{DigestHasher, Tag};
use crate::values::{ArenaRef, BindingImage, BuildBudget, KeyedTypes, SettlementKey};

thread_local! {
    /// The arena node each observing step ran under, in the order they ran.
    static BUILDS: RefCell<Vec<ArenaRef>> = const { RefCell::new(Vec::new()) };
    /// The keys a [`sequence`] seeks, in order: each under the sequence's own arena node where
    /// the flag is set, else under the root.
    static SEQUENCE: RefCell<Vec<(usize, bool)>> = const { RefCell::new(Vec::new()) };
    /// What [`settled`] read as a [`sequence`] woke from each of its seeks.
    static COUNTS: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
    /// The type a [`minting`] asker laid in its own region, which its seek's binding carries.
    static BOUND: std::cell::Cell<KType> = const { std::cell::Cell::new(KType::NUMBER) };
    /// How many settlements had been evicted each time [`noted`] ran.
    static EVICTED_THEN: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

fn builds() -> Vec<ArenaRef> {
    BUILDS.with(|builds| builds.borrow().clone())
}

fn push_build(at: ArenaRef) {
    BUILDS.with(|builds| builds.borrow_mut().push(at));
}

fn reset() {
    BUILDS.with(|builds| builds.borrow_mut().clear());
    COUNTS.with(|counts| counts.borrow_mut().clear());
    EVICTED_THEN.with(|evicted| evicted.borrow_mut().clear());
}

/// How many settlements the drains on this thread have evicted, and how many frames were born
/// cold, since the last [`fresh_tally`].
fn evicted_and_cold() -> (usize, usize) {
    TIERS.with(|tiers| (tiers.get().evicted, tiers.get().cold_births))
}

/// How many settlements the drains on this thread have built since the last [`fresh_tally`].
fn settled() -> usize {
    TIERS.with(|tiers| tiers.get().settled)
}

/// How many nodes the drains on this thread have relayed into the agent tier for new settlements
/// since the last [`fresh_tally`].
fn relayed() -> usize {
    TIERS.with(|tiers| tiers.get().relayed)
}

/// Start the tallies over.
fn fresh_tally() {
    TIERS.with(|tiers| tiers.set(Tiers::default()));
}

/// The key a test names by `at`.
fn key(at: usize) -> SettlementKey {
    SettlementKey(DigestHasher::new(Tag::Number).count(at).finished())
}

/// A seek for the settlement keyed `at` under `parent`, its binding carrying no type.
fn seek<'graph>(at: usize, parent: ArenaRef) -> Seek<'graph> {
    Seek {
        parent,
        settlement: Some((key(at), BindingImage::default())),
    }
}

/// A budget of `bytes`.
fn budget(bytes: usize) -> BuildBudget {
    BuildBudget {
        bytes: NonZeroUsize::new(bytes).expect("a budget is not zero"),
    }
}

/// The smallest budget: every miss evicts every settlement no cell holds.
fn smallest() -> BuildBudget {
    BuildBudget {
        bytes: NonZeroUsize::MIN,
    }
}

/// A graph stood up by the load door under `budget`, and its run root.
fn loaded(type_region: &Bump, budget: BuildBudget) -> (TestGraph<'_>, SlabHandle) {
    let (graph, run, ()) = TestGraph::load(1, budget, || {
        Ok::<_, ()>((
            crate::values::tests::seeded_tiers(type_region),
            &crate::scope::LoadFacts::EMPTY,
            (),
        ))
    })
    .expect("the load succeeds");
    (graph, run)
}

/// Release the run root and check the graph is empty.
fn torn_down(mut graph: TestGraph<'_>, run: SlabHandle) {
    graph.release_root(run).expect("the run root releases");
    assert!(graph.is_empty(), "the run leaves no cell behind");
}

/// Record the arena node this frame runs under, and finish.
fn observe<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    push_build(step.arena_ref(ArenaRef::ROOT));
    step.finish_fresh(|_, _, _| Active::new(KValue::Null))
}

/// End once the children are in.
fn finished<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    step.done()
}

/// Deliver once the children are in.
fn deliver<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    step.finish_fresh(|_, _, _| Active::new(KValue::Null))
}

/// Ask for two frames seeking the keys the state names: `1` and `1`, or `1` and `2`.
fn seek_two<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let (mut step, state) = step.state();
    let KValue::Number(second) = state else {
        return step.failed(StepError::Refused);
    };
    step.spawn_seeking(
        fresh(observe, Use::Reads, KValue::Null),
        seek(1, ArenaRef::ROOT),
    );
    let second = seek(second as usize, ArenaRef::ROOT);
    let asked = step.spawn_seeking(fresh(observe, Use::Reads, KValue::Null), second);
    step.park(asked, finished, KValue::Null, None)
}

/// Run `start` over a graph loaded under `budget`, then tear it down; hand back the bytes of the
/// first arena node it recorded.
fn run_loaded(start: AnyStep, budget: BuildBudget) -> Option<usize> {
    reset();
    fresh_tally();
    let type_region = Bump::new();
    let (mut graph, run) = loaded(&type_region, budget);
    Scheduler::over(&mut graph)
        .run(work(start, KValue::Null), run, Placement::Fresh)
        .expect("the root work ends");
    let bytes = (builds().first()).and_then(|first| graph.settlement_bytes(*first));
    torn_down(graph, run);
    bytes
}

/// What one settlement weighs: every settlement in these tests weighs the same.
fn settlement_bytes() -> usize {
    fn start<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
        let request = fresh(observe, Use::Reads, KValue::Null);
        let asked = step.spawn_seeking(request, seek(1, ArenaRef::ROOT));
        step.park(asked, finished, KValue::Null, None)
    }
    run_loaded(start, BuildBudget::default()).expect("the frame ran under its settlement")
}

#[test]
fn a_missed_settlement_is_built_once_and_found_warm_by_the_next_birth() {
    reset();
    fresh_tally();
    let type_region = Bump::new();
    let (mut graph, run) = loaded(&type_region, BuildBudget::default());
    let mut scheduler = Scheduler::over(&mut graph);
    scheduler
        .run(work(seek_two, KValue::Number(1.0)), run, Placement::Fresh)
        .expect("the root work ends");
    let [first, second] = builds()[..] else {
        panic!("two frames ran: {:?}", builds())
    };
    assert_ne!(first, ArenaRef::ROOT, "the frame runs under its settlement");
    assert_eq!(
        first, second,
        "the second birth finds the first's settlement"
    );
    assert_eq!(settled(), 1);
    assert_eq!(
        scheduler.peak_live_cells(),
        2,
        "a settlement is no cell of the run, and no frame parked for it"
    );
    assert_eq!(scheduler.graph().live_settlements(), 1);
    drop(scheduler);
    torn_down(graph, run);
}

#[test]
fn two_bindings_give_two_settlements_under_one_parent() {
    reset();
    fresh_tally();
    let type_region = Bump::new();
    let (mut graph, run) = loaded(&type_region, BuildBudget::default());
    Scheduler::over(&mut graph)
        .run(work(seek_two, KValue::Number(2.0)), run, Placement::Fresh)
        .expect("the root work ends");
    let [first, second] = builds()[..] else {
        panic!("two frames ran: {:?}", builds())
    };
    assert_ne!(first, second);
    assert_eq!(settled(), 2);
    assert_eq!(graph.live_settlements(), 2);
    torn_down(graph, run);
}

#[test]
fn a_call_keying_no_settlement_runs_under_its_callees_arena_node() {
    /// Under a frame keyed 1: a call keying nothing whose callee was born under the root, one
    /// whose callee was born under this frame's settlement, and a plain child.
    fn frame<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
        let (mut step, _) = step.state();
        let mine = step.arena_ref(ArenaRef::ROOT);
        for parent in [ArenaRef::ROOT, mine] {
            let unkeyed = Seek {
                parent,
                settlement: None,
            };
            step.spawn_seeking(shares(observe, Use::Reads, KValue::Null), unkeyed);
        }
        let asked = step.spawn(fresh(observe, Use::Reads, KValue::Null));
        step.park(asked, deliver, KValue::Null, None)
    }
    fn start<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
        let request = fresh(frame, Use::Reads, KValue::Null);
        let asked = step.spawn_seeking(request, seek(1, ArenaRef::ROOT));
        step.park(asked, finished, KValue::Null, None)
    }
    run_loaded(start, BuildBudget::default());
    let [under_root, under_mine, plain] = builds()[..] else {
        panic!("three frames ran: {:?}", builds())
    };
    assert_eq!(under_root, ArenaRef::ROOT);
    assert_ne!(under_mine, ArenaRef::ROOT);
    assert_eq!(
        plain, under_mine,
        "a plain child runs under its asker's settlement"
    );
    assert_eq!(settled(), 1);
}

/// A seek for the settlement keyed `at` under the root, its binding carrying `bound`, imaged from
/// `step`'s region.
fn seek_bound<'graph>(
    step: &Step<'_, 'graph, '_, '_, '_, '_, Native, Taken>,
    at: usize,
    bound: KType,
) -> Seek<'graph> {
    let keyed = smallvec![KeyedTypes {
        carried: bound.into(),
        named: None,
    }];
    let image = BindingImage::of(keyed, step.tiers(), &step.types());
    Seek {
        parent: ArenaRef::ROOT,
        settlement: Some((key(at), image)),
    }
}

/// A frame that reads its binding's type, a list of a list of `Bool`, through the agent chain, and
/// finishes.
fn reading<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    {
        let warm = step
            .agent_types()
            .expect("the frame runs under its settlement");
        let bound = BOUND.get();
        assert!(
            warm.resolves(bound),
            "the agent tier holds the frame's binding"
        );
        let TypeNode::List { element } = warm.node(bound) else {
            panic!("the binding is a list")
        };
        assert!(matches!(
            warm.node(element),
            TypeNode::List { element } if element == KType::BOOL
        ));
    }
    step.finish_fresh(|_, _, _| Active::new(KValue::Null))
}

/// An asker that mints a type in its own region and seeks a settlement over it, for a `Fresh`
/// frame.
fn minting<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let (mut step, _) = step.state();
    let bound = {
        let types = step.types();
        let bound = types.list(types.list(KType::BOOL));
        assert!(
            !types.in_root(bound),
            "the asker minted it in its own region"
        );
        bound
    };
    BOUND.set(bound);
    let seek = seek_bound(&step, 7, bound);
    let asked = step.spawn_seeking(fresh(reading, Use::Reads, KValue::Null), seek);
    step.park(asked, deliver, KValue::Null, None)
}

/// An asker that mints nothing and seeks the same settlement, for a `Shares` frame: a hit.
fn rehitting<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let (mut step, _) = step.state();
    let seek = seek_bound(&step, 7, BOUND.get());
    let asked = step.spawn_seeking(shares(reading, Use::Reads, KValue::Null), seek);
    step.park(asked, deliver, KValue::Null, None)
}

/// Ask for a minting asker, and once it is gone, where the state is `2`, a rehitting one.
fn mint_then_rehit<'graph>(
    step: Step<'_, 'graph, '_, '_, '_, '_, Native>,
) -> Action<'graph, Native> {
    let (mut step, state) = step.state();
    let asked = step.spawn(fresh(minting, Use::Reads, KValue::Null));
    let next = if matches!(state, KValue::Number(2.0)) {
        rehit_after
    } else {
        finished
    };
    step.park(asked, next, KValue::Null, None)
}

/// The second half of [`mint_then_rehit`]: the minting asker has finished and been retired.
fn rehit_after<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let (mut step, _) = step.state();
    step.results().for_each(drop);
    let asked = step.spawn(fresh(rehitting, Use::Reads, KValue::Null));
    step.park(asked, finished, KValue::Null, None)
}

/// Run [`mint_then_rehit`] over a fresh graph from `state`, and read the settlements built and
/// the nodes relayed.
fn run_minting(state: f64) -> (usize, usize) {
    reset();
    fresh_tally();
    let type_region = Bump::new();
    let (mut graph, run) = loaded(&type_region, BuildBudget::default());
    Scheduler::over(&mut graph)
        .run(
            work(mint_then_rehit, KValue::Number(state)),
            run,
            Placement::Fresh,
        )
        .expect("the root work ends");
    torn_down(graph, run);
    (settled(), relayed())
}

#[test]
fn a_missed_settlement_relays_its_bindings_nodes_and_a_frame_reads_them_after_the_asker_is_gone() {
    let once = run_minting(1.0);
    assert_eq!(
        once,
        (1, 2),
        "one miss, laying the list and its element's list"
    );
    assert_eq!(
        run_minting(2.0),
        once,
        "the second seek hits, and a hit relays nothing"
    );
}

#[test]
fn a_binding_of_root_types_relays_nothing() {
    fn start<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
        let (mut step, _) = step.state();
        let seek = seek_bound(&step, 8, KType::NUMBER);
        let asked = step.spawn_seeking(fresh(observe, Use::Reads, KValue::Null), seek);
        step.park(asked, finished, KValue::Null, None)
    }
    run_loaded(start, BuildBudget::default());
    assert_eq!((settled(), relayed()), (1, 0));
}

/// Record this frame's arena node, then seek the [`SEQUENCE`] one key at a time: ask for one
/// observing frame, park on it, and ask for the next when woken. Deliver once the last is in.
fn sequence<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let (mut step, state) = step.state();
    let KValue::Number(at) = state else {
        return step.failed(StepError::Refused);
    };
    let (at, mine) = (at as usize, step.arena_ref(ArenaRef::ROOT));
    if at == 0 {
        push_build(mine);
    } else {
        COUNTS.with(|counts| counts.borrow_mut().push(settled()));
        // Take the last seek's result, so the slot is free for the next.
        step.results().for_each(drop);
    }
    let Some((key, under_mine)) = SEQUENCE.with(|sequence| sequence.borrow().get(at).copied())
    else {
        return step.finish_fresh(|_, _, _| Active::new(KValue::Null));
    };
    let parent = if under_mine { mine } else { ArenaRef::ROOT };
    let asked = step.spawn_seeking(fresh(observe, Use::Reads, KValue::Null), seek(key, parent));
    step.park(asked, sequence, KValue::Number((at + 1) as f64), None)
}

/// Run a [`sequence`] of `keys` as a root work's child, itself a frame keyed 1 where `keyed`.
fn run_sequence(
    scheduler: &mut Scheduler<'_, '_, Native>,
    run: SlabHandle,
    keys: &[(usize, bool)],
    keyed: bool,
) {
    fn plain<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
        let asked = step.spawn(fresh(sequence, Use::Reads, KValue::Number(0.0)));
        step.park(asked, finished, KValue::Null, None)
    }
    fn keyed_one<'graph>(
        mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>,
    ) -> Action<'graph, Native> {
        let request = fresh(sequence, Use::Reads, KValue::Number(0.0));
        let asked = step.spawn_seeking(request, seek(1, ArenaRef::ROOT));
        step.park(asked, finished, KValue::Null, None)
    }
    reset();
    SEQUENCE.with(|sequence| *sequence.borrow_mut() = keys.to_vec());
    let start = if keyed { keyed_one } else { plain };
    scheduler
        .run(work(start, KValue::Null), run, Placement::Fresh)
        .expect("the root work ends");
}

/// Whether `at` names a live settlement.
fn live(scheduler: &Scheduler<'_, '_, Native>, at: ArenaRef) -> bool {
    scheduler.graph().is_settled(at)
}

#[test]
fn a_miss_past_the_budget_evicts_the_least_recently_used_unheld_settlement() {
    let two = budget(2 * settlement_bytes());
    fresh_tally();
    let type_region = Bump::new();
    let (mut graph, run) = loaded(&type_region, two);
    let mut scheduler = Scheduler::over(&mut graph);
    let under_root = [1, 2, 3, 1, 4, 2].map(|key| (key, false));
    run_sequence(&mut scheduler, run, &under_root, false);
    assert_eq!(
        COUNTS.with(|counts| counts.borrow().clone()),
        [1, 2, 3, 3, 4, 5]
    );
    let [_, one, two, three, again, four, rebuilt] = builds()[..] else {
        panic!("seven frames ran: {:?}", builds())
    };
    assert_eq!(again, one, "key 1 is a hit");
    assert!(
        !live(&scheduler, two),
        "key 4 evicts key 2, used before key 1's hit and key 3"
    );
    assert!(!live(&scheduler, three), "key 2 then evicts key 3");
    assert!([one, four, rebuilt].iter().all(|at| live(&scheduler, *at)));
    assert_ne!(rebuilt, two, "a stale handle names nothing");
    assert_eq!(evicted_and_cold(), (2, 0));
    assert_eq!(scheduler.graph().live_settlements(), 3);
    assert_eq!(
        scheduler.graph().unheld_bytes(),
        3 * scheduler.graph().settlement_bytes(one).unwrap(),
        "the last miss's settlement turned unheld when its frame ended"
    );
    drop(scheduler);
    torn_down(graph, run);
}

#[test]
fn a_parent_goes_after_its_child_and_a_seek_under_it_then_runs_cold() {
    fresh_tally();
    let type_region = Bump::new();
    let (mut graph, run) = loaded(&type_region, smallest());
    let mut scheduler = Scheduler::over(&mut graph);
    run_sequence(&mut scheduler, run, &[(2, true)], true);
    let [outer, inner] = builds()[..] else {
        panic!("two frames ran: {:?}", builds())
    };
    assert_ne!(outer, ArenaRef::ROOT);
    assert_ne!(
        inner, outer,
        "a binding under a settlement keys a child of it"
    );
    assert_eq!(
        scheduler.graph().live_settlements(),
        2,
        "neither is evicted while a frame holds it"
    );
    run_sequence(&mut scheduler, run, &[(3, false)], false);
    assert!(
        !live(&scheduler, outer) && !live(&scheduler, inner),
        "key 3's miss evicts the child, then its parent"
    );
    assert_eq!(evicted_and_cold(), (2, 0));
    // A seek under the evicted parent births its frame cold.
    fn under_outer<'graph>(
        mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>,
    ) -> Action<'graph, Native> {
        let parent = builds()[0];
        let asked = step.spawn_seeking(fresh(observe, Use::Reads, KValue::Null), seek(2, parent));
        step.park(asked, finished, KValue::Null, None)
    }
    reset();
    push_build(outer);
    scheduler
        .run(work(under_outer, KValue::Null), run, Placement::Fresh)
        .expect("the root work ends");
    assert_eq!(builds()[1], ArenaRef::ROOT);
    assert_eq!(evicted_and_cold(), (2, 1));
    drop(scheduler);
    torn_down(graph, run);
}

/// Seek key 1, and from inside it hop to a frame of the same callee under the same settlement.
fn hop<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let (mut step, state) = step.state();
    match state {
        KValue::Number(_) => {
            let asked = step.spawn_seeking(
                fresh(hop, Use::Reads, KValue::Null),
                seek(1, ArenaRef::ROOT),
            );
            step.park(asked, finished, KValue::Null, None)
        }
        _ => {
            let mine = step.arena_ref(ArenaRef::ROOT);
            push_build(mine);
            let work = Work {
                step: observe,
                state,
            };
            step.tail_under(Placement::Fresh, work, mine)
        }
    }
}

#[test]
fn a_hop_of_the_same_callee_runs_under_its_frames_settlement_and_cold_once_it_is_gone() {
    reset();
    fresh_tally();
    let type_region = Bump::new();
    let (mut graph, run) = loaded(&type_region, smallest());
    let mut scheduler = Scheduler::over(&mut graph);
    scheduler
        .run(work(hop, KValue::Number(1.0)), run, Placement::Fresh)
        .expect("the root work ends");
    let [frame, successor] = builds()[..] else {
        panic!("two frames ran: {:?}", builds())
    };
    assert_ne!(frame, ArenaRef::ROOT);
    assert_eq!(
        successor, frame,
        "the successor runs under its predecessor's settlement"
    );
    assert_eq!(settled(), 1, "a hop looks nothing up");
    // Another key's miss evicts the frame's settlement, which nothing holds now.
    run_sequence(&mut scheduler, run, &[(2, false)], false);
    assert!(!live(&scheduler, frame));
    fn hop_under_stashed<'graph>(
        step: Step<'_, 'graph, '_, '_, '_, '_, Native>,
    ) -> Action<'graph, Native> {
        let (step, state) = step.state();
        let work = Work {
            step: observe,
            state,
        };
        step.tail_under(Placement::Fresh, work, builds()[0])
    }
    fn ask_hop<'graph>(
        mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>,
    ) -> Action<'graph, Native> {
        let asked = step.spawn(fresh(hop_under_stashed, Use::Reads, KValue::Null));
        step.park(asked, finished, KValue::Null, None)
    }
    reset();
    push_build(frame);
    scheduler
        .run(work(ask_hop, KValue::Null), run, Placement::Fresh)
        .expect("a hop after its settlement's eviction still runs");
    assert_eq!(builds()[1], ArenaRef::ROOT);
    assert_eq!(evicted_and_cold(), (1, 1));
    drop(scheduler);
    torn_down(graph, run);
}

/// A frame keyed 1 that hops under its own settlement, seeks key 2, hops again, and seeks key 3,
/// by stage.
fn hopper<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let (mut step, state) = step.state();
    let KValue::Number(stage) = state else {
        return step.failed(StepError::Refused);
    };
    let mine = step.arena_ref(ArenaRef::ROOT);
    let next = KValue::Number(stage + 1.0);
    match stage as usize {
        0 | 2 => {
            if stage == 0.0 {
                push_build(mine);
            }
            let work = Work {
                step: hopper,
                state: next,
            };
            step.tail_under(Placement::Fresh, work, mine)
        }
        1 | 3 => {
            let key = if stage == 1.0 { 2 } else { 3 };
            let request = fresh(observe, Use::Reads, KValue::Null);
            let asked = step.spawn_seeking(request, seek(key, ArenaRef::ROOT));
            step.park(asked, hopper, next, None)
        }
        _ => {
            push_build(mine);
            step.finish_fresh(|_, _, _| Active::new(KValue::Null))
        }
    }
}

#[test]
fn a_loop_of_hops_holds_its_settlement_through_every_miss() {
    fn start<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
        let request = fresh(hopper, Use::Reads, KValue::Number(0.0));
        let asked = step.spawn_seeking(request, seek(1, ArenaRef::ROOT));
        step.park(asked, finished, KValue::Null, None)
    }
    reset();
    fresh_tally();
    let type_region = Bump::new();
    let (mut graph, run) = loaded(&type_region, smallest());
    let mut scheduler = Scheduler::over(&mut graph);
    scheduler
        .run(work(start, KValue::Null), run, Placement::Fresh)
        .expect("the root work ends");
    let [frame, two, three, last] = builds()[..] else {
        panic!("four frames ran: {:?}", builds())
    };
    assert_eq!(last, frame, "every hop runs warm");
    assert!(live(&scheduler, frame), "a hop's successor holds it");
    assert!(!live(&scheduler, two), "key 3 evicts key 2");
    assert!(live(&scheduler, three), "nothing missed after key 3");
    assert_eq!((settled(), evicted_and_cold()), (3, (1, 0)));
    drop(scheduler);
    torn_down(graph, run);
}

/// Record the arena node this frame runs under, then ask for an evaluation (`Fresh`) and a block
/// (`Shares`) asked for plainly, and deliver once they are in.
fn observe_and_ask<'graph>(
    step: Step<'_, 'graph, '_, '_, '_, '_, Native>,
) -> Action<'graph, Native> {
    let (mut step, _) = step.state();
    push_build(step.arena_ref(ArenaRef::ROOT));
    step.spawn(fresh(observe, Use::Reads, KValue::Null));
    let asked = step.spawn(shares(observe, Use::Reads, KValue::Null));
    step.park(asked, deliver, KValue::Null, None)
}

#[test]
fn every_run_cell_born_under_a_settlement_carries_its_reference() {
    fn start<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
        let request = fresh(observe_and_ask, Use::Reads, KValue::Null);
        step.spawn_seeking(request, seek(1, ArenaRef::ROOT));
        let request = shares(observe_and_ask, Use::Reads, KValue::Null);
        let asked = step.spawn_seeking(request, seek(1, ArenaRef::ROOT));
        step.park(asked, finished, KValue::Null, None)
    }
    run_loaded(start, BuildBudget::default());
    let seen = builds();
    assert_eq!(
        seen.len(),
        6,
        "two frames and their four children ran: {seen:?}"
    );
    assert_ne!(seen[0], ArenaRef::ROOT, "a frame runs under its settlement");
    assert!(
        seen.iter().all(|at| *at == seen[0]),
        "both placements find one settlement, and an evaluation and a block run under it"
    );
    assert_eq!(settled(), 1);
}

#[test]
fn a_shares_hop_of_the_same_callee_keeps_its_settlement() {
    /// A `Shares` frame that hops, `Shares`, to a frame of the same callee under its own
    /// settlement.
    fn frame<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
        let (step, state) = step.state();
        let mine = step.arena_ref(ArenaRef::ROOT);
        push_build(mine);
        let work = Work {
            step: observe,
            state,
        };
        step.tail_under(Placement::Shares, work, mine)
    }
    fn start<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
        let request = shares(frame, Use::Reads, KValue::Null);
        let asked = step.spawn_seeking(request, seek(1, ArenaRef::ROOT));
        step.park(asked, finished, KValue::Null, None)
    }
    run_loaded(start, BuildBudget::default());
    let [frame, successor] = builds()[..] else {
        panic!("two frames ran: {:?}", builds())
    };
    assert_ne!(frame, ArenaRef::ROOT);
    assert_eq!(
        successor, frame,
        "the successor runs under its predecessor's settlement"
    );
    assert_eq!(settled(), 1, "a hop looks nothing up");
}

/// Ask for an [`observe`] frame seeking key 2 under the root — a miss past the smallest budget —
/// and, once it is in, note the evictions so far.
fn miss_then_note<'graph>(
    mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>,
) -> Action<'graph, Native> {
    let request = fresh(observe, Use::Reads, KValue::Null);
    let asked = step.spawn_seeking(request, seek(2, ArenaRef::ROOT));
    step.park(asked, noted, KValue::Null, None)
}

/// [`miss_then_note`]'s second step: note the evictions so far, and record this cell's arena node.
fn noted<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let evicted = TIERS.with(|tiers| tiers.get().evicted);
    EVICTED_THEN.with(|then| then.borrow_mut().push(evicted));
    push_build(step.arena_ref(ArenaRef::ROOT));
    step.finish_fresh(|_, _, _| Active::new(KValue::Null))
}

#[test]
fn a_hand_off_successor_holds_its_frames_settlement_until_its_own_release() {
    /// A frame keyed 1 that records its settlement and hands off to [`miss_then_note`].
    fn frame<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
        let (step, _) = step.state();
        push_build(step.arena_ref(ArenaRef::ROOT));
        let work = Work {
            step: miss_then_note,
            state: KValue::Null,
        };
        step.tail(Placement::Fresh, work)
    }
    fn start<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
        let request = fresh(frame, Use::Reads, KValue::Null);
        let asked = step.spawn_seeking(request, seek(1, ArenaRef::ROOT));
        step.park(asked, finished, KValue::Null, None)
    }
    reset();
    fresh_tally();
    let type_region = Bump::new();
    let (mut graph, run) = loaded(&type_region, smallest());
    let mut scheduler = Scheduler::over(&mut graph);
    scheduler
        .run(work(start, KValue::Null), run, Placement::Fresh)
        .expect("the root work ends");
    let [frame, _two, successor] = builds()[..] else {
        panic!("three frames ran: {:?}", builds())
    };
    assert_ne!(frame, ArenaRef::ROOT);
    assert_eq!(
        successor, frame,
        "the successor runs under its frame's settlement"
    );
    assert_eq!(
        EVICTED_THEN.with(|then| then.borrow().clone()),
        [0],
        "the miss in the successor's life evicted nothing: the successor held the settlement"
    );
    assert!(
        live(&scheduler, frame),
        "nothing missed after the successor's release"
    );
    run_sequence(&mut scheduler, run, &[(3, false)], false);
    assert!(
        !live(&scheduler, frame),
        "the next miss evicts it, held by no cell"
    );
    drop(scheduler);
    torn_down(graph, run);
}

#[test]
fn a_tenants_count_runs_from_its_creation_to_its_release() {
    fn start<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
        let request = shares(miss_then_note, Use::Reads, KValue::Null);
        let asked = step.spawn_seeking(request, seek(1, ArenaRef::ROOT));
        step.park(asked, finished, KValue::Null, None)
    }
    reset();
    fresh_tally();
    let type_region = Bump::new();
    let (mut graph, run) = loaded(&type_region, smallest());
    let mut scheduler = Scheduler::over(&mut graph);
    scheduler
        .run(work(start, KValue::Null), run, Placement::Fresh)
        .expect("the root work ends");
    let [_two, tenant] = builds()[..] else {
        panic!("two frames ran: {:?}", builds())
    };
    assert_ne!(tenant, ArenaRef::ROOT);
    assert_eq!(
        EVICTED_THEN.with(|then| then.borrow().clone()),
        [0],
        "a miss while the tenant lived evicted nothing"
    );
    assert_eq!(
        scheduler.graph().borrowers(tenant),
        Some(0),
        "its release dropped its count"
    );
    run_sequence(&mut scheduler, run, &[(3, false)], false);
    assert!(!live(&scheduler, tenant));
    drop(scheduler);
    torn_down(graph, run);
}
