//! The drain: the loop that runs one root work to its end over a ready stack, and the graph it runs
//! over.
//!
//! The graph holds the run cells and, beside them, the [build arena](super::arena) of the
//! settlements frames run under; no settlement is a cell. Before a cell is born the drain resolves
//! the arena node it runs under: for a call's frame, what its asker's [`Seek`] finds or builds,
//! and for any other cell, what its asker runs under. The drain borrows that arena node for the
//! cell's life, from its birth to its release, so a held settlement is never evicted, and a tail
//! hop's successor borrows its frame's before the frame is released. Between steps it opens no
//! cell but to run a step: a `Fresh` cell borrows its parent's scratch and a `Shares` one is its
//! host's tenant.
//!
//! A tail hop whose birth was laid as a skeleton beside the values it hands on wakes each value on
//! its own, from the kept part it was handed through where that redeems, and builds the
//! successor's birth in the successor's region over them ([`StepBundle::handings`]): nothing
//! homed above the successor is copied, and nothing of the skeleton outlives its predecessor. A
//! spawned child pins its birth whole and keeps each value it is handed from its kept part, or out
//! of the birth where it has none ([`StepBundle::keeping`]).
//!
//! A loop whose hops hand on values their frames built — their threaded set's — runs its hops
//! under a **loop cell**: a tree cell under the loop's receiving cell, with no continuation, created
//! at the first hop that threads, where those values are built and copied to, which every hop after
//! is born under and so pins free. A hop compacts it into a fresh one once it holds twice its live
//! bytes, and the loop's last cell releases it when it delivers.
//!
//! A miss relays its binding's image into the agent tier, settles the body through the door the
//! program handed the graph at load ([`Settles`]), and, once the frame borrows the settlement,
//! evicts unheld settlements to the budget; nothing else evicts in a run. A step's door reads the
//! settlement its cell runs under over the load.
//!
//! Every field here is one scheduler's own. There is no static, no thread-local and no lazily
//! minted cell, so a second scheduler runs beside the first with nothing shared but the program
//! text and the shapes in it.

use smallvec::SmallVec;

use crate::knot::KValueFamily;
use crate::memory::{
    Active, CellGraph, CellHandle, CreateError, CrossedOperand, Dormant, Erased, FIRST_CHUNK, Lend,
    Operand, Ready, RedeemError, ReleaseAbsorption, ReleaseError, SlabHandle,
};
use crate::scheduler::action::{
    Action, Asked, Context, Kind, Placement, Spawns, Step, StepError, Tailing, Use,
};
use crate::scheduler::arena::{BuildArena, Composed, Settled};
use crate::scheduler::continuation::{
    Arrival, BirthAt, Born, Continuation, ContinuationFamily, NativeStep, Provenance, Report,
    Rested, StateAt, StepBundle, Work,
};
use crate::scheduler::delivery::KDelivery;
use crate::scheduler::kept::Kept;
use crate::scheduler::seek::{Compose, Lending, Replayed, Requested, Seek, Settles, Unsettled};
#[cfg(test)]
use crate::scope::HalfHeader;
use crate::scope::{Facts, LoadFacts, RegionHeader};
#[cfg(test)]
use crate::type_lattice::Branch;
use crate::type_lattice::Halves;
use crate::values::{ArenaRef, BuildBudget, ContentDigest, Half, TypeTiers};

/// What a loop cell may hold past twice its live bytes before a hop compacts it: a region's first
/// chunk, within which growth is noise.
const COMPACTION_FLOOR: usize = FIRST_CHUNK;

/// The graph of cells a drain runs over: `cellgraph`'s graph closed over the continuation family,
/// the bundle's scratch family and the delivery bundle.
///
/// A newtype rather than an alias, so the families stay the scheduler's own: what owns a graph
/// makes one, takes its roots from it, keeps it across calls and asks what is left in it, and
/// reaches every other cell only through a [`Scheduler`] over it.
///
/// It holds the type tiers every step's chain reads first, so a step and a crossing take it
/// from the graph they run over rather than from a program or a static, the load's facts every
/// step's door reads beneath its cell's layers, and the build arena its frames run under.
pub struct Graph<'graph, B: StepBundle<'graph>> {
    cells: CellGraph<'graph, ContinuationFamily<B>, B::Scratch, KDelivery, RegionHeader>,
    tiers: TypeTiers<'graph>,
    facts: &'graph LoadFacts<'graph>,
    arena: BuildArena<'graph>,
    /// The door a miss settles through.
    settles: &'graph dyn Settles<'graph>,
}

impl<'graph, B: StepBundle<'graph>> Graph<'graph, B> {
    /// A graph over a slab of `cap` cells, under koan's own crossing verdict, its steps' types read
    /// over `tiers`, its unheld settlements within `budget`, over a load of no bodies.
    pub fn new(cap: u32, tiers: TypeTiers<'graph>, budget: BuildBudget) -> Self {
        Graph::over(cap, tiers, (&LoadFacts::EMPTY, &Unsettled), budget)
    }

    /// A graph as [`new`](Self::new) stands one, its steps' facts read over `facts`, its misses
    /// settled through `settles`.
    pub(super) fn over(
        cap: u32,
        tiers: TypeTiers<'graph>,
        (facts, settles): (&'graph LoadFacts<'graph>, &'graph dyn Settles<'graph>),
        budget: BuildBudget,
    ) -> Self {
        Graph {
            cells: CellGraph::new(cap, crate::values::verdict),
            tiers,
            facts,
            arena: BuildArena::new(budget),
            settles,
        }
    }

    /// A storage-only slab cell no drain enters or releases: the region a root work is born under
    /// and builds its results in. Refused when the slab is full.
    pub fn root(&mut self) -> Result<SlabHandle, CreateError> {
        self.cells.create(None)
    }

    /// Give a root back. Refused for a root that is not live.
    pub fn release_root(&mut self, root: SlabHandle) -> Result<(), ReleaseError> {
        self.cells.release(root, ReleaseAbsorption::IntoHolder)
    }

    /// Whether every cell of this graph, roots included, has been reclaimed.
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Whether the named cell is still live — what a test reads to watch a release land.
    pub fn is_live(&self, cell: impl Into<CellHandle>) -> bool {
        self.cells.is_live(cell)
    }

    /// How many settlements are live.
    #[cfg(test)]
    pub(crate) fn live_settlements(&self) -> usize {
        self.arena.len()
    }

    /// How many compositions are live.
    #[cfg(test)]
    pub(crate) fn live_compositions(&self) -> usize {
        self.arena.compositions()
    }

    /// How many live settlements lie directly under a composition, and how many live compositions
    /// some settlement pins.
    #[cfg(test)]
    pub(crate) fn under_compositions(&self) -> (usize, usize) {
        self.arena.under_compositions()
    }

    /// How many live settlements lie directly under `parent`.
    #[cfg(test)]
    pub(crate) fn children_of(&self, parent: ArenaRef) -> usize {
        self.arena.children_of(parent)
    }

    /// Whether `at` names a live settlement.
    #[cfg(test)]
    pub(crate) fn is_settled(&self, at: ArenaRef) -> bool {
        at != ArenaRef::ROOT && self.arena.is_live(at)
    }

    /// What the live settlement `at` weighs against the budget: what a test that forces eviction
    /// sizes its budget by.
    #[cfg(test)]
    pub(crate) fn settlement_bytes(&self, at: ArenaRef) -> Option<usize> {
        self.arena.bytes_of(at)
    }

    /// How many run cells borrow the live settlement `at`.
    #[cfg(test)]
    pub(crate) fn borrowers(&self, at: ArenaRef) -> Option<usize> {
        self.arena.borrowers(at)
    }

    /// The bytes of live settlements no run cell holds.
    #[cfg(test)]
    pub(crate) fn unheld_bytes(&self) -> usize {
        self.arena.unheld_bytes()
    }

    /// The graph itself, for a test that drives the raw slot doors a step never reaches.
    #[cfg(test)]
    pub(super) fn cells(
        &mut self,
    ) -> &mut CellGraph<'graph, ContinuationFamily<B>, B::Scratch, KDelivery, RegionHeader> {
        &mut self.cells
    }
}

