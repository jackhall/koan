//! The drain: the loop that runs one root work to its end over a ready stack, and the graph it runs
//! over.
//!
//! The graph holds two slab roots when the [load door](super::load) stood it up: the run root every
//! root work is born under, and the build root whose region holds the root-level settlement
//! table. Before a call's frame is born the drain resolves what it borrows: the settlement its
//! asker's [`Seek`] finds or builds with one synchronous step, or, born cold, its parent's scratch
//! for a `Fresh` frame and nothing for a `Shares` one. Any other cell borrows what its parent reads.
//!
//! A miss enters the new settlement beside its asker, and the entry lays the binding's nodes
//! from the asker's region into the settlement's branch before its step runs, so the branch
//! holds them by value.
//!
//! The drain keeps every build cell's books: it stamps a hit's entry and a hop's, fills a new
//! child's entry and sets its home. A miss on a full table releases the least recently born child
//! with its subtree, leaf first, before the new child is created; nothing else releases a
//! settlement in a run. A released settlement a frame still borrows waits dead but
//! undisposed until its last borrower disposes.
//!
//! Every field here is one scheduler's own. There is no static, no thread-local and no lazily
//! minted cell, so a second scheduler runs beside the first with nothing shared but the program
//! text and the shapes in it.

use crate::memory::{
    CellGraph, CellHandle, CreateError, Dormant, Erased, Lend, Operand, RedeemError,
    ReleaseAbsorption, ReleaseError, SlabHandle, TreeHandle,
};
use crate::scheduler::action::{
    Action, Asked, Context, Kind, Placement, Spawns, Step, StepError, Use,
};
use crate::scheduler::continuation::{
    Born, Continuation, ContinuationFamily, NativeStep, Provenance, Report, Rested, StateAt,
    StepBundle, Work,
};
use crate::scheduler::delivery::KDelivery;
use crate::scheduler::seek::{Lending, Seek};
#[cfg(test)]
use crate::scope::HalfHeader;
use crate::scope::RegionHeader;
use crate::type_lattice::Branch;
use crate::values::{Chain, Decision, SpecBinding, SpecHome, TypeTiers};

/// The graph of cells a drain runs over: `cellgraph`'s graph closed over the continuation family,
/// the bundle's scratch family and the delivery bundle.
///
/// A newtype rather than an alias, so the families stay the scheduler's own: what owns a graph
/// makes one, takes its roots from it, keeps it across calls and asks what is left in it, and
/// reaches every other cell only through a [`Scheduler`] over it.
///
/// It holds the type tiers every step's chain reads first, so a step and a crossing take it
/// from the graph they run over rather than from a program or a static.
pub struct Graph<'graph, B: StepBundle<'graph>> {
    cells: CellGraph<'graph, ContinuationFamily<B>, B::Scratch, KDelivery, RegionHeader>,
    tiers: TypeTiers<'graph>,
    /// The build tree's core cell, whose region holds the root-level settlement table: `Some`
    /// for a graph the [load door](Graph::load) stood up, `None` for one [`new`](Graph::new) made,
    /// whose every frame is born cold.
    build_root: Option<SlabHandle>,
}

