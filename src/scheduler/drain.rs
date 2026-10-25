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
//! A miss relays its binding's image into the agent tier, settles the body through the door the
//! program handed the graph at load ([`Settles`]), and, once the frame borrows the settlement,
//! evicts unheld settlements to the budget; nothing else evicts in a run. A step's door reads the
//! settlement its cell runs under over the load.
//!
//! Every field here is one scheduler's own. There is no static, no thread-local and no lazily
//! minted cell, so a second scheduler runs beside the first with nothing shared but the program
//! text and the shapes in it.

use crate::memory::{
    CellGraph, CellHandle, CreateError, Dormant, Erased, Lend, Operand, RedeemError,
    ReleaseAbsorption, ReleaseError, SlabHandle,
};
use crate::scheduler::action::{
    Action, Asked, Context, Kind, Placement, Spawns, Step, StepError, Use,
};
use crate::scheduler::arena::{BuildArena, Composed, Settled};
use crate::scheduler::continuation::{
    Born, Continuation, ContinuationFamily, NativeStep, Provenance, Report, Rested, StateAt,
    StepBundle, Work,
};
use crate::scheduler::delivery::KDelivery;
use crate::scheduler::seek::{Compose, Lending, Seek, Settles, Unsettled};
#[cfg(test)]
use crate::scope::HalfHeader;
use crate::scope::{Facts, LoadFacts, RegionHeader};
#[cfg(test)]
use crate::type_lattice::Branch;
use crate::values::{ArenaRef, BuildBudget, TypeTiers};

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
    /// How many compositions a miss built: a count.
    pub composed: usize,
    /// How many cells running composed code bounced with its syntax, their composition not live
    /// at their birth: a count.
    pub bounced: usize,
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
            composed: 0,
            bounced: 0,
        })
    };
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