/// The most any step has left in each of its cell's type tiers, and how many frames sought and
/// built a settlement: what a test reads to see a loop's tiers hold flat across hops and its
/// settlement found once.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Tiers {
    /// Nodes in the write home's region branch.
    pub region: usize,
    /// Nodes in the scratch branch.
    pub scratch: usize,
    /// Nodes in the agent tier.
    pub agent: usize,
    /// Verdicts recorded in the agent tier.
    pub agent_verdicts: usize,
    /// Bytes the scratch bump has handed out.
    pub scratch_bytes: usize,
    /// How many frames sought a settlement: a count, not a high-water mark.
    pub sought: usize,
    /// How many of those built one on a miss: a count.
    pub settled: usize,
    /// How many settlements a miss evicted: a count.
    pub evicted: usize,
    /// How many frames were born cold because the arena node their seek or hop named is dead: a
    /// count.
    pub cold_births: usize,
    /// Nodes the drain laid into the agent tier from new settlements' askers' branches: a count.
    pub relayed: usize,
    /// Nodes new settlements' records named that their walks laid into the agent tier: a count.
    pub rested: usize,
    /// Nodes run steps laid into the agent tier, waking their state included: a count, which a
    /// settlement's resting leaves at nothing.
    pub stepped: usize,
    /// How many compositions a miss built: a count.
    pub composed: usize,
    /// How many cells running composed code bounced with its syntax, their composition not live
    /// at their birth: a count.
    pub bounced: usize,
    /// The most bytes of live settlements and compositions no run cell held once a miss had
    /// evicted: a high-water mark.
    pub unheld: usize,
    /// The most settlements live at once at a birth: a high-water mark.
    pub live: usize,
}

/// Add to one of the counts in [`TIERS`].
#[cfg(test)]
fn tally(count: impl FnOnce(&mut Tiers)) {
    TIERS.with(|tiers| {
        let mut now = tiers.get();
        count(&mut now);
        tiers.set(now);
    });
}

#[cfg(test)]
thread_local! {
    /// Bytes run steps have written to their write homes' regions: a count. Tests reset it.
    pub(crate) static REGION_WRITTEN: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
thread_local! {
    /// The [`Tiers`] every step has left so far. Tests reset it.
    pub(crate) static TIERS: std::cell::Cell<Tiers> = const {
        std::cell::Cell::new(Tiers {
            region: 0,
            scratch: 0,
            agent: 0,
            agent_verdicts: 0,
            scratch_bytes: 0,
            sought: 0,
            settled: 0,
            evicted: 0,
            cold_births: 0,
            relayed: 0,
            rested: 0,
            stepped: 0,
            composed: 0,
            bounced: 0,
            unheld: 0,
            live: 0,
        })
    };
}

/// What every region held at one tail, sampled by the drain before it creates the successor: what
/// a test reads to see a loop's memory hold flat across hops.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HopSample {
    /// The cell receiving the loop's result, which tells one loop's tails from another's.
    pub host: CellHandle,
    /// Bytes every region of the graph has handed out, absorbed bumps included.
    pub live: usize,
    /// Bytes the region of the cell receiving the loop's result has handed out.
    pub host_region: usize,
    /// Bytes that cell's scratch has handed out.
    pub host_scratch: usize,
    /// Bytes the tailing cell's own region has handed out; `0` for a tenant, which has none.
    pub hop_region: usize,
    /// The weight of the values the successor's birth threads.
    pub threads: usize,
}

#[cfg(test)]
thread_local! {
    /// Every [`HopSample`] taken so far, in tail order. Tests reset it.
    pub(crate) static HOPS: std::cell::RefCell<Vec<HopSample>> =
        const { std::cell::RefCell::new(Vec::new()) };
    /// How many hops' successors built their birth over the values a skeleton handed on.
    pub(crate) static SKELETONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
thread_local! {
    /// The caching body the running step runs, by its shape's address: set by the step, taken by
    /// the drain as the step returns.
    pub(crate) static CACHING: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };

    /// Per caching body, by its shape's address: the nodes its steps laid in the scratch branch,
    /// and in the agent tier. Tests reset it.
    pub(crate) static LAID: std::cell::RefCell<std::collections::BTreeMap<usize, (usize, usize)>> =
        const { std::cell::RefCell::new(std::collections::BTreeMap::new()) };
}

/// The drain: a ready stack and a request buffer, made per call over a graph it borrows.
///
/// A view dropped before its root work ended abandons what is on its stack: the graph keeps the
/// cells already born, under the root they were born under, and that root's release does not
/// empty the graph. After a successful [`run`](Scheduler::run) the view holds nothing.
pub struct Scheduler<'a, 'graph, B: StepBundle<'graph>> {
    graph: &'a mut Graph<'graph, B>,
    /// The ready stack: last in, first out, so the drain is depth-first. Its depth is the unborn
    /// siblings along the current path.
    ready: Vec<Entry<'graph, B>>,
    spawns: Spawns<'graph, B>,
    #[cfg(test)]
    census: Census,
}

/// Where the drain places a frame it seeks for: the arena node it runs under, whether placing it
/// missed, the composition its first step must rebuild, and the request its first step lays a
/// level from.
type Placing<'graph> = (
    ArenaRef,
    bool,
    Option<ContentDigest>,
    Option<Box<Requested<'graph>>>,
);