impl<'graph, B: StepBundle<'graph>> Graph<'graph, B> {
    /// A graph over a slab of `cap` cells, under koan's own crossing verdict, its steps' types read
    /// over `tiers`.
    pub fn new(cap: u32, tiers: TypeTiers<'graph>) -> Self {
        Graph::assemble(CellGraph::new(cap, crate::values::verdict), tiers, None)
    }

    /// A graph over `cells`, its steps' types read over `tiers`, its build root `build_root`.
    pub(super) fn assemble(
        cells: CellGraph<'graph, ContinuationFamily<B>, B::Scratch, KDelivery, RegionHeader>,
        tiers: TypeTiers<'graph>,
        build_root: Option<SlabHandle>,
    ) -> Self {
        Graph {
            cells,
            tiers,
            build_root,
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

    /// How many entries the settlement table in `cell`'s region header names, live or stale:
    /// `None` where no header is laid, `0` where the header has no table.
    #[cfg(test)]
    pub(crate) fn table_len(
        &self,
        cell: impl Into<CellHandle>,
    ) -> Result<Option<usize>, crate::memory::PeekError> {
        self.cells.peek(cell, |header, _| {
            header.into_value().table().map_or(0, |table| table.len())
        })
    }

    /// The live children the settlement table in `cell`'s region header names: `0` for a
    /// cell with no table or none live.
    #[cfg(test)]
    pub(crate) fn live_children(&self, cell: impl Into<CellHandle>) -> usize {
        let cells = &self.cells;
        let live = cells.peek(cell, |header, _| {
            (header.into_value().table())
                .map_or(0, |table| table.live(|spec| cells.is_live(spec)).count())
        });
        live.ok().flatten().unwrap_or(0)
    }

    /// Every live settlement of the build tree, under the build root's table and each live
    /// child's in turn.
    #[cfg(test)]
    pub(crate) fn live_settlements(&self) -> usize {
        let mut pending = std::vec::Vec::new();
        if let Some(root) = self.build_root {
            self.children_of(root.into(), &mut pending);
        }
        let mut live = 0;
        while let Some(spec) = pending.pop() {
            live += 1;
            self.children_of(spec.into(), &mut pending);
        }
        live
    }

    /// Push each live child `cell`'s table names onto `children`.
    #[cfg(test)]
    fn children_of(&self, cell: CellHandle, children: &mut std::vec::Vec<TreeHandle>) {
        let cells = &self.cells;
        let _ = cells.peek(cell, |header, _| {
            if let Some(table) = header.into_value().table() {
                children.extend(table.live(|spec| cells.is_live(spec)));
            }
        });
    }

    /// Whether the settlement `build` names has no undisposed child: `None` for the build
    /// root, or a settlement that is not live.
    #[cfg(test)]
    pub(crate) fn is_childless(&self, build: crate::values::ArenaRef) -> Option<bool> {
        self.cells.is_childless(build.opened()?).ok()
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
    /// How many settlements a miss or a test released, descendants included: a count.
    pub evicted: usize,
    /// How many frames were born cold because the build cell their seek or hop named is dead: a
    /// count.
    pub cold_births: usize,
    /// Nodes the drain laid into the agent tier from new settlements' askers' branches: a count.
    pub relayed: usize,
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
        })
    };
}

#[cfg(test)]
thread_local! {
    /// A settlement the drain evicts before its next step, for a test whose step cannot reach
    /// [`Scheduler::evict`]: set by the step, taken by the drain. A refused eviction panics.
    pub(crate) static EVICT_BEFORE_STEP: std::cell::Cell<Option<crate::values::ArenaRef>> =
        const { std::cell::Cell::new(None) };

    /// The caching body the running step runs, by its shape's address: set by the step, taken by
    /// the drain as the step returns.
    pub(crate) static CACHING: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };

    /// Per caching body, by its shape's address: the nodes its steps laid in the scratch branch,
    /// and in the agent tier. Tests reset it.
    pub(crate) static LAID: std::cell::RefCell<std::collections::BTreeMap<usize, (usize, usize)>> =
        const { std::cell::RefCell::new(std::collections::BTreeMap::new()) };

    /// A settlement whose children the drain logs to [`WATCH_LOG`] before each step it runs.
    pub(crate) static WATCHED: std::cell::Cell<Option<crate::values::ArenaRef>> =
        const { std::cell::Cell::new(None) };

    /// What the drain saw of [`WATCHED`], interleaved with what the test's steps log as they run.
    pub(crate) static WATCH_LOG: std::cell::RefCell<std::vec::Vec<Watched>> =
        const { std::cell::RefCell::new(std::vec::Vec::new()) };
}

/// One line of [`WATCH_LOG`].
#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Watched {
    /// Whether the watched settlement had no undisposed child, logged before a step.
    Childless(bool),
    /// A test step, named as it starts.
    Step(&'static str),
}