/// One entry of the ready stack.
#[expect(
    clippy::large_enum_variant,
    reason = "an unborn request carries what a miss settles; boxing it would allocate per call, \
              and the stack holds only the unborn siblings along one path"
)]
enum Entry<'graph, B: StepBundle<'graph>> {
    /// A live cell with a step to run: a woken consumer, or a root work.
    Live(CellHandle),
    /// A request not yet born: the provenance the drain filled for it, the request at rest, and
    /// the arena node its asker runs under. `release_after` is a tail hop's predecessor, released
    /// with its borrow of that arena node once this cell's first step has returned — the
    /// successor wakes its state out of the predecessor's region, and borrows first.
    Unborn {
        provenance: Provenance,
        asked: Asked<'graph, B>,
        asker: ArenaRef,
        release_after: Option<CellHandle>,
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
                },
                arena: ArenaRef::ROOT,
                state,
            },
        )?;
        self.ready.push(Entry::Live(root));
        while let Some(entry) = self.ready.pop() {
            let (cell, release_after) = match entry {
                Entry::Live(cell) => (cell, None),
                Entry::Unborn {
                    provenance,
                    asked,
                    asker,
                    release_after,
                } => {
                    let born = if release_after.is_some() {
                        Born::Hop
                    } else {
                        Born::Child
                    };
                    let cell = self.create(provenance, asked, born, asker)?;
                    (cell, release_after.map(|predecessor| (predecessor, asker)))
                }
            };
            let (action, provenance, arena) = self.step(cell)?;
            // A hop's successor has woken what its predecessor kept, and borrows its arena node,
            // so the predecessor can go.
            if let Some((predecessor, arena)) = release_after {
                self.retire(predecessor, arena)?;
            }
            match action.kind() {
                Kind::Finished { completed } => {
                    self.retire(cell, arena)?;
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
                Kind::Tail(asked) => self.ready.push(Entry::Unborn {
                    provenance,
                    asked,
                    asker: arena,
                    release_after: Some(cell),
                }),
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
            let home = match asked.use_ {
                Use::Forwards => provenance.home,
                Use::Reads | Use::Keeps => cell,
            };
            ready.push(Entry::Unborn {
                provenance: Provenance {
                    parent: cell,
                    report: Some(Report {
                        slot,
                        use_: asked.use_,
                    }),
                    home,
                },
                asked,
                asker: arena,
                release_after: None,
            });
        }
    }

    /// Give a request its cell, under the parent its provenance names, its birth crossing as
    /// `born` says, running under the arena node its lending resolves to, which it borrows.
    /// `asker` is the arena node the cell that asked runs under. A miss evicts to the budget once
    /// the new cell borrows its settlement.
    fn create(
        &mut self,
        provenance: Provenance,
        asked: Asked<'graph, B>,
        born: Born,
        asker: ArenaRef,
    ) -> Result<CellHandle, DrainStalled> {
        let arena = &mut self.graph.arena;
        let (under, missed) = match asked.lending {
            Lending::Parent => (asker, false),
            Lending::Call(seek) => self.seek(seek),
            Lending::Under(under) if arena.is_live(under) => {
                arena.stamp(under);
                (under, false)
            }
            Lending::Under(_) => {
                #[cfg(test)]
                tally(|tiers| tiers.cold_births += 1);
                (ArenaRef::ROOT, false)
            }
            Lending::Compose(root) => self.composed(root),
        };
        self.graph.arena.borrow(under);
        if missed {
            let _evicted = self.graph.arena.evict_to_budget();
            #[cfg(test)]
            tally(|tiers| tiers.evicted += _evicted);
        }
        self.born(
            provenance.parent,
            asked.placement,
            Continuation {
                step: asked.step,
                provenance,
                arena: under,
                state: Rested::Dormant(asked.state, born),
            },
        )
    }

    /// The arena node a frame `seek` asks for runs under, and whether it was a miss: the parent
    /// itself for a seek with no key; else the settlement under the parent the key names, stamped,
    /// or built and its binding's image relayed into the agent tier. The root, cold, for a parent
    /// that is no longer live.
    fn seek(&mut self, seek: Seek<'graph>) -> (ArenaRef, bool) {
        let Seek { parent, settlement } = seek;
        let Some((key, image, ask)) = settlement else {
            // A callee of composed code whose composition is gone is born under the root, and
            // bounces with its syntax.
            return match self.graph.arena.is_live(parent) {
                true => (parent, false),
                false => (ArenaRef::ROOT, false),
            };
        };
        #[cfg(test)]
        tally(|tiers| tiers.sought += 1);
        let arena = &mut self.graph.arena;
        if !arena.is_live(parent) {
            #[cfg(test)]
            tally(|tiers| tiers.cold_births += 1);
            return (ArenaRef::ROOT, false);
        }
        if let Some(found) = arena.find(parent, key) {
            #[cfg(debug_assertions)]
            assert!(
                self.holds_binding(&image),
                "a hit's binding rests: its key digests the content its types are read from"
            );
            self.graph.arena.stamp(found);
            return (found, false);
        }
        // The binding's types rest once it resolves, before its walk reads them.
        let resting: crate::values::Chain<'graph, '_, 'graph, '_> = self.graph.tiers.resting();
        let before = resting.own_len();
        let mut relayed = before;
        let mut rest = || {
            image.relay(&resting);
            relayed = resting.own_len();
        };
        let settles = self.graph.settles;
        let Some(facts) = Settled::build(
            arena.facts(parent).cloned(),
            self.graph.facts,
            |bump, below, witness| settles.settle(bump, below, &ask, &mut rest, witness),
        ) else {
            debug_assert_eq!(relayed, before, "nothing rests for a settlement not built");
            return (parent, false);
        };
        let settled = self.graph.arena.insert(parent, key, facts);
        #[cfg(test)]
        tally(|tiers| {
            tiers.settled += 1;
            tiers.relayed += relayed - before;
            tiers.rested += resting.own_len() - relayed;
        });
        (settled, true)
    }

    /// The composition `compose` asks for, and whether it was a miss: found by its syntax digest
    /// and stamped; built through the program's door over the syntax a bounce copied; or, with no
    /// syntax in hand, the root, where the cell's first step bounces.
    fn composed(&mut self, compose: Compose) -> (ArenaRef, bool) {
        let digest = match &compose {
            Compose::Find(digest) => *digest,
            Compose::Owned { digest, .. } => {
                #[cfg(test)]
                tally(|tiers| tiers.bounced += 1);
                *digest
            }
        };
        if let Some(found) = self.graph.arena.composition(digest) {
            self.graph.arena.stamp(found);
            return (found, false);
        }
        let Compose::Owned { syntax, .. } = compose else {
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
        state,
    } = context
        .continuation()
        .expect("a ready cell carries a continuation");
    // Both slots come off here, so the step is handed what it parked in each and
    // reaches neither door itself. Only a park stores them back, so a step that
    // finishes, hops or fails leaves the scratch slot empty and its bump goes back at
    // this step's end.
    let scratch = context.scratch_state();
    let state = match state {
        Rested::Awake(state) => state,
        Rested::Dormant(dormant, born) => match wake::<B>(context, tiers, dormant, born) {
            Ok(state) => state,
            Err(_) => {
                return (Action::refused(StepError::Unredeemable), provenance, arena);
            }
        },
    };
    #[cfg(test)]
    let before = (
        (context.scratch_header().get())
            .and_then(HalfHeader::branch)
            .map_or(0, Branch::len),
        tiers.agent.len(),
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
        (arena, composed),
        spawns,
        state,
        scratch,
    ));
    #[cfg(test)]
    {
        let len = |branch: Option<&Branch<'_>>| branch.map_or(0, Branch::len);
        let agent = tiers.agent;
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
/// cross it to the executing cell's `'here` at the verdict's price — free for a child, whose spawner is above it,
/// and for a tenant, which crosses nothing; copied for a tail hop's sibling, which is `Apart`.
fn wake<'graph, 'step, 'here, 'scratch, 'lent, B: StepBundle<'graph>>(
    context: &mut Context<'graph, 'step, 'here, 'scratch, 'lent, B>,
    tiers: TypeTiers<'graph>,
    dormant: Dormant<'graph, B::Birth>,
    born: Born,
) -> Result<StateAt<'graph, 'here, B>, RedeemError>
where
    'graph: 'step + 'here + 'lent,
    'here: 'scratch,
    'lent: 'scratch,
    Erased<'graph, B::Birth>: Copy,
{
    let carrier = context.redeem(dormant)?;
    let copy_bytes = B::weight(&context.read(&carrier).value());
    let birth = context.alloc_here(
        &[Operand {
            carrier: &carrier,
            copy_bytes,
        }],
        |writer, header, views| B::cross(writer, header, &views[0], born, tiers),
    );
    Ok(B::born(birth))
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