/// One entry of the ready stack.
#[expect(
    clippy::large_enum_variant,
    reason = "an unborn request carries what a miss settles; boxing it would allocate per call, \
              and the stack holds only the unborn siblings along one path"
)]
enum Entry<'graph, B: StepBundle<'graph>> {
    /// A live cell with a step to run: a woken consumer, or a root work.
    Live(CellHandle),
    /// A request not yet born: the provenance the drain filled for it, the request at rest, the
    /// arena node its asker runs under, the cell it is born under and how its birth crosses.
    /// `release_after` is a tail's predecessor, released with its borrow of that arena node once
    /// this cell's first step has returned — the successor wakes its state out of the
    /// predecessor's region, and borrows first. A frame released before the evaluation it handed
    /// off to is dead but undisposed until that child disposes.
    Unborn {
        provenance: Provenance,
        asked: Asked<'graph, B>,
        asker: ArenaRef,
        under: CellHandle,
        born: Born,
        release_after: Option<CellHandle>,
        /// A loop cell the hop compacted away from, released after `release_after`.
        release_loop: Option<CellHandle>,
    },
}

impl<'a, 'graph, B: StepBundle<'graph>> Scheduler<'a, 'graph, B>
where
    Erased<'graph, B::Birth>: Copy,
{
    /// A drain over `graph`.
    pub fn over(graph: &'a mut Graph<'graph, B>) -> Self {
        Scheduler {
            graph,
            ready: Vec::new(),
            spawns: Spawns::new(),
            #[cfg(test)]
            census: Census::default(),
        }
    }

    /// Run `work` to its end, born under `under` at `placement`, and hand back what it left at
    /// rest, if it ended by [`Step::leave`](crate::scheduler::Step::leave).
    ///
    /// Everything else the drain runs is a descendant the root work asked for. The root work
    /// reports to nobody, so its end is success and the stack emptying first is
    /// [`DrainStalled::Unfinished`]. A koan error is a tagged value and travels between cells as
    /// data, so no `Result` passes from one cell to another and these are the only errors the
    /// scheduler defines.
    pub fn run(
        &mut self,
        work: Work<'graph, 'graph, B>,
        under: impl Into<CellHandle>,
        placement: Placement,
    ) -> Result<Option<Resting<'graph, B>>, DrainStalled> {
        self.drive(
            work.step,
            Rested::Awake(B::born(work.state)),
            under.into(),
            placement,
        )
    }

    /// Run a later root work to its end, born under `under` at `placement` from what an earlier one
    /// left at rest there, and hand back what this one leaves. The birth wakes at the new cell's
    /// first entry at the verdict's price, exactly as a spawned child's does — free for a tenant of
    /// the cell it was left in, which crosses nothing.
    pub fn resume(
        &mut self,
        step: NativeStep<'graph, B>,
        resting: Resting<'graph, B>,
        under: impl Into<CellHandle>,
        placement: Placement,
    ) -> Result<Option<Resting<'graph, B>>, DrainStalled> {
        self.drive(
            step,
            Rested::Dormant(resting.0, Born::Child),
            under.into(),
            placement,
        )
    }

    /// The drain loop both root works share: born under `under`, reporting to nobody, run until
    /// it ends.
    fn drive(
        &mut self,
        step: NativeStep<'graph, B>,
        state: Rested<'graph, 'graph, B>,
        under: CellHandle,
        placement: Placement,
    ) -> Result<Option<Resting<'graph, B>>, DrainStalled> {
        // A view a stalled run left entries on starts over: they belong to a root work that will
        // never end, so nothing this run does may pop them.
        self.ready.clear();
        let root = self.born(
            under,
            placement,
            Continuation {
                step,
                provenance: Provenance {
                    parent: under,
                    report: None,
                    home: under,
                    loop_cell: None,
                    threads: false,
                    loop_base: 0,
                },
                arena: ArenaRef::ROOT,
                arrival: Arrival::Placed,
                sought: None,
                state,
            },
        )?;
        self.ready.push(Entry::Live(root));
        while let Some(entry) = self.ready.pop() {
            let (cell, release_after, release_loop) = match entry {
                Entry::Live(cell) => (cell, None, None),
                Entry::Unborn {
                    provenance,
                    asked,
                    asker,
                    under,
                    born,
                    release_after,
                    release_loop,
                } => {
                    let cell = self.create(provenance, asked, (under, born), asker)?;
                    let release_after = release_after.map(|predecessor| (predecessor, asker));
                    (cell, release_after, release_loop)
                }
            };
            let (action, provenance, arena) = self.step(cell)?;
            // A hop's successor has woken what its predecessor kept, and borrows its arena node,
            // so the predecessor can go — and the loop cell it compacted away from after it.
            if let Some((predecessor, arena)) = release_after {
                self.retire(predecessor, arena)?;
            }
            if let Some(old) = release_loop {
                self.release_loop(old)?;
            }
            match action.kind() {
                Kind::Finished { completed } => {
                    self.retire(cell, arena)?;
                    // The loop's last cell has delivered: its loop cell's bytes reached the home
                    // already, by a pledge or a copy.
                    if let Some(loop_cell) = provenance.loop_cell {
                        self.release_loop(loop_cell)?;
                    }
                    // The consumer comes off the drain's own copy of the provenance, read straight
                    // off the continuation, so no step can name the cell that wakes.
                    if provenance.report.is_none() {
                        debug_assert!(self.ready.is_empty(), "the root work ended last");
                        return Ok(None);
                    }
                    if completed {
                        self.ready.push(Entry::Live(provenance.parent));
                    }
                }
                Kind::Park => self.asked_by(cell, provenance, arena),
                Kind::Tail(asked, tailing) => {
                    let mut provenance = provenance;
                    let mut release_loop = None;
                    if let Tailing::Hop { threads } = tailing {
                        release_loop = self.loop_for(&mut provenance, threads)?;
                        #[cfg(test)]
                        self.sample_hop(cell, provenance.parent, threads);
                    }
                    // A hand-off is born under this cell; a hop or a rebirth beside it, under its
                    // loop cell where it has one.
                    let (under, born) = match tailing {
                        Tailing::HandOff => (cell, Born::Child),
                        Tailing::Hop { .. } | Tailing::Rebirth => {
                            (provenance.loop_cell.unwrap_or(provenance.parent), Born::Hop)
                        }
                    };
                    self.ready.push(Entry::Unborn {
                        provenance,
                        asked,
                        asker: arena,
                        under,
                        born,
                        release_after: Some(cell),
                        release_loop,
                    });
                }
                // Only a root work leaves: `leave` refuses a cell that reports to somebody.
                Kind::Left(dormant) => {
                    self.retire(cell, arena)?;
                    debug_assert!(self.ready.is_empty(), "the root work ended last");
                    return Ok(Some(Resting(dormant)));
                }
                Kind::Failed(error) => return Err(DrainStalled::Step(error)),
            }
        }
        Err(DrainStalled::Unfinished)
    }

    /// The loop cell a hop's successor is born under, set on `provenance`: a new one under the
    /// loop's receiving cell at the first hop that threads, and a fresh one beside the current one
    /// once its bytes pass twice its live bytes, or twice what the hop threads — a compaction, whose
    /// threaded values the successor copies into the fresh cell — handing back the one it replaced
    /// to release after the successor's first step. A loop cell's live bytes are read at the first
    /// hop after it was filled, so its values' types weigh in as their values do; amortized, each
    /// hop copies a constant.
    fn loop_for(
        &mut self,
        provenance: &mut Provenance,
        threads: Option<usize>,
    ) -> Result<Option<CellHandle>, DrainStalled> {
        let fresh = match (provenance.loop_cell, threads) {
            (None, None) => return Ok(None),
            (None, Some(_)) => None,
            (Some(loop_cell), threads) => {
                let used = (self.graph.cells.region_used(loop_cell))
                    .map_err(|_| DrainStalled::Unreleasable)?;
                if provenance.loop_base == 0 {
                    provenance.loop_base = used.max(1);
                    return Ok(None);
                }
                let live = provenance.loop_base.max(threads.unwrap_or(0));
                if used <= 2 * live + COMPACTION_FLOOR {
                    return Ok(None);
                }
                Some(loop_cell)
            }
        };
        let created = (self.graph.cells)
            .create_tree_lent(provenance.parent, Lend::ParentScratch, None)
            .map_err(|_| DrainStalled::Unspawnable)?;
        #[cfg(test)]
        self.census.born();
        provenance.loop_cell = Some(created.into());
        provenance.loop_base = 0;
        Ok(fresh)
    }

    /// Release a loop cell: it borrows no arena node, and runs nothing.
    fn release_loop(&mut self, loop_cell: CellHandle) -> Result<(), DrainStalled> {
        let CellHandle::Tree(handle) = loop_cell else {
            return Err(DrainStalled::Unreleasable);
        };
        (self.graph.cells.release_tree(handle)).map_err(|_| DrainStalled::Unreleasable)?;
        #[cfg(test)]
        self.census.retired();
        Ok(())
    }

    /// Push every child the parked step asked for, so the first asked is on top. The cell itself
    /// is left alone: it is live, parked on the run it registered, and wakes when the last slot
    /// fills.
    ///
    /// A child is born under the cell that asked for it and reports to the slot it was pushed at,
    /// so no step can name a destination that is not its spawner's; its home is the spawner, or
    /// under `Forwards` the spawner's own home. `arena` is what the spawner runs under.
    fn asked_by(&mut self, cell: CellHandle, provenance: Provenance, arena: ArenaRef) {
        let Scheduler { ready, spawns, .. } = self;
        for (slot, asked) in spawns.take().rev() {
            let (home, threads) = match asked.use_ {
                Use::Forwards => (provenance.home, provenance.threads),
                Use::Threads => match provenance.loop_cell {
                    Some(loop_cell) => (loop_cell, true),
                    None => (cell, false),
                },
                Use::Reads | Use::Keeps => (cell, false),
            };
            ready.push(Entry::Unborn {
                provenance: Provenance {
                    parent: cell,
                    report: Some(Report {
                        slot,
                        use_: asked.use_,
                    }),
                    home,
                    loop_cell: None,
                    threads,
                    loop_base: 0,
                },
                asked,
                asker: arena,
                under: cell,
                born: Born::Child,
                release_after: None,
                release_loop: None,
            });
        }
    }

    /// Give a request its cell, under `parent`, its birth crossing as `born` says, running under
    /// the arena node its lending resolves to, which it borrows. `asker` is the arena node the cell
    /// that asked runs under. A miss evicts to the budget once the new cell borrows its settlement.
    fn create(
        &mut self,
        provenance: Provenance,
        asked: Asked<'graph, B>,
        (parent, born): (CellHandle, Born),
        asker: ArenaRef,
    ) -> Result<CellHandle, DrainStalled> {
        let arena = &mut self.graph.arena;
        let mut replayed = SmallVec::<[ArenaRef; 4]>::new();
        let replaying = matches!(asked.lending, Lending::Replay(_));
        let (under, missed, wanting, sought) = match asked.lending {
            Lending::Parent => (asker, false, None, None),
            Lending::Call(seek) => self.seek(seek),
            Lending::Replay(levels) => self.replay(levels, asker, &mut replayed),
            Lending::Pinning(seek, syntax) => {
                let (_, built) = self.composed(Compose::Owned(syntax));
                let (under, missed, wanting, sought) = self.seek(seek);
                (under, missed || built, wanting, sought)
            }
            Lending::Under(under) if arena.is_live(under) => {
                arena.stamp(under);
                (under, false, None, None)
            }
            Lending::Under(_) => {
                #[cfg(test)]
                tally(|tiers| tiers.cold_births += 1);
                (ArenaRef::ROOT, false, None, None)
            }
            Lending::Compose(compose) => {
                let (under, missed) = self.composed(compose);
                (under, missed, None, None)
            }
        };
        self.graph.arena.borrow(under);
        // A replay's levels are held until the frame borrows the last: its chain holds them since.
        for level in replayed.iter().rev() {
            self.graph.arena.release(*level);
        }
        if missed {
            let _evicted = self.graph.arena.evict_to_budget();
            #[cfg(test)]
            {
                let unheld = self.graph.arena.unheld_bytes();
                tally(|tiers| {
                    tiers.evicted += _evicted;
                    tiers.unheld = tiers.unheld.max(unheld);
                });
            }
        }
        #[cfg(test)]
        {
            let live = self.graph.arena.len();
            tally(|tiers| tiers.live = tiers.live.max(live));
        }
        let arrival = match wanting {
            Some(digest) => Arrival::Wanting(digest),
            None if replaying => Arrival::Replayed,
            None => Arrival::Placed,
        };
        self.born(
            parent,
            asked.placement,
            Continuation {
                step: asked.step,
                provenance,
                arena: under,
                arrival,
                sought,
                state: Rested::Dormant(asked.state, born),
            },
        )
    }

    /// The arena node a frame born by a replay runs under, whether any level missed, the
    /// composition its first step must rebuild, and the request its first step lays a level from:
    /// each of `levels` found under the one before, or settled there, from `base`, the asker's —
    /// its pins found or built first — and borrowed into `held` until the frame borrows its own;
    /// the last is the frame's own seek. A level that settles nothing ends the replay, and the
    /// frame is born where it stopped, with no request.
    fn replay(
        &mut self,
        levels: Vec<Replayed<'graph>>,
        base: ArenaRef,
        held: &mut SmallVec<[ArenaRef; 4]>,
    ) -> Placing<'graph> {
        // The frame was counted cold where its own seek found its parent gone; it is born warm.
        #[cfg(test)]
        if levels
            .last()
            .is_some_and(|last| last.seek.settlement.is_some())
        {
            tally(|tiers| tiers.cold_births = tiers.cold_births.saturating_sub(1));
        }
        let (mut parent, mut missed) = (base, false);
        let count = levels.len();
        for (at, Replayed { seek, pins }) in levels.into_iter().enumerate() {
            for syntax in pins {
                missed |= self.composed(Compose::Owned(syntax)).1;
            }
            let (under, miss, wanting, sought) = self.seek(Seek { parent, ..seek });
            missed |= miss;
            if at + 1 == count {
                return (under, missed, wanting, sought);
            }
            if wanting.is_some() || under == parent {
                return (under, missed, wanting, None);
            }
            self.graph.arena.borrow(under);
            held.push(under);
            parent = under;
        }
        (parent, missed, None, None)
    }

    /// The arena node a frame `seek` asks for runs under, whether it was a miss, the composition
    /// its first step must rebuild, and — where the seek hands it back and the frame runs under the
    /// settlement it sought — the request its first step lays a level from: the parent itself for a
    /// seek with no key; else the settlement under the parent the key names, stamped, or built —
    /// its binding's image relayed into the agent tier under the root, nothing relayed under a
    /// composition. The root, cold, for a parent that is no longer live. A miss under a composition
    /// binding a module of another composed code pins that code's composition, where no layer below
    /// holds the module's tree; where that composition is gone, the frame is born under the parent
    /// wanting it rebuilt.
    fn seek(&mut self, seek: Seek<'graph>) -> Placing<'graph> {
        let Seek {
            parent,
            settlement,
            hands_back,
        } = seek;
        let Some((image, sought)) = settlement else {
            // A callee of composed code whose composition is gone is born under the root, and
            // bounces with its syntax.
            return match self.graph.arena.is_live(parent) {
                true => (parent, false, None, None),
                false => (ArenaRef::ROOT, false, None, None),
            };
        };
        let key = sought.key;
        #[cfg(test)]
        tally(|tiers| tiers.sought += 1);
        let arena = &mut self.graph.arena;
        if !arena.is_live(parent) {
            #[cfg(test)]
            tally(|tiers| tiers.cold_births += 1);
            return (ArenaRef::ROOT, false, None, None);
        }
        let handed = |image, sought| hands_back.then(|| Box::new(Requested { sought, image }));
        let door = arena.door(parent);
        let base = door.composition.map(|(digest, _)| digest);
        if let Some(found) = arena.find(parent, key) {
            #[cfg(debug_assertions)]
            assert!(
                base.is_some() || self.holds_binding(&image),
                "a hit's binding rests: its key digests the content its types are read from"
            );
            self.graph.arena.stamp(found);
            return (found, false, None, handed(image, sought));
        }
        // Under a composition, each module of other composed code no layer below holds the tree
        // of pins its composition.
        let mut pins = SmallVec::new();
        if let Some(base) = base {
            let below = match door.composition {
                Some((_, composition)) => Facts::composed(composition, door.top),
                None => Facts::under(self.graph.facts, door.top),
            };
            for asked in &sought.ask.bound {
                let super::seek::Asked::Module {
                    content,
                    composition: Some(composed),
                    ..
                } = *asked
                else {
                    continue;
                };
                if composed == base
                    || below.tree_of(content).is_some()
                    || self.graph.facts.tree_of(content).is_some()
                {
                    continue;
                }
                match arena.composition(composed) {
                    Some(at) => {
                        if !pins.contains(&at) {
                            pins.push(at);
                        }
                    }
                    None => return (parent, false, Some(composed), None),
                }
            }
        }
        let pinned = arena.pinned(&pins);
        // The binding's types rest once it resolves, before its walk reads them: in the agent tier
        // under the root, and in the walk's own chain under a composition.
        let resting: crate::values::Chain<'graph, '_, 'graph, '_> = self.graph.tiers.resting();
        let before = resting.own_len();
        let mut relayed = before;
        let mut rest = |walk: Option<&crate::values::Chain<'_, '_, '_, '_>>| match (base, walk) {
            (None, _) => {
                image.relay(&resting);
                relayed = resting.own_len();
            }
            (Some(_), Some(walk)) => image.relay(walk),
            (Some(_), None) => {}
        };
        let settles = self.graph.settles;
        let built = Settled::build(
            arena.under(parent),
            pinned,
            self.graph.facts,
            |bump, below, pinned, witness| {
                settles.settle(bump, below, pinned, &sought.ask, &mut rest, witness)
            },
        );
        let Some(facts) = built else {
            debug_assert_eq!(relayed, before, "nothing rests for a settlement not built");
            return (parent, false, None, None);
        };
        let settled = self.graph.arena.insert(parent, key, facts, pins);
        #[cfg(test)]
        tally(|tiers| {
            tiers.settled += 1;
            tiers.relayed += relayed - before;
            tiers.rested += resting.own_len() - relayed;
        });
        (settled, true, None, handed(image, sought))
    }

    /// The composition `compose` asks for, and whether it was a miss: found by its syntax digest
    /// and stamped; built through the program's door over the syntax a bounce copied; or, with no
    /// syntax in hand, the root, where the cell's first step bounces.
    fn composed(&mut self, compose: Compose) -> (ArenaRef, bool) {
        let digest = match &compose {
            Compose::Find(digest) => *digest,
            Compose::Owned(syntax) => {
                #[cfg(test)]
                tally(|tiers| tiers.bounced += 1);
                syntax.digest()
            }
        };
        if let Some(found) = self.graph.arena.composition(digest) {
            self.graph.arena.stamp(found);
            return (found, false);
        }
        let Compose::Owned(syntax) = compose else {
            return (ArenaRef::ROOT, false);
        };
        let settles = self.graph.settles;
        let facts = Composed::build(syntax, |syntax, bump, witness| {
            settles.compose(syntax, bump, witness)
        });
        #[cfg(test)]
        tally(|tiers| tiers.composed += 1);
        (self.graph.arena.compose(digest, facts), true)
    }

    /// Whether the frozen load or the agent tier holds the node of every handle `image` carries,
    /// as the miss that built its settlement relayed them. Lays nothing.
    #[cfg(debug_assertions)]
    fn holds_binding(&self, image: &crate::values::BindingImage<'_>) -> bool {
        let resting: crate::values::Chain<'graph, '_, 'graph, '_> = self.graph.tiers.resting();
        image.handles().all(|kt| resting.resolves(kt))
    }

    /// Enter one cell, wake its state, and run the step its continuation names. Hands back the
    /// provenance and arena node it read off the continuation.
    fn step(
        &mut self,
        cell: CellHandle,
    ) -> Result<(Action<'graph, B>, Provenance, ArenaRef), DrainStalled> {
        self.spawns.clear();
        // `enter` borrows the graph exclusively and the step borrows the buffer, so the two are
        // taken as separate fields rather than through `self`.
        let Scheduler { graph, spawns, .. } = self;
        let (tiers, facts, books) = (graph.tiers, graph.facts, &graph.arena);
        (graph.cells)
            .enter(cell, |context| {
                run_step::<B>(context, tiers, (facts, books), spawns)
            })
            .map_err(|_| DrainStalled::Unenterable)
    }

    /// One cell born under another, through the door its placement names: a `Fresh` cell
    /// borrowing its parent's scratch, a `Shares` one its host's tenant.
    fn born(
        &mut self,
        under: CellHandle,
        placement: Placement,
        continuation: Continuation<'graph, 'graph, B>,
    ) -> Result<CellHandle, DrainStalled> {
        let cells = &mut self.graph.cells;
        let continuation = Some(continuation);
        let born = match placement {
            Placement::Fresh => cells
                .create_tree_lent(under, Lend::ParentScratch, continuation)
                .map(CellHandle::from)
                .map_err(|_| ()),
            Placement::Shares => cells
                .create_tenant(under, continuation)
                .map(CellHandle::from)
                .map_err(|_| ()),
        }
        .map_err(|_| DrainStalled::Unspawnable)?;
        #[cfg(test)]
        self.census.born();
        Ok(born)
    }

    /// Release a finished cell by its kind, and its borrow of `arena`, the arena node it ran
    /// under. Its step has already filled its consumer's receipt, so nothing is in flight at the
    /// death. The drain runs no slab cell — a slab cell is a root — so one here is refused.
    fn retire(&mut self, cell: CellHandle, arena: ArenaRef) -> Result<(), DrainStalled> {
        let released = match cell {
            CellHandle::Slab(_) => false,
            CellHandle::Tree(handle) => self.graph.cells.release_tree(handle).is_ok(),
            CellHandle::Tenant(handle) => self.graph.cells.release_tenant(handle).is_ok(),
        };
        if !released {
            return Err(DrainStalled::Unreleasable);
        }
        self.graph.arena.release(arena);
        #[cfg(test)]
        self.census.retired();
        Ok(())
    }

    /// Push a [`HopSample`] for `cell`'s tail, its loop's result received by `host`, its successor's
    /// birth threading `threads`.
    #[cfg(test)]
    fn sample_hop(&self, cell: CellHandle, host: CellHandle, threads: Option<usize>) {
        let cells = &self.graph.cells;
        let sample = HopSample {
            host,
            live: cells.live_bytes(),
            host_region: cells.region_used(host).unwrap_or(0),
            host_scratch: cells.scratch_used(host).unwrap_or(0),
            hop_region: match cell {
                CellHandle::Tree(_) => cells.region_used(cell).unwrap_or(0),
                _ => 0,
            },
            threads: threads.unwrap_or(0),
        };
        HOPS.with(|hops| hops.borrow_mut().push(sample));
    }

    /// The graph this drain runs over, to ask what is left in it.
    pub fn graph(&self) -> &Graph<'graph, B> {
        self.graph
    }

    /// The most cells the drain has held live at once — what a test reads to hold a loop of tail
    /// hops to its hand-off and a recursion to its path. A root is not the drain's birth and is not
    /// counted.
    #[cfg(test)]
    pub(crate) fn peak_live_cells(&self) -> usize {
        self.census.peak
    }
}