/// What a new cell borrows, as the drain resolved it from its asker's [`Lending`].
enum Borrows {
    /// What its parent reads: a tree cell its parent's scratch, a tenant a copy of its parent's
    /// lend.
    Parent,
    /// A call's frame: the settlement it found, or, `None`, what a cold frame borrows: its
    /// parent's scratch for a tree cell, nothing for a tenant.
    Call(Option<CellHandle>),
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
enum Entry<'graph, B: StepBundle<'graph>> {
    /// A live cell with a step to run: a woken consumer, or a root work.
    Live(CellHandle),
    /// A request not yet born: the provenance the drain filled for it and the request at rest.
    /// `release_after` is a tail hop's predecessor, released once this cell's first step has
    /// returned — the successor wakes its state out of the predecessor's region.
    Unborn {
        provenance: Provenance,
        asked: Asked<'graph, B>,
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
            Borrows::Parent,
            Continuation {
                step,
                provenance: Provenance {
                    parent: under,
                    report: None,
                    home: under,
                },
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
                    release_after,
                } => {
                    let born = if release_after.is_some() {
                        Born::Hop
                    } else {
                        Born::Child
                    };
                    // The cell whose region holds the request's birth: a hop's predecessor, else
                    // the cell that asked.
                    let asker = release_after.unwrap_or(provenance.parent);
                    (self.create(provenance, asked, born, asker)?, release_after)
                }
            };
            #[cfg(test)]
            if let Some(spec) = EVICT_BEFORE_STEP.take() {
                self.evict(spec).expect("the test's settlement evicts");
            }
            #[cfg(test)]
            if let Some(watched) = WATCHED.get() {
                let childless = self.graph.is_childless(watched) == Some(true);
                WATCH_LOG.with(|log| log.borrow_mut().push(Watched::Childless(childless)));
            }
            let (action, provenance) = self.step(cell)?;
            // A hop's successor has woken what its predecessor kept, so the predecessor can go.
            if let Some(predecessor) = release_after {
                self.retire(predecessor)?;
            }
            match action.kind() {
                Kind::Finished { completed } => {
                    self.retire(cell)?;
                    // The consumer comes off the drain's own copy of the provenance, read straight
                    // off the continuation, so no step can name the cell that wakes.
                    if provenance.report.is_none() {
                        debug_assert!(self.ready.is_empty(), "the root work ended last");
                        #[cfg(test)]
                        self.no_lend_cycle();
                        return Ok(None);
                    }
                    if completed {
                        self.ready.push(Entry::Live(provenance.parent));
                    }
                }
                Kind::Park => self.asked_by(cell, provenance),
                Kind::Tail(asked) => self.ready.push(Entry::Unborn {
                    provenance,
                    asked,
                    release_after: Some(cell),
                }),
                // Only the drain's own entry of a settlement runs one.
                Kind::Quiesced => unreachable!("a settlement is never on the ready stack"),
                // Only a root work leaves: `leave` refuses a cell that reports to somebody.
                Kind::Left(dormant) => {
                    self.retire(cell)?;
                    debug_assert!(self.ready.is_empty(), "the root work ended last");
                    #[cfg(test)]
                    self.no_lend_cycle();
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
    /// under `Forwards` the spawner's own home.
    fn asked_by(&mut self, cell: CellHandle, provenance: Provenance) {
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
                release_after: None,
            });
        }
    }

    /// Give a request its cell, under the parent its provenance names, its birth crossing as
    /// `born` says, borrowing what its lending resolves to. `asker` is the cell whose region holds
    /// the birth, which a missed settlement reads its binding's nodes from.
    fn create(
        &mut self,
        provenance: Provenance,
        asked: Asked<'graph, B>,
        born: Born,
        asker: CellHandle,
    ) -> Result<CellHandle, DrainStalled> {
        let borrows = match asked.lending {
            Lending::Parent => Borrows::Parent,
            Lending::Call(None) => Borrows::Call(None),
            Lending::Call(Some(seek)) => Borrows::Call(self.seek(seek, asker)?),
            Lending::Under(build) => Borrows::Call(match build.opened() {
                None => None,
                Some(spec) if self.graph.cells.is_live(spec) => {
                    self.stamp_hop(spec);
                    Some(spec.into())
                }
                Some(_) => {
                    #[cfg(test)]
                    tally(|tiers| tiers.cold_births += 1);
                    None
                }
            }),
        };
        self.born(
            provenance.parent,
            asked.placement,
            borrows,
            Continuation {
                step: asked.step,
                provenance,
                state: Rested::Dormant(asked.state, born),
            },
        )
    }

