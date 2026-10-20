//! What a cell's branch of the type lattice costs at a birth: a spawned child links its spawner's
//! branch rather than copying it, and a recursion that mints at every level holds each level's
//! branch to what that level mints. And what a birth makes resolve and costs: a hop reaches what
//! its birth's home read through its link, and each way a cell deep in a recursion hops or hands a
//! result up costs the same however deep it sits, as does a tail cycle's merges into its host.
//!
//! A native step reads and interns through its own chain over the thread's test root, and reports
//! the length of its write home's branch, which is the cost the birth left it. The types are lists
//! of lists, which name no symbol: `List^k` is `Number` under `k` lists.

use std::cell::Cell;

use crate::knot::KValue;
use crate::memory::{Active, Bump};
use crate::scheduler::tests::bundle::{AnyStep, Native, TestGraph, TestStep};
use crate::scheduler::tests::native::{fresh, record, recorded, reset, shares, work};
use crate::scheduler::{Action, Placement, Scheduler, Step, Use, Work};
use crate::type_lattice::KType;

thread_local! {
    /// How many types the spawner mints before it spawns.
    static MINTED: Cell<usize> = const { Cell::new(0) };
    /// The handle a cell hands its child: a `KType` is `Copy` and carries no brand, so it needs no
    /// crossing to reach the child's step.
    static HANDED: Cell<KType> = const { Cell::new(KType::NUMBER) };
    /// How many levels the recursion has entered, and how many it enters before it stops.
    static LEVEL: Cell<usize> = const { Cell::new(0) };
    static DEPTH: Cell<usize> = const { Cell::new(0) };
    /// What the recursion's deepest level does with a list it lays, rather than reporting its
    /// own branch.
    static SHAPE: Cell<Option<Shape>> = const { Cell::new(None) };
    /// How many frames a tail cycle has entered.
    static FRAMES: Cell<usize> = const { Cell::new(0) };
}

/// How many frames a tail cycle enters before it reports.
const CYCLE_FRAMES: usize = 3;

/// A way the deepest level of a recursion hands a list it laid onward, each measured by the
/// branch length of the cell the list lands in, but for a cycle, which reports its merges' cost.
#[derive(Clone, Copy, Debug)]
enum Shape {
    /// To a tenant, which hops `Fresh` with it: the successor is a tree child of the level.
    TenantFresh,
    /// To a `Fresh` child, which hops `Fresh` with it: the list is homed in the child's parent,
    /// but the hop's state rests in the child, a sibling of the successor, so it is copied.
    TreeFresh,
    /// To a `Fresh` child, which lays a list of it and hops `Fresh` with that, homed in itself.
    TreeFreshCopied,
    /// To a `Fresh` child, which lays a list of it and hops `Shares` with that.
    TreeShares,
    /// To a tenant, which lays a list of it in its host and hops `Shares` with that.
    TenantShares,
    /// To a `Fresh` child asked to keep its result, which hands the list back up.
    KeptResult,
    /// To a `Fresh` child, which lays a list of it and hops `Shares` to a tenant of this level; the
    /// tenant lays a list of that in this level's region and hops `Fresh` to the next such child.
    /// After the last, it reports this level's reach steps rather than a branch length.
    Cycle,
}

/// The list of the handed type, interned through `step`'s chain and handed on in its place.
fn mint_list(step: &Step<'_, '_, '_, '_, '_, '_, Native>) {
    let handed = HANDED.with(Cell::get);
    let minted = step.types().list(handed);
    HANDED.with(|slot| slot.set(minted));
}

/// How many reach steps the merges into `step`'s write home's branch have taken, or 0 where it has
/// none.
fn reach_steps(step: &Step<'_, '_, '_, '_, '_, '_, Native>) -> usize {
    step.header()
        .get()
        .and_then(|header| header.branch())
        .map_or(0, |branch| branch.reach_steps())
}

/// The length of `step`'s write home's branch, or 0 where it has none.
fn branch_length(step: &Step<'_, '_, '_, '_, '_, '_, Native>) -> usize {
    step.header()
        .get()
        .and_then(|header| header.branch())
        .map_or(0, |branch| branch.len())
}

