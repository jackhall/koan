//! Seeking a specialization: the drain finds a frame's specialization in its parent's table before
//! the frame is born, or builds it first with one synchronous step, and the frame borrows it,
//! whatever its placement: a `Fresh` frame as its tree cell's lend, a `Shares` one as its tenant's.
//! A missed specialization's branch holds its binding's nodes, copied from the asker's region, so a
//! frame finds them after the asker is gone.

use std::cell::RefCell;
use std::num::NonZeroUsize;
use std::ptr;

use crate::knot::KValue;
use crate::memory::{Active, Bump};
use crate::scheduler::tests::bundle::{Native, TestGraph, TestStep};
use crate::scheduler::tests::native::{fresh, shares, work};
use crate::scheduler::{
    Action, EVICT_BEFORE_STEP, Placement, Scheduler, Seek, Step, StepError, TIERS, Tiers, Use,
    WATCH_LOG, WATCHED, Watched, Work,
};
use crate::scope::{Binding, Settled};
use crate::type_lattice::{KType, TypeNode};
use crate::values::digest::{DigestHasher, Tag};
use crate::values::{BuildRef, KeyedTypes, SpecBinding, SpecBounds, SpecKey};

thread_local! {
    /// The build cell each observing step ran under, in the order they ran.
    static BUILDS: RefCell<Vec<BuildRef>> = const { RefCell::new(Vec::new()) };
    /// A type a frame laid in the agent tier before it parked, to read again once it wakes.
    static LAID: std::cell::Cell<KType> = const { std::cell::Cell::new(KType::NUMBER) };
    /// The keys a [`sequence`] seeks, in order: each under the sequence's own build cell where
    /// the flag is set, else under the root.
    static SEQUENCE: RefCell<Vec<(usize, bool)>> = const { RefCell::new(Vec::new()) };
    /// What [`specialized`] read as a [`sequence`] woke from each of its seeks.
    static COUNTS: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
    /// Where [`settling`] laid its settled facts, and where each [`read_settled`] frame found
    /// them, as addresses: `None` for a frame that found none.
    static SETTLED: RefCell<Vec<Option<usize>>> = const { RefCell::new(Vec::new()) };
    /// The type a [`minting`] asker laid in its own region, which its seek's binding carries.
    static BOUND: std::cell::Cell<KType> = const { std::cell::Cell::new(KType::NUMBER) };
}

fn builds() -> Vec<BuildRef> {
    BUILDS.with(|builds| builds.borrow().clone())
}

fn reset() {
    BUILDS.with(|builds| builds.borrow_mut().clear());
    COUNTS.with(|counts| counts.borrow_mut().clear());
    WATCHED.set(None);
    WATCH_LOG.with(|log| log.borrow_mut().clear());
}

/// Log that the step `name` started, after whatever the drain logged of the watched cell.
fn log(name: &'static str) {
    WATCH_LOG.with(|log| log.borrow_mut().push(Watched::Step(name)));
}

/// What the drain logged of the watched cell just before the step `name` started.
fn before(name: &'static str) -> Watched {
    WATCH_LOG.with(|log| {
        let log = log.borrow();
        let at = (log.iter())
            .position(|line| *line == Watched::Step(name))
            .unwrap_or_else(|| panic!("`{name}` ran: {log:?}"));
        log[at - 1]
    })
}

/// How many specializations the drains on this thread have evicted, and how many frames were born
/// cold, since the last [`fresh_tally`].
fn evicted_and_cold() -> (usize, usize) {
    TIERS.with(|tiers| (tiers.get().evicted, tiers.get().cold_births))
}

/// Bounds of `root` and `nested`.
fn bounds(root: usize, nested: usize) -> SpecBounds {
    SpecBounds {
        root: NonZeroUsize::new(root).expect("a bound is not zero"),
        nested: NonZeroUsize::new(nested).expect("a bound is not zero"),
    }
}

/// How many specializations the drains on this thread have built since the last [`fresh_tally`].
fn specialized() -> usize {
    TIERS.with(|tiers| tiers.get().specialized)
}

/// Start the tally [`specialized`] reads over.
fn fresh_tally() {
    TIERS.with(|tiers| tiers.set(Tiers::default()));
}

/// The key a test names by `at`.
fn key(at: usize) -> SpecKey {
    SpecKey(DigestHasher::new(Tag::Number).count(at).finished())
}

/// A specialization's step: enlist.
fn specialize<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    step.enlist(None)
}

/// A seek for the specialization keyed `at` under `parent`.
fn seek<'graph>(at: usize, parent: BuildRef) -> Seek<'graph, Native> {
    Seek {
        parent,
        key: key(at),
        binding: SpecBinding::default(),
        work: work(specialize, KValue::Null),
    }
}