    /// The settlement a frame `seek` asks for borrows: the one the parent's table names under
    /// the key, if it is live, its entry stamped; or else one created under the parent and stepped
    /// at once beside `asker`, laying the binding's nodes first, before anything else runs, and
    /// entered in the table. A full table first releases its least recently born child with its
    /// subtree, and the new child takes its entry. `None` for a parent that is not live, or a
    /// graph with no build root, whose frame is born cold.
    fn seek(
        &mut self,
        seek: Seek<'graph, B>,
        asker: CellHandle,
    ) -> Result<Option<CellHandle>, DrainStalled> {
        #[cfg(test)]
        tally(|tiers| tiers.sought += 1);
        let Seek {
            parent,
            key,
            binding,
            work,
        } = seek;
        let parent: CellHandle = match (parent.opened(), self.graph.build_root) {
            (Some(spec), _) => spec.into(),
            (None, Some(root)) => root.into(),
            (None, None) => return Ok(None),
        };
        let cells = &self.graph.cells;
        let decided = cells.peek(parent, |header, _| {
            let table = header.into_value().table()?;
            Some(table.decide(key, |spec| cells.is_live(spec)))
        });
        let decision = match decided {
            Ok(decision) => decision.flatten(),
            Err(_) => {
                #[cfg(test)]
                tally(|tiers| tiers.cold_births += 1);
                return Ok(None);
            }
        };
        // A loaded graph's build cells all hold a table: the load door lays the root's, and
        // `enlist` a settlement's.
        debug_assert!(decision.is_some(), "a live build cell holds a table");
        let index = match decision {
            None => return Ok(None),
            Some(Decision::Hit(spec)) => {
                #[cfg(debug_assertions)]
                assert!(
                    self.holds_binding(&binding),
                    "a hit's settlement holds its binding's nodes: its key digests the \
                     content they are read from"
                );
                return Ok(Some(spec.into()));
            }
            Some(Decision::Fill(index)) => index,
            Some(Decision::Evict(index, victim)) => {
                self.release_subtree(victim)?;
                index
            }
        };
        // A settlement reports to nobody and is entered here, never from the ready stack. Its
        // step seeks nothing (`enlist` refuses a cell that asked for children), so the entry
        // `decide` chose is still free when the step returns.
        let continuation = Continuation {
            step: work.step,
            provenance: Provenance {
                parent,
                report: None,
                home: parent,
            },
            state: Rested::Awake(B::born(work.state)),
        };
        let spec = (self.graph.cells)
            .create_tree_lent(parent, Lend::Named(parent), Some(continuation))
            .map_err(|_| DrainStalled::Unspawnable)?;
        #[cfg(test)]
        tally(|tiers| tiers.settled += 1);
        let (action, _) = self.step_beside(spec.into(), asker, &binding)?;
        match action.kind() {
            Kind::Quiesced => {
                let cells = &self.graph.cells;
                let entered = cells.peek(parent, |header, _| {
                    let table = header.into_value().table()?;
                    table.fill(index, key, spec);
                    Some(())
                });
                let homed = cells.peek(spec, |header, _| {
                    header.into_value().set_home(SpecHome { me: spec, parent })
                });
                debug_assert!(
                    matches!((entered, homed), (Ok(Some(Some(()))), Ok(Some(())))),
                    "the parent's table and the settlement's header stand"
                );
                Ok(Some(spec.into()))
            }
            Kind::Failed(error) => Err(DrainStalled::Step(error)),
            _ => Err(DrainStalled::Step(StepError::Refused)),
        }
    }

    /// Whether the frozen load or the agent tier holds the node of every handle `binding` carries,
    /// as a settlement's seed laid them. Lays nothing. `adopt` lays children before parents,
    /// so a found handle's children are there too.
    #[cfg(debug_assertions)]
    fn holds_binding(&self, binding: &SpecBinding) -> bool {
        let tiers = self.graph.tiers;
        let resting = crate::type_lattice::TypeChain::resting(tiers.frozen, tiers.agent);
        binding.handles().all(|kt| resting.resolves(kt))
    }

    /// Stamp the entry of the live settlement `spec` in its parent's table: a hop of the same
    /// callee under it is a birth, as a seek that finds it is.
    fn stamp_hop(&self, spec: TreeHandle) {
        let cells = &self.graph.cells;
        let home = cells.peek(spec, |header, _| header.into_value().home());
        let Ok(Some(Some(home))) = home else {
            debug_assert!(false, "a live settlement has a home");
            return;
        };
        let stamped = cells.peek(home.parent, |header, _| {
            header
                .into_value()
                .table()
                .is_some_and(|table| table.stamp(spec))
        });
        // An entry is rekeyed only when its handle is stale, or it names the victim a miss
        // releases, so a live settlement's entry stands.
        debug_assert_eq!(stamped, Ok(Some(true)), "a live settlement has an entry");
    }

