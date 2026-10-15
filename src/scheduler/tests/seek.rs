//! Seeking a specialization: the drain finds a frame's specialization in its parent's table before
//! the frame is born, or builds it first with one synchronous step, and the frame borrows it.

use std::cell::RefCell;

use crate::knot::KValue;
use crate::memory::{Active, Bump};
use crate::scheduler::tests::bundle::{Native, TestGraph};
use crate::scheduler::tests::native::{fresh, work};
use crate::scheduler::{
    Action, Placement, Scheduler, Seek, Step, StepError, TIERS, Tiers, Use, Work,
};
use crate::type_lattice::{Habitat, Root};
use crate::values::digest::{DigestHasher, Tag};
use crate::values::{BuildRef, Half, SpecKey};

thread_local! {
    /// The build cell each observing step ran under, in the order they ran.
    static BUILDS: RefCell<Vec<BuildRef>> = const { RefCell::new(Vec::new()) };
}

fn builds() -> Vec<BuildRef> {
    BUILDS.with(|builds| builds.borrow().clone())
}

fn reset() {
    BUILDS.with(|builds| builds.borrow_mut().clear());
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

/// A specialization's step: enlist under the key its state names.
fn specialize<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let (step, state) = step.state();
    let KValue::Number(at) = state else {
        return step.failed(StepError::Refused);
    };
    step.enlist(key(at as usize))
}

/// A seek for the specialization keyed `at` under `parent`.
fn seek<'graph>(at: usize, parent: BuildRef) -> Seek<'graph, Native> {
    Seek {
        parent,
        key: key(at),
        work: work(specialize, KValue::Number(at as f64)),
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
    step.spawn_seeking(fresh(observe, Use::Reads, KValue::Null), first);
    let second = seek(second as usize, BuildRef::ROOT);
    let asked = step.spawn_seeking(fresh(observe, Use::Reads, KValue::Null), second);
    step.park(asked, finished, KValue::Null, None)
}

/// A graph stood up by the load door, its build root's header laid.
fn loaded(type_region: &Bump) -> (TestGraph<'_>, crate::scheduler::Roots) {
    let (graph, roots, ()) = TestGraph::load(2, |step| {
        let _ = Half::of(step.writer(), step.header()).own_or_lay();
        Ok::<_, ()>((Root::seeded(type_region), ()))
    })
    .expect("the load succeeds");
    (graph, roots)
}

#[test]
fn a_missed_specialization_is_built_once_and_found_warm_by_the_next_birth() {
    reset();
    fresh_tally();
    let type_region = Bump::new();
    let (mut graph, roots) = loaded(&type_region);
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

#[test]
fn two_bindings_give_two_specializations_under_one_parent() {
    reset();
    fresh_tally();
    let type_region = Bump::new();
    let (mut graph, roots) = loaded(&type_region);
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
    let mut graph = TestGraph::new(1, Root::seeded(&type_region));
    let root = graph.root().expect("a fresh slab admits a root");
    let mut scheduler = Scheduler::over(&mut graph);
    scheduler
        .run(work(seek_two, KValue::Number(1.0)), root, Placement::Fresh)
        .expect("the root work ends");
    assert_eq!(builds(), [BuildRef::ROOT, BuildRef::ROOT]);
    assert_eq!(specialized(), 0);
}

/// Seek key 1 under the root, then from inside it key 2 under its own specialization, and key 1
/// again under the root.
fn nest<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let (mut step, state) = step.state();
    let KValue::Number(depth) = state else {
        return step.failed(StepError::Refused);
    };
    if depth > 0.0 {
        let asked = step.spawn_seeking(
            fresh(nest, Use::Reads, KValue::Number(0.0)),
            seek(1, BuildRef::ROOT),
        );
        return step.park(asked, finished, KValue::Null, None);
    }
    let mine = step.build_ref(BuildRef::ROOT);
    BUILDS.with(|builds| builds.borrow_mut().push(mine));
    step.spawn_seeking(fresh(observe, Use::Reads, KValue::Null), seek(2, mine));
    let asked = step.spawn_seeking(
        fresh(observe, Use::Reads, KValue::Null),
        seek(1, BuildRef::ROOT),
    );
    step.park(asked, deliver, KValue::Null, None)
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
    let asked = step.spawn_seeking(fresh(observe, Use::Reads, KValue::Null), seek(2, parent));
    step.park(asked, finished, KValue::Null, None)
}

#[test]
fn a_specialization_nests_under_its_frames_and_evicts_leaf_first() {
    reset();
    fresh_tally();
    let type_region = Bump::new();
    let (mut graph, roots) = loaded(&type_region);
    let mut scheduler = Scheduler::over(&mut graph);
    scheduler
        .run(work(nest, KValue::Number(1.0)), roots.run, Placement::Fresh)
        .expect("the root work ends");
    let [outer, inner, again] = builds()[..] else {
        panic!("three frames ran: {:?}", builds())
    };
    assert_ne!(outer, BuildRef::ROOT);
    assert_ne!(
        inner, outer,
        "a binding under a specialization keys a child of it"
    );
    assert_eq!(
        again, outer,
        "a recursive birth of the same key finds the entry"
    );
    assert_eq!(specialized(), 2);
    assert_eq!(scheduler.graph().table_len(roots.build), Ok(Some(1)));
    assert!(
        scheduler.evict(outer).is_err(),
        "a specialization with a child is not evicted"
    );
    scheduler.evict(inner).expect("a leaf evicts");
    scheduler.evict(outer).expect("then its parent");
    // The evicted parent's key is renewed in place by the next miss; a seek under the evicted one
    // births its frame cold.
    reset();
    BUILDS.with(|builds| builds.borrow_mut().push(outer));
    scheduler
        .run(
            work(seek_under_stashed, KValue::Null),
            roots.run,
            Placement::Fresh,
        )
        .expect("the root work ends");
    assert_eq!(
        builds()[1],
        BuildRef::ROOT,
        "a frame under an evicted parent is born cold"
    );
    reset();
    scheduler
        .run(
            work(seek_two, KValue::Number(1.0)),
            roots.run,
            Placement::Fresh,
        )
        .expect("the root work ends");
    assert_eq!(specialized(), 3, "the evicted specialization is rebuilt");
    assert_eq!(scheduler.graph().table_len(roots.build), Ok(Some(1)));
    let [rebuilt, _] = builds()[..] else {
        panic!("two frames ran")
    };
    scheduler.evict(rebuilt).expect("the rebuilt leaf evicts");
    drop(scheduler);
    graph
        .release_root(roots.run)
        .expect("the run root releases");
    graph
        .release_root(roots.build)
        .expect("the build root releases");
    assert!(graph.is_empty());
}

/// Seek key 1, and from inside it hop to a frame of the same callee under the same specialization.
fn hop<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let (mut step, state) = step.state();
    match state {
        KValue::Number(_) => {
            let asked = step.spawn_seeking(
                fresh(hop, Use::Reads, KValue::Null),
                seek(1, BuildRef::ROOT),
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
    let (mut graph, roots) = loaded(&type_region);
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
}