/// Record the build cell this frame runs under, and finish.
fn observe<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    BUILDS.with(|builds| builds.borrow_mut().push(step.build_ref(BuildRef::ROOT)));
    step.finish_fresh(|_, _, _| Active::new(KValue::Null))
}

/// End once the children are in.
fn finished<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    step.done()
}

/// Ask for two frames seeking the keys the state names: `1` and `1`, or `1` and `2`.
fn seek_two<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let (mut step, state) = step.state();
    let KValue::Number(second) = state else {
        return step.failed(StepError::Refused);
    };
    let first = seek(1, BuildRef::ROOT);
    step.spawn_seeking(fresh(observe, Use::Reads, KValue::Null), Some(first));
    let second = seek(second as usize, BuildRef::ROOT);
    let asked = step.spawn_seeking(fresh(observe, Use::Reads, KValue::Null), Some(second));
    step.park(asked, finished, KValue::Null, None)
}

/// A graph stood up by the load door under `bounds`, its build root's header and table laid.
fn loaded(type_region: &Bump, bounds: SpecBounds) -> (TestGraph<'_>, crate::scheduler::Roots) {
    let (graph, roots, ()) = TestGraph::load(2, bounds, || {
        Ok::<_, ()>((crate::values::tests::seeded_tiers(type_region), ()))
    })
    .expect("the load succeeds");
    (graph, roots)
}

#[test]
fn a_missed_specialization_is_built_once_and_found_warm_by_the_next_birth() {
    reset();
    fresh_tally();
    let type_region = Bump::new();
    let (mut graph, roots) = loaded(&type_region, SpecBounds::default());
    let mut scheduler = Scheduler::over(&mut graph);
    scheduler
        .run(
            work(seek_two, KValue::Number(1.0)),
            roots.run,
            Placement::Fresh,
        )
        .expect("the root work ends");
    let [first, second] = builds()[..] else {
        panic!("two frames ran: {:?}", builds())
    };
    assert_ne!(
        first,
        BuildRef::ROOT,
        "the frame borrows its specialization"
    );
    assert_eq!(
        first, second,
        "the second birth finds the first's specialization"
    );
    assert_eq!(specialized(), 1);
    assert_eq!(
        scheduler.peak_live_cells(),
        2,
        "a specialization is no cell of the run, and no frame parked for it"
    );
    assert_eq!(scheduler.graph().table_len(roots.build), Ok(Some(1)));
}

/// A specialization's step that settles facts: lay an empty record in its region, and enlist
/// with it.
fn settling<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let settled = &step.writer().fill(1, |_| Settled::empty(Binding::EMPTY))[0];
    SETTLED.with(|laid| laid.borrow_mut().push(Some(ptr::from_ref(settled).addr())));
    step.enlist(Some(settled))
}

/// Record where this frame finds its specialization's settled facts, and finish.
fn read_settled<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let found = step.settled().map(|settled| ptr::from_ref(settled).addr());
    SETTLED.with(|read| read.borrow_mut().push(found));
    step.finish_fresh(|_, _, _| Active::new(KValue::Null))
}

/// Ask for two frames under one key, whose specialization settles facts, and a third that seeks
/// none.
fn seek_settled<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let (mut step, _) = step.state();
    for _ in 0..2 {
        let seek = Seek {
            parent: BuildRef::ROOT,
            key: key(1),
            binding: SpecBinding::default(),
            work: work(settling, KValue::Null),
        };
        step.spawn_seeking(fresh(read_settled, Use::Reads, KValue::Null), Some(seek));
    }
    let asked = step.spawn_seeking(fresh(read_settled, Use::Reads, KValue::Null), None);
    step.park(asked, finished, KValue::Null, None)
}

#[test]
fn a_frame_reads_its_specializations_settled_facts_through_its_lend() {
    reset();
    SETTLED.with(|settled| settled.borrow_mut().clear());
    let type_region = Bump::new();
    let (mut graph, roots) = loaded(&type_region, SpecBounds::default());
    Scheduler::over(&mut graph)
        .run(
            work(seek_settled, KValue::Null),
            roots.run,
            Placement::Fresh,
        )
        .expect("the root work ends");
    let seen = SETTLED.with(|settled| settled.borrow().clone());
    let [Some(laid), first, second, none] = seen[..] else {
        panic!("one specialization settled, then three frames read: {seen:?}")
    };
    assert_eq!(
        first,
        Some(laid),
        "a frame reads the facts its specialization laid"
    );
    assert_eq!(
        second,
        Some(laid),
        "a second frame under the binding reads the same facts"
    );
    assert_eq!(
        none, None,
        "a frame that borrows no specialization reads none"
    );
}