    /// Release the live settlement `top` and every live settlement beneath it, leaf
    /// first, and return how many: descend from `top` through live children to a settlement
    /// with none, release it, and start again. A released child's entry goes stale, so each
    /// descent finds a new leaf; the drain lays nothing and grows no call stack.
    fn release_subtree(&mut self, top: TreeHandle) -> Result<usize, DrainStalled> {
        let mut released = 0;
        while self.graph.cells.is_live(top) {
            let mut leaf = top;
            while let Some(child) = self.live_child(leaf.into()) {
                leaf = child;
            }
            (self.graph.cells.release_tree(leaf)).map_err(|_| DrainStalled::Unreleasable)?;
            released += 1;
        }
        #[cfg(test)]
        tally(|tiers| tiers.evicted += released);
        Ok(released)
    }

    /// A live child the table of the build cell `cell` names, if any.
    fn live_child(&self, cell: CellHandle) -> Option<TreeHandle> {
        let cells = &self.graph.cells;
        let found = cells.peek(cell, |header, _| {
            (header.into_value().table())?
                .live(|child| cells.is_live(child))
                .next()
        });
        found.ok().flatten().flatten()
    }

    /// Evict the settlement `spec` names, with its subtree, leaf first: refused for the build
    /// root and a settlement that is not live. Each waits dead but undisposed until its last
    /// borrower disposes, and its entry goes stale until the next miss on its key rebuilds it. A
    /// test drives this; in a run only a miss evicts.
    #[cfg(test)]
    pub(crate) fn evict(&mut self, spec: crate::values::ArenaRef) -> Result<(), DrainStalled> {
        let spec = spec.opened().ok_or(DrainStalled::Unreleasable)?;
        if !self.graph.cells.is_live(spec) {
            return Err(DrainStalled::Unreleasable);
        }
        self.release_subtree(spec).map(|_| ())
    }

    /// Evict every live settlement, as a test's teardown does before it releases the roots.
    #[cfg(test)]
    pub(crate) fn evict_every(&mut self) {
        let Some(root) = self.graph.build_root else {
            return;
        };
        while let Some(child) = self.live_child(root.into()) {
            self.release_subtree(child)
                .expect("a live settlement releases");
        }
    }

    /// The law every test run checks as its root work ends: koan's lends close no wait cycle
    /// ([`CellGraph::lend_cycle`]). A run cell never waits on a settlement, so a host's
    /// scratch holding a settlement's count is never what that settlement waits on.
    #[cfg(test)]
    fn no_lend_cycle(&self) {
        assert_eq!(
            self.graph.cells.lend_cycle(),
            None,
            "a lend cycle leaks its cells"
        );
    }

    /// Enter one cell, wake its state, and run the step its continuation names.
    fn step(&mut self, cell: CellHandle) -> Result<(Action<'graph, B>, Provenance), DrainStalled> {
        self.spawns.clear();
        // `enter` borrows the graph exclusively and the step borrows the buffer, so the two are
        // taken as separate fields rather than through `self`.
        let Scheduler { graph, spawns, .. } = self;
        let tiers = graph.tiers;
        (graph.cells)
            .enter(cell, |context| run_step::<B>(context, tiers, spawns, None))
            .map_err(|_| DrainStalled::Unenterable)
    }

    /// Enter the new settlement `spec` beside `asker` and run its one step, laying the node of
    /// every handle `binding` carries into the agent tier first: read from the region branch of the
    /// asker's write home, where the request's birth rests and so where every type its arguments
    /// carry resolves; insert-if-absent, a load handle at a bit test. The read sits at a brand of
    /// its own, so the agent tier keeps no reference into the run tree: a copy by value, made while
    /// the asker is between steps. An empty binding opens no read.
    fn step_beside(
        &mut self,
        spec: CellHandle,
        asker: CellHandle,
        binding: &SpecBinding,
    ) -> Result<(Action<'graph, B>, Provenance), DrainStalled> {
        if binding.is_empty() {
            return self.step(spec);
        }
        self.spawns.clear();
        let Scheduler { graph, spawns, .. } = self;
        let tiers = graph.tiers;
        (graph.cells)
            .enter_beside(spec, asker, |context, read, _| {
                let source = read.and_then(|header| header.into_value().branch());
                run_step::<B>(context, tiers, spawns, Some((source, binding)))
            })
            .map_err(|_| DrainStalled::Unenterable)
    }

