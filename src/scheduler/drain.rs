//! The drain: the loop that runs one root work to its end over a ready stack, and the graph it runs
//! over.
//!
//! Every field here is one scheduler's own. There is no static, no thread-local and no lazily
//! minted cell, so a second scheduler runs beside the first with nothing shared but the program
//! text and the shapes in it.

use crate::memory::{
    CellGraph, CellHandle, CreateError, Erased, Operand, RedeemError, ReleaseAbsorption,
    ReleaseError, SlabHandle,
};
use crate::scheduler::action::{
    Action, Asked, Context, Kind, Placement, Spawns, Step, StepError, Use,
};
use crate::scheduler::continuation::{
    Continuation, ContinuationFamily, Provenance, Report, Rested, StepBundle, Work,
};
use crate::scheduler::delivery::KDelivery;

/// A bundle's state at one brand.
type StateAt<'graph, 'cell, B> =
    <<B as StepBundle<'graph>>::State as crate::memory::Reattachable<'graph>>::At<'cell>;

/// The graph of cells a drain runs over: `cellgraph`'s graph closed over the continuation family,
/// the bundle's scratch family and the delivery bundle.
///
/// A newtype rather than an alias, so the families stay the scheduler's own: what owns a graph
/// makes one, takes its roots from it, keeps it across calls and asks what is left in it, and
/// reaches every other cell only through a [`Scheduler`] over it.
pub struct Graph<'graph, B: StepBundle<'graph>>(
    CellGraph<'graph, ContinuationFamily<B>, B::Scratch, KDelivery>,
);

impl<'graph, B: StepBundle<'graph>> Graph<'graph, B> {
    /// A graph over a slab of `cap` cells, under koan's own crossing verdict.
    pub fn new(cap: u32) -> Self {
        Graph(CellGraph::new(cap, crate::values::verdict))
    }

    /// A storage-only slab cell no drain enters or releases: the region a root work is born under
    /// and builds its results in. Refused when the slab is full.
    pub fn root(&mut self) -> Result<SlabHandle, CreateError> {
        self.0.create(None)
    }

    /// Give a root back. Refused for a root that is not live.
    pub fn release_root(&mut self, root: SlabHandle) -> Result<(), ReleaseError> {
        self.0.release(root, ReleaseAbsorption::IntoHolder)
    }

    /// Whether every cell of this graph, roots included, has been reclaimed.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether the named cell is still live — what a test reads to watch a release land.
    pub fn is_live(&self, cell: impl Into<CellHandle>) -> bool {
        self.0.is_live(cell)
    }

    /// The graph itself, for a test that drives the raw slot doors a step never reaches.
    #[cfg(test)]
    pub(super) fn cells(
        &mut self,
    ) -> &mut CellGraph<'graph, ContinuationFamily<B>, B::Scratch, KDelivery> {
        &mut self.0
    }
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
    Erased<'graph, B::State>: Copy,
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

    /// Run `work` to its end, born under `under` at `placement`.
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
    ) -> Result<(), DrainStalled> {
        // A view a stalled run left entries on starts over: they belong to a root work that will
        // never end, so nothing this run does may pop them.
        self.ready.clear();
        let under = under.into();
        let root = self.born(
            under,
            placement,
            Continuation {
                step: work.step,
                provenance: Provenance {
                    parent: under,
                    report: None,
                    home: under,
                },
                state: Rested::Awake(work.state),
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
                } => (self.create(provenance, asked)?, release_after),
            };
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
                        return Ok(());
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

    /// Give a request its cell, under the parent its provenance names.
    fn create(
        &mut self,
        provenance: Provenance,
        asked: Asked<'graph, B>,
    ) -> Result<CellHandle, DrainStalled> {
        self.born(
            provenance.parent,
            asked.placement,
            Continuation {
                step: asked.step,
                provenance,
                state: Rested::Dormant(asked.state),
            },
        )
    }

    /// Enter one cell, wake its state, and run the step its continuation names.
    fn step(&mut self, cell: CellHandle) -> Result<(Action<'graph, B>, Provenance), DrainStalled> {
        self.spawns.clear();
        // `enter` borrows the graph exclusively and the step borrows the buffer, so the two are
        // taken as separate fields rather than through `self`.
        let Scheduler { graph, spawns, .. } = self;
        graph
            .0
            .enter(cell, |context| {
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
                    Rested::Dormant(dormant) => match wake::<B>(context, dormant) {
                        Ok(state) => state,
                        Err(_) => {
                            return (Action::refused(StepError::Unredeemable), provenance);
                        }
                    },
                };
                let step = step(Step::new(context, &provenance, spawns, state, scratch));
                (step, provenance)
            })
            .map_err(|_| DrainStalled::Unenterable)
    }

    /// One cell born under another, through the door its placement names.
    fn born(
        &mut self,
        under: CellHandle,
        placement: Placement,
        continuation: Continuation<'graph, 'graph, B>,
    ) -> Result<CellHandle, DrainStalled> {
        let born = match placement {
            Placement::Fresh => self
                .graph
                .0
                .create_tree(under, Some(continuation))
                .map(CellHandle::from),
            Placement::Shares => self
                .graph
                .0
                .create_tenant(under, Some(continuation))
                .map(CellHandle::from),
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
            CellHandle::Tree(handle) => self.graph.0.release_tree(handle).is_ok(),
            CellHandle::Tenant(handle) => self.graph.0.release_tenant(handle).is_ok(),
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

/// Wake a state a spawn or a hop put to rest: redeem it into this step and cross it to the
/// executing cell's `'here` at the verdict's price — free for a child, whose spawner is above it,
/// and for a tenant, which crosses nothing; copied for a tail hop's sibling, which is `Apart`.
fn wake<'graph, 'step, 'here, 'scratch, B: StepBundle<'graph>>(
    context: &mut Context<'graph, 'step, 'here, 'scratch, B>,
    dormant: crate::memory::Dormant<'graph, B::State>,
) -> Result<StateAt<'graph, 'here, B>, RedeemError>
where
    'graph: 'step + 'here,
    'here: 'scratch,
    Erased<'graph, B::State>: Copy,
{
    let carrier = context.redeem(dormant)?;
    let copy_bytes = B::weight(&context.read(&carrier).value());
    Ok(context.alloc_here(
        &[Operand {
            carrier: &carrier,
            copy_bytes,
        }],
        |writer, views| B::cross(writer, &views[0]),
    ))
}

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