/// How many nodes the drains on this thread have laid into the agent tier for new specializations
/// since the last [`fresh_tally`].
fn relayed() -> usize {
    TIERS.with(|tiers| tiers.get().relayed)
}

/// A seek for the specialization keyed `at` under the build root, its binding carrying `bound`,
/// built by `specialize`.
fn seek_bound<'graph>(
    at: usize,
    bound: KType,
    specialize: TestStep<'graph>,
) -> Seek<'graph, Native> {
    let mut binding = SpecBinding::default();
    binding.push(KeyedTypes {
        carried: bound.into(),
        named: None,
    });
    Seek {
        parent: BuildRef::ROOT,
        key: key(at),
        binding,
        work: work(specialize, KValue::Null),
    }
}

/// A specialization's step over a binding the asker minted: its own chain resolves the binding's
/// type through the agent tier before it lays anything, and its region lays no branch; then it
/// enlists.
fn holding<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    {
        let types = step.types();
        let bound = BOUND.get();
        assert!(!types.in_root(bound));
        assert!(
            types.resolves(bound),
            "the drain laid the binding's node before the step"
        );
        assert_eq!(
            types.own_len(),
            0,
            "the specialization's region lays no branch"
        );
    }
    step.enlist(None)
}

/// A specialization's step over a binding of root types: its branch holds nothing.
fn holding_root<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    {
        let types = step.types();
        assert!(types.resolves(KType::NUMBER));
        assert_eq!(types.own_len(), 0, "a root handle lays nothing");
    }
    step.enlist(None)
}

/// A frame that reads its binding's type, a list of a list of `Bool`, through the agent chain, and
/// finishes.
fn reading<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    {
        let warm = step
            .agent_types()
            .expect("the frame borrows its specialization");
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

/// Finish with nothing, so the asker wakes.
fn report<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    step.finish_fresh(|_, _, _| Active::new(KValue::Null))
}

/// An asker that mints a type in its own region and seeks a specialization over it, for a
/// `Fresh` frame.
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
    let seek = seek_bound(7, bound, holding);
    let asked = step.spawn_seeking(fresh(reading, Use::Reads, KValue::Null), Some(seek));
    step.park(asked, report, KValue::Null, None)
}

/// An asker that mints nothing and seeks the same specialization, for a `Shares` frame: a hit.
fn rehitting<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let (mut step, _) = step.state();
    let seek = seek_bound(7, BOUND.get(), holding);
    let asked = step.spawn_seeking(shares(reading, Use::Reads, KValue::Null), Some(seek));
    step.park(asked, report, KValue::Null, None)
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

/// Run [`mint_then_rehit`] over a fresh graph from `state`, and read the specializations built
/// and the nodes relayed.
fn run_minting(state: f64) -> (usize, usize) {
    reset();
    fresh_tally();
    let type_region = Bump::new();
    let (mut graph, roots) = loaded(&type_region, SpecBounds::default());
    Scheduler::over(&mut graph)
        .run(
            work(mint_then_rehit, KValue::Number(state)),
            roots.run,
            Placement::Fresh,
        )
        .expect("the root work ends");
    (specialized(), relayed())
}

#[test]
fn a_missed_specialization_holds_its_bindings_nodes_and_a_frame_reads_them_after_the_asker_is_gone()
{
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

/// Seek a specialization over a binding of `Number` alone.
fn seek_root_bound<'graph>(
    step: Step<'_, 'graph, '_, '_, '_, '_, Native>,
) -> Action<'graph, Native> {
    let (mut step, _) = step.state();
    let seek = seek_bound(8, KType::NUMBER, holding_root);
    let asked = step.spawn_seeking(fresh(observe, Use::Reads, KValue::Null), Some(seek));
    step.park(asked, finished, KValue::Null, None)
}

#[test]
fn a_binding_of_root_types_lays_nothing_in_its_specialization() {
    reset();
    fresh_tally();
    let type_region = Bump::new();
    let (mut graph, roots) = loaded(&type_region, SpecBounds::default());
    Scheduler::over(&mut graph)
        .run(
            work(seek_root_bound, KValue::Null),
            roots.run,
            Placement::Fresh,
        )
        .expect("the root work ends");
    assert_eq!((specialized(), relayed()), (1, 0));
}