/// Run `start` as a root work over a slab of `cells`, starting from `Number`, and hand back what
/// it recorded.
fn run(cells: u32, start: AnyStep) -> Vec<String> {
    reset();
    HANDED.with(|handed| handed.set(KType::NUMBER));
    let type_region = Bump::new();
    let mut graph: TestGraph<'_> =
        TestGraph::new(cells, crate::values::tests::seeded_tiers(&type_region));
    let root = graph.root().expect("a fresh slab admits a root");
    Scheduler::over(&mut graph)
        .run(work(start, KValue::Null), root, Placement::Fresh)
        .expect("the root work ends");
    graph.release_root(root).expect("the root releases");
    assert!(graph.is_empty(), "the run leaves no cell behind");
    recorded()
}

/// The spawner: mint `List^1` through `List^n`, hand the last to a `Fresh` child, park on it.
fn spawner<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    for _ in 0..MINTED.with(Cell::get) {
        mint_list(&step);
    }
    let asked = step.spawn(fresh(reader, Use::Reads, KValue::Null));
    step.park(asked, woken, KValue::Null, None)
}

/// The child: read the handed type, then report its branch's length.
fn reader<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let _ = step.types().node(HANDED.with(Cell::get));
    record(branch_length(&step).to_string());
    step.finish_fresh(|_, _, _| Active::new(KValue::Null))
}

/// A root work, woken by the child it parked on: take the result and end.
fn woken<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let _ = step.results().count();
    step.done()
}

#[test]
fn a_child_links_its_spawners_branch() {
    // Under Miri a spawner holding one type and one holding two are enough to read the link.
    let counts = if cfg!(miri) { [1, 2] } else { [8, 256] };
    for minted in counts {
        MINTED.with(|count| count.set(minted));
        assert_eq!(
            run(2, spawner),
            ["1"],
            "a child born under a spawner holding {minted} types holds only the one it read"
        );
    }
}

/// A child level of the recursion, which delivers to the level above it.
fn level<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    descend(step, relay)
}

/// The recursion's first level, a root work, which ends with nothing to deliver.
fn first<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    descend(step, woken)
}

/// Mint the list of the level above's type, then either spawn the next level `Fresh` and park on
/// it, to wake at `wake`, or, at the bottom, report the branch's length or [`hand_down`]. Each
/// level reads the type it lists once, through its link, so its branch holds that type and its
/// own.
fn descend<'graph>(
    mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>,
    wake: TestStep<'graph>,
) -> Action<'graph, Native> {
    let at = LEVEL.with(|level| level.replace(level.get() + 1));
    mint_list(&step);
    if at + 1 == DEPTH.with(Cell::get) {
        if let Some(shape) = SHAPE.with(Cell::get) {
            return hand_down(step, wake, shape);
        }
        record(branch_length(&step).to_string());
        return step.finish_fresh(|_, _, _| Active::new(KValue::Null));
    }
    let asked = step.spawn(fresh(level, Use::Reads, KValue::Null));
    step.park(asked, wake, KValue::Null, None)
}

/// A parked level, woken by the level below it: take its result and deliver one of its own.
fn relay<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let _ = step.results().count();
    step.finish_fresh(|_, _, _| Active::new(KValue::Null))
}

/// What the deepest level of a minting recursion `depth` levels deep, one cell per level, reports.
fn deepest(depth: u32) -> Vec<String> {
    SHAPE.with(|shape| shape.set(None));
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
fn nests<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let types = step.types();
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
fn lists<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let (mut step, state) = step.state();
    let types = step.types();
    let list = crate::values::List::new(step.writer(), [state].into_iter(), &types, &Bump::new());
    let asked = step.spawn(shares(hops, Use::Reads, KValue::List(list)));
    step.park(asked, relay, KValue::Null, None)
}

/// The deepest level: lay a list of the type it minted in its own region and hand it on as
/// `shape` says, then park, to wake at `wake`, or, for a kept result, to report its own branch.
fn hand_down<'graph>(
    mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>,
    wake: TestStep<'graph>,
    shape: Shape,
) -> Action<'graph, Native> {
    let types = step.types();
    let element = KValue::List(crate::values::List::new(
        step.writer(),
        std::iter::empty(),
        &types,
        &Bump::new(),
    ));
    let list = KValue::List(crate::values::List::new(
        step.writer(),
        [element].into_iter(),
        &types,
        &Bump::new(),
    ));
    let (request, wake) = match shape {
        Shape::TenantFresh => (shares(hop_fresh, Use::Reads, list), wake),
        Shape::TreeFresh => (fresh(hop_fresh, Use::Reads, list), wake),
        Shape::TreeFreshCopied => (fresh(wrap_and_hop_fresh, Use::Reads, list), wake),
        Shape::TreeShares => (fresh(wrap_and_hop_shares, Use::Reads, list), wake),
        Shape::TenantShares => (shares(wrap_and_hop_shares, Use::Reads, list), wake),
        // The deepest level is never the root work, so its own wake is `relay`'s.
        Shape::KeptResult => (
            fresh(hand_back, Use::Keeps, list),
            relay_measuring as TestStep<'graph>,
        ),
        Shape::Cycle => (fresh(cycle_frame, Use::Reads, list), wake),
    };
    let asked = step.spawn(request);
    step.park(asked, wake, KValue::Null, None)
}