    /// One cell born under another, through the door its placement names, borrowing what
    /// `borrows` says.
    fn born(
        &mut self,
        under: CellHandle,
        placement: Placement,
        borrows: Borrows,
        continuation: Continuation<'graph, 'graph, B>,
    ) -> Result<CellHandle, DrainStalled> {
        let cells = &mut self.graph.cells;
        let continuation = Some(continuation);
        let born = match (placement, borrows) {
            (Placement::Fresh, Borrows::Call(Some(spec))) => cells
                .create_tree_lent(under, Lend::Named(spec), continuation)
                .map(CellHandle::from)
                .map_err(|_| ()),
            (Placement::Fresh, Borrows::Parent | Borrows::Call(None)) => cells
                .create_tree_lent(under, Lend::ParentScratch, continuation)
                .map(CellHandle::from)
                .map_err(|_| ()),
            (Placement::Shares, Borrows::Call(spec)) => cells
                .create_tenant_lent(under, spec.map(Lend::Named), continuation)
                .map(CellHandle::from)
                .map_err(|_| ()),
            (Placement::Shares, Borrows::Parent) => cells
                .create_tenant(under, continuation)
                .map(CellHandle::from)
                .map_err(|_| ()),
        }
        .map_err(|_| DrainStalled::Unspawnable)?;
        #[cfg(test)]
        self.census.born();
        Ok(born)
    }

    /// Release a finished cell by its kind. Its step has already filled its consumer's receipt, so
    /// nothing is in flight at the death. The drain runs no slab cell — a slab cell is a root — so
    /// one here is refused.
    fn retire(&mut self, cell: CellHandle) -> Result<(), DrainStalled> {
        let released = match cell {
            CellHandle::Slab(_) => false,
            CellHandle::Tree(handle) => self.graph.cells.release_tree(handle).is_ok(),
            CellHandle::Tenant(handle) => self.graph.cells.release_tenant(handle).is_ok(),
        };
        if !released {
            return Err(DrainStalled::Unreleasable);
        }
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

/// One step of `cell`'s continuation, inside its entry: wake its state, seed its branch where
/// the entry was opened beside an asker (`seed`: the asker's region branch and the binding whose
/// nodes it lays), run the step function, and tally.
fn run_step<'graph, 'step, 'here, 'scratch, 'lent, B: StepBundle<'graph>>(
    context: &mut Context<'graph, 'step, 'here, 'scratch, 'lent, B>,
    tiers: TypeTiers<'graph>,
    spawns: &mut Spawns<'graph, B>,
    seed: Option<(Option<&Branch<'_>>, &SpecBinding)>,
) -> (Action<'graph, B>, Provenance)
where
    'graph: 'step + 'here + 'lent,
    'here: 'scratch,
    'lent: 'scratch,
    Erased<'graph, B::Birth>: Copy,
{
    let Continuation {
        step,
        provenance,
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
                return (Action::refused(StepError::Unredeemable), provenance);
            }
        },
    };
    // A settlement's state is awake, so nothing above precedes its seeding. Its binding rests
    // in the agent tier, which every frame under the settlement reads.
    if let Some((source, binding)) = seed {
        let types: Chain<'graph, 'here, 'graph, 'step> = tiers.resting();
        #[cfg(test)]
        let before = types.own_len();
        for kt in binding.handles() {
            types.adopt(source, kt);
        }
        #[cfg(test)]
        tally(|tiers| tiers.relayed += types.own_len() - before);
    }
    #[cfg(test)]
    let before = (
        (context.scratch_header().get())
            .and_then(HalfHeader::branch)
            .map_or(0, Branch::len),
        tiers.agent.len(),
    );
    let step = step(Step::new(
        context,
        tiers,
        &provenance,
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
    (step, provenance)
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