#[test]
fn two_bindings_give_two_specializations_under_one_parent() {
    reset();
    fresh_tally();
    let type_region = Bump::new();
    let (mut graph, roots) = loaded(&type_region, SpecBounds::default());
    let mut scheduler = Scheduler::over(&mut graph);
    scheduler
        .run(
            work(seek_two, KValue::Number(2.0)),
            roots.run,
            Placement::Fresh,
        )
        .expect("the root work ends");
    let [first, second] = builds()[..] else {
        panic!("two frames ran: {:?}", builds())
    };
    assert_ne!(first, second);
    assert_eq!(specialized(), 2);
    assert_eq!(scheduler.graph().table_len(roots.build), Ok(Some(2)));
}

#[test]
fn a_graph_with_no_build_root_births_every_seeking_frame_cold() {
    reset();
    fresh_tally();
    let type_region = Bump::new();
    let mut graph = TestGraph::new(1, crate::values::tests::seeded_tiers(&type_region));
    let root = graph.root().expect("a fresh slab admits a root");
    let mut scheduler = Scheduler::over(&mut graph);
    scheduler
        .run(work(seek_two, KValue::Number(1.0)), root, Placement::Fresh)
        .expect("the root work ends");
    assert_eq!(builds(), [BuildRef::ROOT, BuildRef::ROOT]);
    assert_eq!(specialized(), 0);
}

/// Deliver once the children are in.
fn deliver<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    step.finish_fresh(|_, _, _| Active::new(KValue::Null))
}

/// Seek, as a frame of a callee keyed on the stashed build cell, under the build cell stashed
/// first.
fn seek_under_stashed<'graph>(
    mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>,
) -> Action<'graph, Native> {
    let parent = builds()[0];
    let asked = step.spawn_seeking(
        fresh(observe, Use::Reads, KValue::Null),
        Some(seek(2, parent)),
    );
    step.park(asked, finished, KValue::Null, None)
}

/// Record this frame's build cell, then seek the [`SEQUENCE`] one key at a time: ask for one
/// observing frame, park on it, and ask for the next when woken. Deliver once the last is in.
fn sequence<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let (mut step, state) = step.state();
    let KValue::Number(at) = state else {
        return step.failed(StepError::Refused);
    };
    let (at, mine) = (at as usize, step.build_ref(BuildRef::ROOT));
    if at == 0 {
        BUILDS.with(|builds| builds.borrow_mut().push(mine));
    } else {
        COUNTS.with(|counts| counts.borrow_mut().push(specialized()));
        // Take the last seek's result, so the slot is free for the next.
        step.results().for_each(drop);
    }
    let Some((key, under_mine)) = SEQUENCE.with(|sequence| sequence.borrow().get(at).copied())
    else {
        return step.finish_fresh(|_, _, _| Active::new(KValue::Null));
    };
    let parent = if under_mine { mine } else { BuildRef::ROOT };
    let asked = step.spawn_seeking(
        fresh(observe, Use::Reads, KValue::Null),
        Some(seek(key, parent)),
    );
    step.park(asked, sequence, KValue::Number((at + 1) as f64), None)
}

/// Run a [`sequence`] of `keys` as a root work's child, seeking `under` for it where given.
fn run_sequence(
    scheduler: &mut Scheduler<'_, '_, Native>,
    run: crate::memory::SlabHandle,
    keys: &[(usize, bool)],
    under: Option<usize>,
) {
    fn plain<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
        let asked = step.spawn(fresh(sequence, Use::Reads, KValue::Number(0.0)));
        step.park(asked, finished, KValue::Null, None)
    }
    fn keyed<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
        let request = fresh(sequence, Use::Reads, KValue::Number(0.0));
        let asked = step.spawn_seeking(request, Some(seek(1, BuildRef::ROOT)));
        step.park(asked, finished, KValue::Null, None)
    }
    reset();
    SEQUENCE.with(|sequence| *sequence.borrow_mut() = keys.to_vec());
    let start = match under {
        None => plain,
        Some(1) => keyed,
        Some(other) => panic!("a sequence runs under key 1 or the root, not {other}"),
    };
    scheduler
        .run(work(start, KValue::Null), run, Placement::Fresh)
        .expect("the root work ends");
}

/// Whether the specialization `build` names is live.
fn live(scheduler: &Scheduler<'_, '_, Native>, build: BuildRef) -> bool {
    scheduler
        .graph()
        .is_live(build.opened().expect("a specialization"))
}

/// Evict every specialization, release both roots, and check the graph is empty.
fn torn_down(mut graph: TestGraph<'_>, roots: crate::scheduler::Roots) {
    Scheduler::over(&mut graph).evict_every();
    graph
        .release_root(roots.run)
        .expect("the run root releases");
    graph
        .release_root(roots.build)
        .expect("the build root releases");
    assert!(graph.is_empty(), "the run leaves no cell behind");
}