/// Hop `Fresh` with the state to [`measures`].
fn hop_fresh<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let (step, state) = step.state();
    step.tail(
        Placement::Fresh,
        Work {
            step: measures,
            state,
        },
    )
}

/// Lay a list of the state and hop with it as `placement` says, to `next`.
fn wrap_and_tail<'graph>(
    step: Step<'_, 'graph, '_, '_, '_, '_, Native>,
    placement: Placement,
    next: TestStep<'graph>,
) -> Action<'graph, Native> {
    let (step, state) = step.state();
    let types = step.types();
    let list = crate::values::List::new(step.writer(), [state].into_iter(), &types, &Bump::new());
    let state = KValue::List(list);
    step.tail(placement, Work { step: next, state })
}

/// Lay a list of the state and hop `Fresh` with it to [`measures`].
fn wrap_and_hop_fresh<'graph>(
    step: Step<'_, 'graph, '_, '_, '_, '_, Native>,
) -> Action<'graph, Native> {
    wrap_and_tail(step, Placement::Fresh, measures)
}

/// Lay a list of the state and hop `Shares` with it to [`measures`].
fn wrap_and_hop_shares<'graph>(
    step: Step<'_, 'graph, '_, '_, '_, '_, Native>,
) -> Action<'graph, Native> {
    wrap_and_tail(step, Placement::Shares, measures)
}

/// A cycle's frame: lay a list of the state and hop `Shares` to a tenant of the level, whose
/// branch the hop merges this frame's into.
fn cycle_frame<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    FRAMES.with(|frames| frames.set(frames.get() + 1));
    wrap_and_tail(step, Placement::Shares, cycle_tenant)
}

/// A cycle's tenant of the level: lay a list of the state in the level's region and hop `Fresh`
/// to the next frame, whose birth links the level's branch; after the last frame, report the
/// level's reach steps before the result crosses back.
fn cycle_tenant<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    if FRAMES.with(Cell::get) < CYCLE_FRAMES {
        return wrap_and_tail(step, Placement::Fresh, cycle_frame);
    }
    record(reach_steps(&step).to_string());
    step.finish_fresh(|_, _, _| Active::new(KValue::Null))
}

/// The hop's successor: report its branch's length.
fn measures<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    record(branch_length(&step).to_string());
    step.finish_fresh(|_, _, _| Active::new(KValue::Null))
}

/// Hand the state back up as a kept result.
fn hand_back<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let (step, state) = step.state();
    step.finish(state)
}

/// [`relay`], reporting the branch's length once the kept result has crossed into it.
fn relay_measuring<'graph>(
    mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>,
) -> Action<'graph, Native> {
    let _ = step.results().count();
    record(branch_length(&step).to_string());
    step.finish_fresh(|_, _, _| Active::new(KValue::Null))
}

/// What the cell a list lands in reports when the deepest level of a recursion `depth` levels deep
/// hands it on as `shape` says.
fn handed_under(depth: u32, shape: Shape) -> Vec<String> {
    assert!(depth >= 2, "the deepest level is not the root work");
    SHAPE.with(|slot| slot.set(Some(shape)));
    LEVEL.with(|level| level.set(0));
    DEPTH.with(|limit| limit.set(depth as usize));
    FRAMES.with(|frames| frames.set(0));
    run(depth, first)
}

#[test]
fn each_way_a_deep_cell_hands_a_value_on_costs_the_same_however_deep() {
    let (shallow, deep) = if cfg!(miri) { (2, 3) } else { (4, 16) };
    for shape in [
        Shape::TenantFresh,
        Shape::TreeFresh,
        Shape::TreeFreshCopied,
        Shape::TreeShares,
        Shape::TenantShares,
        Shape::KeptResult,
    ] {
        let near = handed_under(shallow, shape);
        assert_eq!(near.len(), 1, "{shape:?}: the landing cell reported once");
        assert_eq!(
            handed_under(deep, shape),
            near,
            "{shape:?}: the landing cell's branch holds the same at depth {shallow} and {deep}"
        );
    }
}

