//! What a cell's branch of the type lattice costs at a birth: a spawned child links its spawner's
//! branch rather than copying it, and a recursion that mints at every level holds each level's
//! branch to what that level mints. And what a birth makes resolve: a hop that merges its birth's
//! home reaches what the home read through its link.
//!
//! A native step reads and interns through its own chain over the thread's test root, and reports
//! the length of its write home's branch, which is the cost the birth left it. The types are lists
//! of lists, which name no symbol: `List^k` is `Number` under `k` lists.

use std::cell::Cell;

use crate::knot::KValue;
use crate::memory::{Active, Bump};
use crate::scheduler::tests::bundle::{Native, TestGraph, TestStep, test_root};
use crate::scheduler::tests::native::{fresh, record, recorded, reset, shares, work};
use crate::scheduler::{Action, Placement, Scheduler, Step, Use, Work};
use crate::type_lattice::{Branch, KType};

thread_local! {
    /// How many types the spawner mints before it spawns.
    static MINTED: Cell<usize> = const { Cell::new(0) };
    /// The handle a cell hands its child: a `KType` is `Copy` and carries no brand, so it needs no
    /// crossing to reach the child's step.
    static HANDED: Cell<KType> = const { Cell::new(KType::NUMBER) };
    /// How many levels the recursion has entered, and how many it enters before it stops.
    static LEVEL: Cell<usize> = const { Cell::new(0) };
    static DEPTH: Cell<usize> = const { Cell::new(0) };
}

/// The list of the handed type, interned through `step`'s chain and handed on in its place.
fn mint_list(step: &Step<'_, '_, '_, '_, '_, Native>) {
    let handed = HANDED.with(Cell::get);
    let minted = step.types(test_root()).list(handed);
    HANDED.with(|slot| slot.set(minted));
}

/// The length of `step`'s write home's branch, or 0 where it has none.
fn branch_length(step: &Step<'_, '_, '_, '_, '_, Native>) -> usize {
    step.header().get().map_or(0, Branch::len)
}

/// Run `start` as a root work over a slab of `cells`, starting from `Number`, and hand back what
/// it recorded.
fn run(cells: u32, start: TestStep<'static>) -> Vec<String> {
    reset();
    HANDED.with(|handed| handed.set(KType::NUMBER));
    let mut graph: TestGraph<'static> = TestGraph::new(cells);
    let root = graph.root().expect("a fresh slab admits a root");
    Scheduler::over(&mut graph)
        .run(work(start, KValue::Null), root, Placement::Fresh)
        .expect("the root work ends");
    graph.release_root(root).expect("the root releases");
    assert!(graph.is_empty(), "the run leaves no cell behind");
    recorded()
}

/// The spawner: mint `List^1` through `List^n`, hand the last to a `Fresh` child, park on it.
fn spawner<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    for _ in 0..MINTED.with(Cell::get) {
        mint_list(&step);
    }
    let asked = step.spawn(fresh(reader, Use::Reads, KValue::Null));
    step.park(asked, woken, KValue::Null, None)
}

/// The child: read the handed type, then report its branch's length.
fn reader<'graph>(step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let _ = step.types(test_root()).node(HANDED.with(Cell::get));
    record(branch_length(&step).to_string());
    step.finish_fresh(test_root(), |_, _, _| Active::new(KValue::Null))
}

/// A root work, woken by the child it parked on: take the result and end.
fn woken<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let _ = step.results(test_root()).count();
    step.done()
}

#[test]
fn a_child_links_its_spawners_branch() {
    for minted in [8, 256] {
        MINTED.with(|count| count.set(minted));
        assert_eq!(
            run(2, spawner),
            ["1"],
            "a child born under a spawner holding {minted} types holds only the one it read"
        );
    }
}

/// A child level of the recursion, which delivers to the level above it.
fn level<'graph>(step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    descend(step, relay)
}

/// The recursion's first level, a root work, which ends with nothing to deliver.
fn first<'graph>(step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    descend(step, woken)
}

/// Mint the list of the level above's type, then either spawn the next level `Fresh` and park on
/// it, to wake at `wake`, or, at the bottom, report the branch's length. Each level reads the type
/// it lists once, through its link, so its branch holds that type and its own.
fn descend<'graph>(
    mut step: Step<'_, 'graph, '_, '_, '_, Native>,
    wake: TestStep<'graph>,
) -> Action<'graph, Native> {
    let at = LEVEL.with(|level| level.replace(level.get() + 1));
    mint_list(&step);
    if at + 1 == DEPTH.with(Cell::get) {
        record(branch_length(&step).to_string());
        return step.finish_fresh(test_root(), |_, _, _| Active::new(KValue::Null));
    }
    let asked = step.spawn(fresh(level, Use::Reads, KValue::Null));
    step.park(asked, wake, KValue::Null, None)
}

/// A parked level, woken by the level below it: take its result and deliver one of its own.
fn relay<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let _ = step.results(test_root()).count();
    step.finish_fresh(test_root(), |_, _, _| Active::new(KValue::Null))
}

/// What the deepest level of a minting recursion `depth` levels deep, one cell per level, reports.
fn deepest(depth: u32) -> Vec<String> {
    LEVEL.with(|level| level.set(0));
    DEPTH.with(|limit| limit.set(depth as usize));
    run(depth, first)
}

#[test]
fn a_minting_recursion_holds_each_level_flat() {
    let shallow = deepest(8);
    assert_eq!(
        shallow,
        ["2"],
        "the deepest level holds the type it read and the one it minted"
    );
    assert_eq!(
        deepest(64),
        shallow,
        "the deepest level's branch holds its own, whatever lies above it"
    );
}

/// The case's spawner: lay `[[[1]]]`, minting `List^1` through `List^3`, and hand it to a `Fresh`
/// child, whose branch links this one.
fn nests<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let types = step.types(test_root());
    let scratch = Bump::new();
    let writer = step.writer();
    let mut nested = KValue::Number(1.0);
    for _ in 0..3 {
        let list = crate::values::List::new(writer, [nested].into_iter(), &types, &scratch);
        nested = KValue::List(list);
    }
    let asked = step.spawn(fresh(lists, Use::Reads, nested));
    step.park(asked, woken, KValue::Null, None)
}

/// The child: lay a list of what it was born with in its own region. Its type, `List^4`, reads
/// `List^3` through the link and caches it, but not `List^2`, which the list still carries. A
/// tenant is handed the list pinned.
fn lists<'graph>(step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let (mut step, state) = step.state();
    let types = step.types(test_root());
    let list = crate::values::List::new(step.writer(), [state].into_iter(), &types, &Bump::new());
    let asked = step.spawn(shares(hops, Use::Reads, KValue::List(list)));
    step.park(asked, relay, KValue::Null, None)
}

/// The tenant: hop `Fresh` with the list. The successor has a branch of its own and links none,
/// and the list's home, the tenant's host, outlives it, so the birth merges the host's branch.
fn hops<'graph>(step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let (step, state) = step.state();
    let work = Work {
        step: arrives,
        state,
    };
    step.tail(Placement::Fresh, work)
}

/// The hop's successor, whose birth passed the habitat check.
fn arrives<'graph>(step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    record(String::from("arrived"));
    step.finish_fresh(test_root(), |_, _, _| Active::new(KValue::Null))
}

#[test]
fn a_hop_reaches_what_its_births_home_read_through_its_link() {
    assert_eq!(run(4, nests), ["arrived"]);
}