/// One step of `cell`'s continuation, inside its entry: wake its state, run the step function,
/// and tally.
fn run_step<'graph, 'step, 'here, 'scratch, 'lent, B: StepBundle<'graph>>(
    context: &mut Context<'graph, 'step, 'here, 'scratch, 'lent, B>,
    tiers: TypeTiers<'graph>,
    (load, books): (&'graph LoadFacts<'graph>, &BuildArena<'graph>),
    spawns: &mut Spawns<'graph, B>,
) -> (Action<'graph, B>, Provenance, ArenaRef)
where
    'graph: 'step + 'here + 'lent,
    'here: 'scratch,
    'lent: 'scratch,
    Erased<'graph, B::Birth>: Copy,
{
    let Continuation {
        step,
        provenance,
        arena,
        arrival,
        sought,
        state,
    } = context
        .continuation()
        .expect("a ready cell carries a continuation");
    #[cfg(test)]
    let agent_before = tiers.agent.len();
    // Both slots come off here, so the step is handed what it parked in each and
    // reaches neither door itself. Only a park stores them back, so a step that
    // finishes, hops or fails leaves the scratch slot empty and its bump goes back at
    // this step's end.
    let scratch = context.scratch_state();
    let state = match state {
        Rested::Awake(state) => state,
        Rested::Dormant(dormant, born) => {
            match wake::<B>(context, tiers, dormant, (born, provenance.loop_cell)) {
                Ok(woken) => woken,
                Err(_) => {
                    return (Action::refused(StepError::Unredeemable), provenance, arena);
                }
            }
        }
    };
    #[cfg(test)]
    let before = (
        (context.scratch_header().get())
            .and_then(HalfHeader::branch)
            .map_or(0, Branch::len),
        tiers.agent.len(),
        context.writer().in_use(),
    );
    // The door reads the settlement the cell runs under, over the composition its chain rests on
    // or the load, borrowed from the arena for the step.
    let door = books.door(arena);
    let (facts, composed) = match door.composition {
        Some((digest, composition)) => (Facts::composed(composition, door.top), Some(digest)),
        None => (Facts::under(load, door.top), None),
    };
    let step = step(Step::new(
        context,
        tiers,
        facts,
        &provenance,
        (arena, composed, arrival, sought),
        books,
        spawns,
        state,
        scratch,
    ));
    #[cfg(test)]
    {
        let len = |branch: Option<&Branch<'_>>| branch.map_or(0, Branch::len);
        let agent = tiers.agent;
        tally(|tiers| tiers.stepped += agent.len() - agent_before);
        if let Some(shape) = CACHING.take() {
            let scratch = len(context.scratch_header().get().and_then(HalfHeader::branch))
                .saturating_sub(before.0);
            let warm = agent.len().saturating_sub(before.1);
            LAID.with(|laid| {
                let mut laid = laid.borrow_mut();
                let entry = laid.entry(shape).or_default();
                *entry = (entry.0 + scratch, entry.1 + warm);
            });
        }
        REGION_WRITTEN.with(|written| {
            written.set(written.get() + context.writer().in_use().saturating_sub(before.2))
        });
        TIERS.with(|tiers| {
            let most = tiers.get();
            tiers.set(Tiers {
                region: most
                    .region
                    .max(len(context.header().get().and_then(HalfHeader::branch))),
                scratch: most.scratch.max(len(context
                    .scratch_header()
                    .get()
                    .and_then(HalfHeader::branch))),
                agent: most.agent.max(agent.len()),
                agent_verdicts: most.agent_verdicts.max(agent.verdict_count()),
                scratch_bytes: most.scratch_bytes.max(context.scratch_writer().in_use()),
                ..most
            });
        });
    }
    (step, provenance, arena)
}

/// Wake a birth a spawn, a hop or a root work's `leave` put to rest: redeem it into this step and
/// cross it to the executing cell's `'here` at the verdict's price — free for a child, whose
/// spawner is above it, and for a tenant, which crosses nothing; copied for a tail hop's sibling,
/// which is `Apart`. A hop's successor crosses each value it is handed on its own ([`hopped`]); a
/// child pins its birth whole and keeps each value it is handed beside it ([`spawned`]).
fn wake<'graph, 'step, 'here, 'scratch, 'lent, B: StepBundle<'graph>>(
    context: &mut Context<'graph, 'step, 'here, 'scratch, 'lent, B>,
    tiers: TypeTiers<'graph>,
    dormant: Dormant<'graph, B::Birth>,
    (born, loop_cell): (Born, Option<CellHandle>),
) -> Result<StateAt<'graph, 'here, B>, RedeemError>
where
    'graph: 'step + 'here + 'lent,
    'here: 'scratch,
    'lent: 'scratch,
    Erased<'graph, B::Birth>: Copy,
{
    let carrier = context.redeem(dormant)?;
    let kept = match born {
        Born::Hop => match hopped::<B>(context, tiers, &carrier, loop_cell) {
            Some(birth) => return Ok(B::born(birth?)),
            None => None,
        },
        Born::Child => spawned::<B>(context, &carrier),
        Born::Left => None,
    };
    let copy_bytes = B::weight(&context.read(&carrier).value());
    let birth = context.alloc_here(
        &[Operand {
            carrier: &carrier,
            copy_bytes,
        }],
        |writer, header, views| {
            let birth = B::cross(writer, header, &views[0], born, tiers);
            match (&kept, &views[0]) {
                (Some(kept), CrossedOperand::Pinned { .. }) => B::keeping(writer, birth, kept),
                // A copied birth is other storage than what was kept: it keeps afresh.
                _ => {
                    debug_assert!(kept.is_none(), "a spawned child's birth pins");
                    birth
                }
            }
        },
    );
    Ok(B::born(birth))
}

/// What a spawned child keeps of each value its birth hands on, where its spawner laid the birth
/// beside them ([`StepBundle::handings`]): the kept part each was handed through where that
/// redeems, and otherwise the value projected out of the birth, kept at the birth's home. Every
/// such home is the spawner or above it, and the spawner is above the child, so the birth itself
/// pins whole. `None` for a birth that hands nothing on, and where a value is gone.
fn spawned<'graph, 'step, 'here, 'scratch, 'lent, B: StepBundle<'graph>>(
    context: &mut Context<'graph, 'step, 'here, 'scratch, 'lent, B>,
    carrier: &Ready<'graph, 'step, B::Birth>,
) -> Option<SmallVec<[Kept<'graph>; 8]>>
where
    'graph: 'step + 'here + 'lent,
    'here: 'scratch,
    'lent: 'scratch,
    Erased<'graph, B::Birth>: Copy,
{
    let mut parts: SmallVec<[Option<Ready<'graph, 'step, KValueFamily>>; 8]> = SmallVec::new();
    let mut reused: SmallVec<[Option<Kept<'graph>>; 8]> = SmallVec::new();
    let laid = {
        let context = &*context;
        let birth = context.read(carrier).value();
        B::handings(&birth, &mut |index, kept, _| {
            let from_kept = kept.and_then(|part| part.redeemed(context));
            #[cfg(debug_assertions)]
            if let Some(redeemed) = &from_kept {
                let held = B::handed(context.read(carrier).value(), index);
                debug_assert!(
                    held.is_some_and(
                        |held| held.same_storage(&context.read::<KValueFamily>(redeemed).value())
                    ),
                    "a kept part names the storage the birth holds"
                );
            }
            reused.push(
                kept.filter(|part| part.path.is_empty() && from_kept.is_some())
                    .map(|part| part.kept),
            );
            parts.push(from_kept.or_else(|| {
                context.project::<B::Birth, KValueFamily>(carrier, |birth, _| {
                    B::handed(birth.value(), index).map(Active::new)
                })
            }));
        })
    };
    if !laid {
        return None;
    }
    let mut kept = SmallVec::new();
    for (part, reused) in parts.into_iter().zip(reused) {
        kept.push(match reused {
            Some(reused) => reused,
            None => Kept(context.keep(part?)),
        });
    }
    Some(kept)
}

/// A hop's successor's birth, where its predecessor laid it as a skeleton beside the values it
/// hands on ([`StepBundle::handings`]): each value crosses on its own, from the kept part it was
/// handed through where that redeems — pinned free from an ancestor — and otherwise out of the
/// skeleton; one that does not pin is copied into the successor, save, under a loop cell, a
/// threaded one or one handed through a kept part, which is copied into `loop_cell` and pinned from
/// there. Threaded values cross first, so the successor's branch links the loop cell's. The
/// successor keeps a carrier of each, at its home where it pinned and here where it was copied,
/// and the birth is built here over them. `None` for a birth that crosses whole.
fn hopped<'graph, 'step, 'here, 'scratch, 'lent, B: StepBundle<'graph>>(
    context: &mut Context<'graph, 'step, 'here, 'scratch, 'lent, B>,
    tiers: TypeTiers<'graph>,
    carrier: &Ready<'graph, 'step, B::Birth>,
    loop_cell: Option<CellHandle>,
) -> Option<Result<BirthAt<'graph, 'here, B>, RedeemError>>
where
    'graph: 'step + 'here + 'lent,
    'here: 'scratch,
    'lent: 'scratch,
    Erased<'graph, B::Birth>: Copy,
{
    let mut handings: SmallVec<[Option<Ready<'graph, 'step, KValueFamily>>; 8]> = SmallVec::new();
    let mut reused: SmallVec<[Option<Kept<'graph>>; 8]> = SmallVec::new();
    let mut threaded: SmallVec<[bool; 8]> = SmallVec::new();
    let mut threads_first: SmallVec<[bool; 8]> = SmallVec::new();
    let laid = {
        let context = &*context;
        let birth = context.read(carrier).value();
        B::handings(&birth, &mut |index, kept, threads| {
            let from_kept = kept.and_then(|part| part.redeemed(context));
            // A value handed through a kept part rested where it outlives a hop — the loop cell,
            // or a home above it — so one that does not pin rode an old loop cell: it is carried
            // into the current one, as a threaded value is, rather than into the hop.
            threaded.push((threads || from_kept.is_some()) && loop_cell.is_some());
            threads_first.push(threads);
            reused.push(
                kept.filter(|part| part.path.is_empty() && from_kept.is_some())
                    .map(|part| part.kept),
            );
            handings.push(from_kept.or_else(|| {
                context.project::<B::Birth, KValueFamily>(carrier, |birth, _| {
                    B::handed(birth.value(), index).map(Active::new)
                })
            }));
        })
    };
    if !laid {
        return None;
    }
    let Some(mut handings): Option<SmallVec<[_; 8]>> = handings.into_iter().collect() else {
        return Some(Err(RedeemError::Gone));
    };
    // What the loop threads crosses first: it rests in the loop cell, so the successor's branch
    // links the loop cell's, and each merge after stops at what that link reaches.
    let order: SmallVec<[usize; 8]> = (0..handings.len())
        .filter(|at| threads_first[*at])
        .chain((0..handings.len()).filter(|at| !threads_first[*at]))
        .collect();
    let mut crossed: SmallVec<[_; 8]> =
        crossed_here::<B>(context, tiers, &handings, &order, &|index| !threaded[index]);
    // A threaded value that did not pin is copied into the loop cell, which outlives the hop, and
    // pinned from there.
    if let Some(loop_cell) = loop_cell {
        let waiting: SmallVec<[usize; 8]> = (0..handings.len())
            .filter(|at| crossed[*at].0.is_none())
            .collect();
        for at in waiting {
            let operand = [Operand {
                carrier: &handings[at],
                copy_bytes: 0,
            }];
            let homed = context.alloc_into::<KValueFamily, KValueFamily>(
                loop_cell,
                &operand,
                |writer, header, views| {
                    let types = tiers.chain(Halves::alone(Half::of(writer, header)));
                    Active::new(crate::values::cross_view(writer, &types, &views[0]))
                },
            );
            let Ok(homed) = homed else {
                return Some(Err(RedeemError::Gone));
            };
            handings[at] = homed;
            reused[at] = None;
            crossed[at] = crossed_here::<B>(context, tiers, &handings, &[at], &|_| true)[at];
        }
    }
    let mut kept: SmallVec<[Kept<'graph>; 8]> = SmallVec::new();
    for ((carrier, reused), (value, pinned)) in handings.iter().zip(&reused).zip(&crossed) {
        let value = value.expect("every value has crossed");
        kept.push(match (pinned, reused) {
            (true, Some(reused)) => *reused,
            (true, None) => Kept(context.keep(carrier.clone())),
            (false, _) => {
                let lifted = context.lift::<KValueFamily>(value);
                Kept(context.keep(lifted))
            }
        });
    }
    let values: SmallVec<[_; 8]> = (crossed.iter())
        .map(|(value, _)| value.expect("every value has crossed"))
        .collect();
    let copy_bytes = B::weight(&context.read(carrier).value());
    let birth = context.alloc_here(
        &[Operand {
            carrier,
            copy_bytes,
        }],
        |writer, header, views| B::hop(writer, header, &views[0], &values, &kept, tiers),
    );
    #[cfg(test)]
    SKELETONS.with(|count| count.set(count.get() + 1));
    Some(Ok(birth))
}

/// The handings at `order`'s indices crossed into this cell in that order, each beside whether it
/// pinned, by its index; any other index is `None` and unpinned. One that does not pin is copied
/// here where `copies` takes its index, and is `None` where it does not.
#[allow(clippy::type_complexity)]
fn crossed_here<'graph, 'step, 'here, 'scratch, 'lent, B: StepBundle<'graph>>(
    context: &mut Context<'graph, 'step, 'here, 'scratch, 'lent, B>,
    tiers: TypeTiers<'graph>,
    handings: &[Ready<'graph, 'step, KValueFamily>],
    order: &[usize],
    copies: &dyn Fn(usize) -> bool,
) -> SmallVec<[(Option<crate::knot::KValue<'graph, 'here>>, bool); 8]>
where
    'graph: 'step + 'here + 'lent,
    'here: 'scratch,
    'lent: 'scratch,
    Erased<'graph, B::Birth>: Copy,
{
    let operands: SmallVec<[Operand<'graph, '_, 'step, KValueFamily>; 8]> = (order.iter())
        .map(|at| Operand {
            carrier: &handings[*at],
            copy_bytes: context
                .read::<KValueFamily>(&handings[*at])
                .value()
                .weight()
                .bytes(),
        })
        .collect();
    // The kinds are named: the birth's `Copy` bound in scope would otherwise answer for them.
    context.alloc_here::<_, KValueFamily>(&operands, |writer, header, views| {
        let types = tiers.chain(Halves::alone(Half::of(writer, header)));
        let mut crossed: SmallVec<[_; 8]> = SmallVec::from_elem((None, false), handings.len());
        let mut next = order.iter();
        let copies = |crossing: usize| copies(order[crossing]);
        crate::values::cross_views(writer, &types, views, &copies, &mut |value, pinned| {
            let at = *next.next().expect("one crossing per index in order");
            crossed[at] = (value, pinned);
        });
        crossed
    })
}

/// What a root work left at rest in its home when it ended by
/// [`Step::leave`](crate::scheduler::Step::leave), for the graph's owner to
/// [`resume`](Scheduler::resume) a later root work from.
///
/// Opaque and brand-free: it names no place, since the home is the cell the root work was born
/// under, which the owner already holds. A resume from anywhere else is refused when its birth does
/// not redeem.
pub struct Resting<'graph, B: StepBundle<'graph>>(Dormant<'graph, B::Birth>);

impl<'graph, B: StepBundle<'graph>> Clone for Resting<'graph, B>
where
    Erased<'graph, B::Birth>: Copy,
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<'graph, B: StepBundle<'graph>> Copy for Resting<'graph, B> where Erased<'graph, B::Birth>: Copy {}

/// The drain's own tally of the cells it has created and released. Cellgraph keeps no live count
/// for tree cells or tenants, so the high-water mark a test reads is counted here, where only a
/// test build pays for it.
#[cfg(test)]
#[derive(Default)]
struct Census {
    live: usize,
    peak: usize,
}

#[cfg(test)]
impl Census {
    /// Count one birth, and move the high-water mark if it rose.
    fn born(&mut self) {
        self.live += 1;
        self.peak = self.peak.max(self.live);
    }

    /// Count one release.
    fn retired(&mut self) {
        self.live -= 1;
    }
}

/// The drain's own failure: the stack ran dry before the root work ended, a door the substrate
/// guards refused, or a step could not proceed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrainStalled {
    /// The stack emptied before the root work ended: some cell is parked on a run nothing will
    /// fill.
    Unfinished,
    /// A cell could not be created under the parent its provenance names.
    Unspawnable,
    /// A cell on the stack was not enterable.
    Unenterable,
    /// A release the drain declared was refused.
    Unreleasable,
    /// A step reported that it could not proceed.
    Step(StepError),
}