#[test]
fn a_parent_past_its_bound_releases_its_least_recently_born_child() {
    fresh_tally();
    let type_region = Bump::new();
    let (mut graph, roots) = loaded(&type_region, bounds(3, 2));
    let mut scheduler = Scheduler::over(&mut graph);
    let under_root = [1, 2, 3, 1, 4, 2].map(|key| (key, false));
    run_sequence(&mut scheduler, roots.run, &under_root, None);
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
        "key 4 evicts key 2, born before key 1's hit and key 3"
    );
    assert!(!live(&scheduler, three), "key 2 then evicts key 3");
    assert!([one, four, rebuilt].iter().all(|b| live(&scheduler, *b)));
    assert_ne!(rebuilt, two);
    assert_eq!(evicted_and_cold(), (2, 0));
    assert_eq!(scheduler.graph().live_children(roots.build), 3);
    // Under key 1's specialization the nested bound holds.
    let nested = [10, 11, 12].map(|key| (key, true));
    run_sequence(&mut scheduler, roots.run, &nested, Some(1));
    let [mine, ten, ..] = builds()[..] else {
        panic!("four frames ran: {:?}", builds())
    };
    assert_eq!(mine, one);
    assert!(!live(&scheduler, ten), "key 12 evicts key 10");
    assert_eq!(evicted_and_cold(), (3, 0));
    assert_eq!(
        scheduler
            .graph()
            .live_children(mine.opened().expect("a specialization")),
        2
    );
    assert_eq!(scheduler.graph().live_specializations(), 5);
    drop(scheduler);
    torn_down(graph, roots);
}

#[test]
fn a_miss_releases_its_victims_subtree_leaf_first() {
    fresh_tally();
    let type_region = Bump::new();
    let (mut graph, roots) = loaded(&type_region, bounds(1, 1));
    let mut scheduler = Scheduler::over(&mut graph);
    run_sequence(&mut scheduler, roots.run, &[(2, true)], Some(1));
    let [outer, inner] = builds()[..] else {
        panic!("two frames ran: {:?}", builds())
    };
    assert_ne!(outer, BuildRef::ROOT);
    assert_ne!(
        inner, outer,
        "a binding under a specialization keys a child of it"
    );
    assert_eq!(scheduler.graph().live_specializations(), 2);
    run_sequence(&mut scheduler, roots.run, &[(3, false)], None);
    assert!(
        !live(&scheduler, outer) && !live(&scheduler, inner),
        "key 3 evicts key 1 with its child"
    );
    assert_eq!(evicted_and_cold(), (2, 0));
    assert_eq!(scheduler.graph().live_specializations(), 1);
    // A seek under the evicted parent births its frame cold.
    reset();
    BUILDS.with(|builds| builds.borrow_mut().push(outer));
    scheduler
        .run(
            work(seek_under_stashed, KValue::Null),
            roots.run,
            Placement::Fresh,
        )
        .expect("the root work ends");
    assert_eq!(builds()[1], BuildRef::ROOT);
    assert_eq!(evicted_and_cold(), (2, 1));
    // A test's eviction of a specialization with a child releases both.
    run_sequence(&mut scheduler, roots.run, &[(2, true)], Some(1));
    let [outer, inner] = builds()[..] else {
        panic!("two frames ran: {:?}", builds())
    };
    scheduler
        .evict(outer)
        .expect("a live specialization evicts");
    assert!(!live(&scheduler, outer) && !live(&scheduler, inner));
    assert_eq!(scheduler.graph().live_specializations(), 0);
    drop(scheduler);
    torn_down(graph, roots);
}

/// Seek key 1, and from inside it hop to a frame of the same callee under the same specialization.
fn hop<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let (mut step, state) = step.state();
    match state {
        KValue::Number(_) => {
            let asked = step.spawn_seeking(
                fresh(hop, Use::Reads, KValue::Null),
                Some(seek(1, BuildRef::ROOT)),
            );
            step.park(asked, finished, KValue::Null, None)
        }
        _ => {
            let mine = step.build_ref(BuildRef::ROOT);
            BUILDS.with(|builds| builds.borrow_mut().push(mine));
            step.tail_under(
                Placement::Fresh,
                Work {
                    step: observe,
                    state,
                },
                mine,
            )
        }
    }
}

/// Hop under the build cell stashed first, as a frame whose specialization was evicted would.
fn hop_under_stashed<'graph>(
    step: Step<'_, 'graph, '_, '_, '_, '_, Native>,
) -> Action<'graph, Native> {
    let (step, state) = step.state();
    let stashed = builds()[0];
    step.tail_under(
        Placement::Fresh,
        Work {
            step: observe,
            state,
        },
        stashed,
    )
}