/// Each frame's `Shares` hop merges the frame's branch, which links the level's, into the level's:
/// two reach steps, at any depth. Each tenant's list lies in the level's region, so the next
/// frame's birth links the level's branch and merges nothing.
#[test]
fn a_tail_cycle_checks_as_many_steps_however_deep_its_host() {
    let (shallow, deep) = if cfg!(miri) { (2, 3) } else { (4, 16) };
    for depth in [shallow, deep] {
        assert_eq!(
            handed_under(depth, Shape::Cycle),
            [(2 * CYCLE_FRAMES).to_string()],
            "a cycle under a host {depth} levels deep"
        );
    }
}

/// The tenant: hop `Fresh` with the list. The successor is a new cell, which has laid no branch,
/// and the list's home, the tenant's host, outlives it, so the birth links the host's branch.
fn hops<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    let (step, state) = step.state();
    let work = Work {
        step: arrives,
        state,
    };
    step.tail(Placement::Fresh, work)
}

/// The hop's successor, whose birth passed the habitat check.
fn arrives<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    record(String::from("arrived"));
    step.finish_fresh(|_, _, _| Active::new(KValue::Null))
}

#[test]
fn a_hop_reaches_what_its_births_home_read_through_its_link() {
    assert_eq!(run(4, nests), ["arrived"]);
}

/// The root work for a caching chain's test: ask `child` in the given placement, park on it.
fn asking<'graph>(
    mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>,
    child: TestStep<'graph>,
    fresh_child: bool,
) -> Action<'graph, Native> {
    let request = if fresh_child {
        fresh(child, Use::Reads, KValue::Null)
    } else {
        shares(child, Use::Reads, KValue::Null)
    };
    let asked = step.spawn(request);
    step.park(asked, woken, KValue::Null, None)
}

/// A `Shares` tenant interning through the `Shares` chain: the host's region branch before and
/// after, and the scratch branch's length after.
fn shares_minting<'graph>(
    step: Step<'_, 'graph, '_, '_, '_, '_, Native>,
) -> Action<'graph, Native> {
    let before = branch_length(&step);
    let chain = step.caching_types();
    let _ = chain.list(chain.list(chain.list(KType::NUMBER)));
    record(format!(
        "{before} {} {}",
        branch_length(&step),
        chain.own_len()
    ));
    step.finish_fresh(|_, _, _| Active::new(KValue::Null))
}

#[test]
fn a_shares_chain_lays_in_the_hosts_scratch_and_leaves_its_region() {
    fn start<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
        asking(step, shares_minting, false)
    }
    let seen = run(2, start);
    let [before, after, scratch] = seen[0]
        .split(' ')
        .map(|n| n.parse::<usize>().unwrap())
        .collect::<Vec<_>>()[..]
    else {
        panic!("three figures");
    };
    assert_eq!(before, after, "the region branch is untouched");
    assert!(scratch > 0, "the lists were laid in the scratch branch");
}

/// A `Fresh` child holding its scratch and running on its caching chain: it borrows no
/// specialization, so it has no agent chain, and a type in its region and one in its own scratch
/// both resolve, a list of each laid in its scratch while the region does not grow.
fn reading<'graph>(mut step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
    assert!(step.hold_scratch(), "a tree cell holds its own scratch");
    assert!(
        step.agent_types().is_none(),
        "a frame with no specialization has no agent chain"
    );
    let in_region = step.types().list(step.types().list(KType::NUMBER));
    let in_scratch = step.scratch_types().list(KType::BOOL);
    let region = branch_length(&step);
    let chain = step.caching_types();
    let before = chain.own_len();
    let _ = chain.list(in_region);
    let _ = chain.list(in_scratch);
    assert_eq!(
        branch_length(&step),
        region,
        "the region branch is untouched"
    );
    record((chain.own_len() - before).to_string());
    step.finish_fresh(|_, _, _| Active::new(KValue::Null))
}

#[test]
fn a_held_fresh_child_reads_its_region_and_its_own_scratch() {
    fn start<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, Native>) -> Action<'graph, Native> {
        asking(step, reading, true)
    }
    let laid: usize = run(2, start)[0].parse().unwrap();
    assert_eq!(
        laid, 2,
        "the caching chain laid both lists in its own scratch"
    );
}