/// Ask for one frame that hops under the stashed build cell.
fn ask_hop<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let asked = step.spawn(fresh(hop_under_stashed, Use::Reads, KValue::Null));
    step.park(asked, finished, KValue::Null, None)
}

#[test]
fn a_hop_of_the_same_callee_borrows_its_frames_specialization_and_runs_cold_once_it_is_gone() {
    reset();
    fresh_tally();
    let type_region = Bump::new();
    let (mut graph, roots) = loaded(&type_region, SpecBounds::default());
    let mut scheduler = Scheduler::over(&mut graph);
    scheduler
        .run(work(hop, KValue::Number(1.0)), roots.run, Placement::Fresh)
        .expect("the root work ends");
    let [frame, successor] = builds()[..] else {
        panic!("two frames ran: {:?}", builds())
    };
    assert_ne!(frame, BuildRef::ROOT);
    assert_eq!(
        successor, frame,
        "the successor borrows its predecessor's specialization"
    );
    assert_eq!(specialized(), 1, "a hop looks nothing up");
    scheduler.evict(frame).expect("the leaf evicts");
    reset();
    BUILDS.with(|builds| builds.borrow_mut().push(frame));
    scheduler
        .run(work(ask_hop, KValue::Null), roots.run, Placement::Fresh)
        .expect("a hop after its specialization's eviction still runs");
    assert_eq!(builds()[1], BuildRef::ROOT);
    assert_eq!(evicted_and_cold(), (1, 1));
}

/// A frame keyed 1 that hops under its own specialization, seeks key 2, hops again, and seeks key
/// 3, by stage.
fn hopper<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let (mut step, state) = step.state();
    let KValue::Number(stage) = state else {
        return step.failed(StepError::Refused);
    };
    let mine = step.build_ref(BuildRef::ROOT);
    let next = KValue::Number(stage + 1.0);
    match stage as usize {
        0 => {
            BUILDS.with(|builds| builds.borrow_mut().push(mine));
            let work = Work {
                step: hopper,
                state: next,
            };
            step.tail_under(Placement::Fresh, work, mine)
        }
        1 | 3 => {
            let key = if stage == 1.0 { 2 } else { 3 };
            let asked = step.spawn_seeking(
                fresh(observe, Use::Reads, KValue::Null),
                Some(seek(key, BuildRef::ROOT)),
            );
            step.park(asked, hopper, next, None)
        }
        2 => step.tail_under(
            Placement::Fresh,
            Work {
                step: hopper,
                state: next,
            },
            mine,
        ),
        _ => {
            BUILDS.with(|builds| builds.borrow_mut().push(mine));
            step.finish_fresh(|_, _, _| Active::new(KValue::Null))
        }
    }
}

#[test]
fn a_hop_is_a_birth_and_keeps_its_loops_specialization() {
    fn start<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
        let request = fresh(hopper, Use::Reads, KValue::Number(0.0));
        let asked = step.spawn_seeking(request, Some(seek(1, BuildRef::ROOT)));
        step.park(asked, finished, KValue::Null, None)
    }
    reset();
    fresh_tally();
    let type_region = Bump::new();
    let (mut graph, roots) = loaded(&type_region, bounds(2, 1));
    let mut scheduler = Scheduler::over(&mut graph);
    scheduler
        .run(work(start, KValue::Null), roots.run, Placement::Fresh)
        .expect("the root work ends");
    let [frame, two, three, last] = builds()[..] else {
        panic!("four frames ran: {:?}", builds())
    };
    assert_eq!(last, frame, "every hop runs warm");
    assert!(
        live(&scheduler, frame),
        "two hops are newer births than key 2's"
    );
    assert!(!live(&scheduler, two), "key 3 evicts key 2");
    assert!(live(&scheduler, three));
    assert_eq!((specialized(), evicted_and_cold()), (3, (1, 0)));
    drop(scheduler);
    torn_down(graph, roots);
}

/// Run `start` as a root work over a loaded graph, then tear it down to an empty graph.
fn run_loaded(start: crate::scheduler::tests::bundle::AnyStep) {
    reset();
    fresh_tally();
    let type_region = Bump::new();
    let (mut graph, roots) = loaded(&type_region, SpecBounds::default());
    let mut scheduler = Scheduler::over(&mut graph);
    scheduler
        .run(work(start, KValue::Null), roots.run, Placement::Fresh)
        .expect("the root work ends");
    drop(scheduler);
    torn_down(graph, roots);
}

/// Record the build cell this frame runs under, then ask for a child copying this frame's lend
/// and deliver once it is in.
fn observe_and_ask<'graph>(
    step: Step<'_, 'graph, '_, '_, '_, '_, Native>,
) -> Action<'graph, Native> {
    let (mut step, _) = step.state();
    BUILDS.with(|builds| builds.borrow_mut().push(step.build_ref(BuildRef::ROOT)));
    let asked = step.spawn(shares(observe, Use::Reads, KValue::Null));
    step.park(asked, deliver, KValue::Null, None)
}

#[test]
fn a_shares_frame_seeking_borrows_its_specialization_and_its_children_copy_it() {
    fn start<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
        let request = shares(observe_and_ask, Use::Reads, KValue::Null);
        step.spawn_seeking(request, Some(seek(1, BuildRef::ROOT)));
        let request = shares(observe, Use::Reads, KValue::Null);
        let asked = step.spawn_seeking(request, Some(seek(1, BuildRef::ROOT)));
        step.park(asked, finished, KValue::Null, None)
    }
    run_loaded(start);
    let [first, child, second] = builds()[..] else {
        panic!("three frames ran: {:?}", builds())
    };
    assert_ne!(
        first,
        BuildRef::ROOT,
        "the tenant borrows its specialization"
    );
    assert_eq!(
        child, first,
        "a tenant asked for plainly copies its asker's lend"
    );
    assert_eq!(
        second, first,
        "the second tenant finds the first's specialization"
    );
    assert_eq!(specialized(), 1);
}

#[test]
fn a_shares_call_seeking_nothing_runs_under_its_fallback() {
    /// Under a frame borrowing a specialization: a `Shares` call seeking nothing, then a `Shares`
    /// child copying the frame's lend.
    fn frame<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
        let (mut step, _) = step.state();
        step.spawn_seeking(shares(observe, Use::Reads, KValue::Null), None);
        let asked = step.spawn(shares(observe, Use::Reads, KValue::Null));
        step.park(asked, deliver, KValue::Null, None)
    }
    fn start<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
        let request = fresh(frame, Use::Reads, KValue::Null);
        let asked = step.spawn_seeking(request, Some(seek(1, BuildRef::ROOT)));
        step.park(asked, finished, KValue::Null, None)
    }
    run_loaded(start);
    let [cold, copied] = builds()[..] else {
        panic!("two frames ran: {:?}", builds())
    };
    assert_eq!(
        cold,
        BuildRef::ROOT,
        "a call seeking nothing borrows nothing"
    );
    assert_ne!(
        copied,
        BuildRef::ROOT,
        "a plain child copies the frame's lend"
    );
}

#[test]
fn a_shares_hop_of_the_same_callee_keeps_its_lend() {
    /// A `Shares` frame that hops, `Shares`, to a frame of the same callee under its own
    /// specialization.
    fn frame<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
        let (step, state) = step.state();
        let mine = step.build_ref(BuildRef::ROOT);
        BUILDS.with(|builds| builds.borrow_mut().push(mine));
        let work = Work {
            step: observe,
            state,
        };
        step.tail_under(Placement::Shares, work, mine)
    }
    fn start<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
        let request = shares(frame, Use::Reads, KValue::Null);
        let asked = step.spawn_seeking(request, Some(seek(1, BuildRef::ROOT)));
        step.park(asked, finished, KValue::Null, None)
    }
    run_loaded(start);
    let [frame, successor] = builds()[..] else {
        panic!("two frames ran: {:?}", builds())
    };
    assert_ne!(frame, BuildRef::ROOT);
    assert_eq!(
        successor, frame,
        "the successor borrows its predecessor's specialization"
    );
    assert_eq!(specialized(), 1, "a hop looks nothing up");
}

/// A keyed frame: lay a type in the agent tier, ask for its specialization's eviction before the
/// next step, and park on a child.
fn evicted<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    log("frame");
    let (mut step, _) = step.state();
    let warm = step
        .agent_types()
        .expect("the frame borrows its specialization");
    let laid = warm.list(warm.list(KType::BOOL));
    LAID.with(|slot| slot.set(laid));
    let mine = step.build_ref(BuildRef::ROOT);
    assert_ne!(mine, BuildRef::ROOT, "the frame borrows its specialization");
    EVICT_BEFORE_STEP.set(Some(mine));
    let asked = step.spawn(fresh(observe, Use::Reads, KValue::Null));
    step.park(asked, woken, KValue::Null, None)
}

/// Wake after the eviction and read the type laid before it, through the agent chain and the
/// caching chain, then finish.
fn woken<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    log("frame woken");
    let laid = LAID.with(std::cell::Cell::get);
    let warm = step
        .agent_types()
        .expect("the frame still borrows its specialization");
    let before = warm.own_len();
    assert_eq!(
        warm.list(laid),
        warm.list(laid),
        "the agent tier still reads"
    );
    let chain = step.caching_types();
    let _ = chain.list(laid);
    assert_eq!(
        warm.own_len(),
        before + 1,
        "the list of the laid type lays once"
    );
    BUILDS.with(|builds| builds.borrow_mut().push(step.build_ref(BuildRef::ROOT)));
    step.finish_fresh(|_, _, _| Active::new(KValue::Null))
}

/// A frame keyed 0 under the root, the host: watch its specialization, and ask for an [`evicted`]
/// frame keyed 1 under it, `Fresh` for a state of `0` and `Shares` for `1`.
fn host<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    log("host");
    let (mut step, state) = step.state();
    let KValue::Number(placement) = state else {
        return step.failed(StepError::Refused);
    };
    let mine = step.build_ref(BuildRef::ROOT);
    WATCHED.set(Some(mine));
    let request = if placement == 0.0 {
        fresh(evicted, Use::Reads, KValue::Null)
    } else {
        shares(evicted, Use::Reads, KValue::Null)
    };
    let asked = step.spawn_seeking(request, Some(seek(1, mine)));
    step.park(asked, host_woken, state, None)
}

/// The host, its frame ended: under a `Fresh` frame, seek key 1 again; under a `Shares` one, ask
/// for a plain child [`plain`].
fn host_woken<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    log("host woken");
    let (mut step, state) = step.state();
    step.results().for_each(drop);
    let asked = if matches!(state, KValue::Number(0.0)) {
        let mine = step.build_ref(BuildRef::ROOT);
        step.spawn_seeking(
            fresh(observe, Use::Reads, KValue::Null),
            Some(seek(1, mine)),
        )
    } else {
        step.spawn(fresh(plain, Use::Reads, KValue::Null))
    };
    step.park(asked, deliver, KValue::Null, None)
}

/// A plain child that logs its step.
fn plain<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    log("plain");
    step.finish_fresh(|_, _, _| Active::new(KValue::Null))
}

/// Run a [`host`] keyed 0 under the root, its frame `Fresh` for `placement` `0` and `Shares` for
/// `1`, then tear the graph down.
fn run_host(placement: f64) {
    fn fresh_host<'graph>(
        mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>,
    ) -> Action<'graph, Native> {
        let request = fresh(host, Use::Reads, KValue::Number(0.0));
        let asked = step.spawn_seeking(request, Some(seek(0, BuildRef::ROOT)));
        step.park(asked, finished, KValue::Null, None)
    }
    fn shares_host<'graph>(
        mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>,
    ) -> Action<'graph, Native> {
        let request = fresh(host, Use::Reads, KValue::Number(1.0));
        let asked = step.spawn_seeking(request, Some(seek(0, BuildRef::ROOT)));
        step.park(asked, finished, KValue::Null, None)
    }
    run_loaded(if placement == 0.0 {
        fresh_host
    } else {
        shares_host
    });
    assert_eq!(EVICT_BEFORE_STEP.get(), None, "the drain evicted it");
}

#[test]
fn an_evicted_specialization_runs_its_fresh_frame_to_its_end_and_disposes_with_it() {
    run_host(0.0);
    let [child, frame, rebuilt] = builds()[..] else {
        panic!("three frames ran: {:?}", builds())
    };
    assert_eq!(child, BuildRef::ROOT);
    assert_ne!(
        frame,
        BuildRef::ROOT,
        "the evicted specialization is still the frame's"
    );
    assert_eq!(
        before("frame woken"),
        Watched::Childless(false),
        "the frame still borrows the evicted specialization"
    );
    assert_eq!(
        before("host woken"),
        Watched::Childless(true),
        "the frame's release disposes it"
    );
    assert_ne!(rebuilt, frame, "the next miss builds a fresh one");
    assert_eq!(specialized(), 3);
}

#[test]
fn an_evicted_specialization_a_shares_frame_borrowed_waits_on_its_hosts_scratch() {
    run_host(1.0);
    let [child, frame] = builds()[..] else {
        panic!("two frames ran: {:?}", builds())
    };
    assert_eq!(
        child,
        BuildRef::ROOT,
        "the frame's tree child borrows its host's scratch"
    );
    assert_ne!(
        frame,
        BuildRef::ROOT,
        "the evicted specialization is still the frame's"
    );
    assert_eq!(
        before("host woken"),
        Watched::Childless(false),
        "the host's scratch holds it past the frame's release"
    );
    assert_eq!(
        before("plain"),
        Watched::Childless(true),
        "it disposes once the host's scratch resets"
    );
}
