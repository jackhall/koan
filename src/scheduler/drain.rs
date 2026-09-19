//! The drain: the loop that runs queued cells, and the graph state it runs them over.
//!
//! Every field here is one scheduler's own. There is no static, no thread-local and no lazily
//! minted cell, so a second scheduler runs beside the first with nothing shared but the program
//! text and the shapes in it.

use std::collections::VecDeque;

use crate::memory::{CellGraph, CellHandle, ReleaseAbsorption, SlabHandle};
use crate::scheduler::action::{Action, Kind, Placement, Request, Spawns, StepError};
use crate::scheduler::continuation::{
    CellPlace, Continuation, ContinuationFamily, Destination, NativeStep, Provenance, Resume,
    ScratchFamily, State, Work,
};
use crate::scheduler::delivery::KDelivery;
use crate::scheduler::submit::{Birth, Submissions, Unit, UnitId};

/// The drain and the graph of cells it runs.
pub struct Scheduler<'graph> {
    graph: CellGraph<'graph, ContinuationFamily, ScratchFamily, KDelivery>,
    queue: Queue,
    spawns: Spawns<'graph>,
    /// Units of work with no cell yet, each waiting on a count of dependencies.
    pending: Submissions<'graph>,
    /// A tail hop's predecessor, waiting on its successor's first step. The successor redeems out
    /// of it, so its release is the last move of the hand-off rather than part of the creation.
    deferred: Option<CellHandle>,
    #[cfg(test)]
    census: Census,
}

impl<'graph> Scheduler<'graph> {
    /// A scheduler over a slab of `cap` cells, under koan's own crossing verdict.
    pub fn new(cap: u32) -> Self {
        Scheduler {
            graph: CellGraph::new(cap, crate::values::verdict),
            queue: Queue::new(),
            spawns: Spawns::new(),
            pending: Submissions::new(),
            deferred: None,
            #[cfg(test)]
            census: Census::default(),
        }
    }

    /// Admit a unit of work as a slab cell and queue it, with nothing waiting on it. The drain's
    /// one door onto a birth that neither a running step nor the submission table asked for.
    pub fn admit(
        &mut self,
        step: NativeStep<'graph>,
        state: State<'graph, 'graph>,
    ) -> Result<SlabHandle, DrainStalled> {
        let continuation = Work { step, state }.continuation(Provenance {
            place: CellPlace::Slab,
            destination: None,
            unit: None,
        });
        let handle = self.in_slab(continuation)?;
        self.queue.push_fresh(handle.into());
        Ok(handle)
    }

    /// Register a unit that gets its cell once `dependencies` submitted units have finished.
    ///
    /// The count is the unit's, the edges say which finishes answer for it, and both are wired
    /// before the run: a body's reference graph is known before it runs, which is why a reader
    /// never observes a binding whose binder has not run.
    pub fn submit(&mut self, unit: Unit<'graph>, dependencies: usize) -> UnitId {
        self.pending.submit(unit, dependencies)
    }

    /// Record that finishing `producer` satisfies one of `dependent`'s dependencies.
    pub fn edge(&mut self, producer: UnitId, dependent: UnitId) {
        self.pending.edge(producer, dependent);
    }

    /// Run until the queue empties with nothing left to launch.
    ///
    /// Success is the queue empty, the graph empty, and the submission table empty beside them.
    /// Anything else is the drain's one failure: a koan error is a tagged value and travels between
    /// cells as data, so no `Result` passes from one cell to another and this is the only error the
    /// scheduler defines.
    pub fn run(&mut self) -> Result<(), DrainStalled> {
        loop {
            // Whatever the round before released. A unit reaching zero is a birth like any other,
            // so it happens here rather than inside the step that satisfied the last dependency.
            self.launch()?;
            let Some(cell) = self.queue.pop() else { break };
            let (action, provenance) = self.step(cell)?;
            // The hop whose successor the step just was: what that successor redeemed is copied in
            // and its own hand-off is behind it, so the predecessor's region can go now.
            self.release_deferred()?;
            match action.kind() {
                Kind::Done => self.finish(cell, provenance)?,
                Kind::Wakes(consumer) => {
                    self.finish(cell, provenance)?;
                    self.queue.push_in_flight(consumer);
                }
                Kind::Park => self.spawn(cell)?,
                Kind::Tail(successor) => self.hop(cell, provenance, successor)?,
                Kind::Failed(error) => return Err(DrainStalled::Step(error)),
            }
        }
        debug_assert!(
            self.deferred.is_none(),
            "a hop's successor ran before the queue emptied"
        );
        match (self.graph.is_empty(), self.pending.is_empty()) {
            (true, true) => Ok(()),
            (true, false) => Err(DrainStalled::UnitsPending),
            _ => Err(DrainStalled::CellsLive),
        }
    }

    /// Give a cell to every unit whose dependencies are all met, and queue it.
    fn launch(&mut self) -> Result<(), DrainStalled> {
        while let Some((id, unit)) = self.pending.ready() {
            let continuation = unit.work.continuation(Provenance {
                place: unit.birth.place(),
                destination: None,
                unit: Some(id),
            });
            let cell = match unit.birth {
                Birth::Slab => self.in_slab(continuation)?.into(),
                Birth::Under(parent, placement) => self.under(parent, placement, continuation)?,
            };
            self.queue.push_fresh(cell);
        }
        Ok(())
    }

    /// Release a cell whose work is done, and release the dependents of the unit it answered for.
    ///
    /// A chain of tail hops settles once, at its last cell: the provenance travels with the work,
    /// so it is whoever finishes it that satisfies the dependency, not whoever started it.
    fn finish(&mut self, cell: CellHandle, provenance: Provenance) -> Result<(), DrainStalled> {
        self.retire(cell)?;
        if let Some(unit) = provenance.unit {
            self.pending.settle(unit);
        }
        Ok(())
    }

    /// Enter one cell and run the step its continuation names.
    fn step(&mut self, cell: CellHandle) -> Result<(Action<'graph>, Provenance), DrainStalled> {
        self.spawns.clear();
        // `enter` borrows the graph exclusively and the step closure borrows the buffer, so the two
        // are taken as separate fields rather than through `self`.
        let Scheduler { graph, spawns, .. } = self;
        graph
            .enter(cell, |context| {
                let continuation = context
                    .continuation()
                    .expect("a queued cell carries a continuation");
                // Both slots come off here, so the step is handed what it parked in each and
                // reaches neither door itself. A step that finishes, hops or fails never hands
                // the scratch state back, so the slot stays empty and its bump goes back at this
                // step's end.
                let scratch = context.scratch_state();
                match continuation {
                    Continuation::Native {
                        step,
                        provenance,
                        state,
                    } => (
                        step(
                            context,
                            Resume {
                                provenance,
                                state,
                                scratch,
                            },
                            spawns,
                        ),
                        provenance,
                    ),
                }
            })
            .map_err(|_| DrainStalled::Unenterable)
    }

    /// Create every child the parked step asked for, and queue them. The cell itself is left
    /// alone: it is live, parked on the run it registered, and wakes when the last slot fills.
    fn spawn(&mut self, cell: CellHandle) -> Result<(), DrainStalled> {
        for index in 0..self.spawns.len() {
            let request = self.spawns.get(index);
            let child = self.create(cell, request, index)?;
            self.queue.push_in_flight(child);
        }
        Ok(())
    }

    /// One child under its spawner, at the placement the spawner asked for, reporting to `slot` of
    /// the spawner's run. Its provenance is the drain's to fill: a child is born under the cell
    /// that asked for it and reports to the slot it was pushed at, so no step can name a
    /// destination that is not its spawner's.
    fn create(
        &mut self,
        spawner: CellHandle,
        request: Request<'graph>,
        slot: usize,
    ) -> Result<CellHandle, DrainStalled> {
        let continuation = request.work.continuation(Provenance {
            place: CellPlace::Under(spawner),
            destination: Some(Destination {
                consumer: spawner,
                slot,
            }),
            unit: None,
        });
        self.under(spawner, request.placement, continuation)
    }

    /// Hand one cell's work to its successor.
    ///
    /// The successor is born first and the predecessor released last, because the successor's first
    /// step redeems what the predecessor kept: the release waits for that step to return, which is
    /// what [`release_deferred`](Self::release_deferred) does at the top of the next round.
    ///
    /// It is a sibling under the same parent, or a co-tenant of the same host — never a slab cell,
    /// which would have to hold the predecessor to redeem, pinning its region into the hold so the
    /// release sealed instead of reclaiming. That is why a hop out of a slab cell is refused: a
    /// slab cell is under nothing, so it has no sibling to hop to.
    ///
    /// The provenance travels verbatim. Same place, same destination — the successor inherits its
    /// predecessor's receipt, which is what makes a loop of hops one consumer's single result.
    fn hop(
        &mut self,
        cell: CellHandle,
        provenance: Provenance,
        successor: Request<'graph>,
    ) -> Result<(), DrainStalled> {
        let CellPlace::Under(place) = provenance.place else {
            return Err(DrainStalled::Unhoppable);
        };
        let continuation = successor.work.continuation(provenance);
        let successor = self.under(place, successor.placement, continuation)?;
        self.queue.push_hop(successor);
        self.deferred = Some(cell);
        Ok(())
    }

    /// One cell born in the slab, under nothing.
    fn in_slab(
        &mut self,
        continuation: Continuation<'graph, 'graph>,
    ) -> Result<SlabHandle, DrainStalled> {
        let handle = self
            .graph
            .create(Some(continuation))
            .map_err(|_| DrainStalled::SlabFull)?;
        #[cfg(test)]
        self.census.born();
        Ok(handle)
    }

    /// One cell born under another, through the door its placement names.
    fn under(
        &mut self,
        under: CellHandle,
        placement: Placement,
        continuation: Continuation<'graph, 'graph>,
    ) -> Result<CellHandle, DrainStalled> {
        let born = match placement {
            Placement::Fresh => self
                .graph
                .create_tree(under, Some(continuation))
                .map(CellHandle::from),
            Placement::Shares => self
                .graph
                .create_tenant(under, Some(continuation))
                .map(CellHandle::from),
        }
        .map_err(|_| DrainStalled::Unspawnable)?;
        #[cfg(test)]
        self.census.born();
        Ok(born)
    }

    /// Release the predecessor a hop left behind, if there is one.
    fn release_deferred(&mut self) -> Result<(), DrainStalled> {
        match self.deferred.take() {
            Some(predecessor) => self.retire(predecessor),
            None => Ok(()),
        }
    }

    /// Release a finished cell by its kind. Its step has already filled its consumer's receipt, so
    /// nothing is in flight at the death.
    fn retire(&mut self, cell: CellHandle) -> Result<(), DrainStalled> {
        #[cfg(test)]
        self.census.retired();
        let released = match cell {
            CellHandle::Slab(handle) => self
                .graph
                .release(handle, ReleaseAbsorption::IntoHolder)
                .is_ok(),
            CellHandle::Tree(handle) => self.graph.release_tree(handle).is_ok(),
            CellHandle::Tenant(handle) => self.graph.release_tenant(handle).is_ok(),
        };
        released.then_some(()).ok_or(DrainStalled::Unreleasable)
    }

    /// Whether every cell the drain created has been reclaimed.
    pub fn is_empty(&self) -> bool {
        self.graph.is_empty()
    }

    /// Whether the named cell is still live — what a test reads to watch a release land.
    pub fn is_live(&self, cell: impl Into<CellHandle>) -> bool {
        self.graph.is_live(cell)
    }

    /// The most cells the drain has held live at once — what a test reads to hold a loop of tail
    /// hops to its two-cell hand-off.
    #[cfg(test)]
    pub(crate) fn peak_live_cells(&self) -> usize {
        self.census.peak
    }
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

/// The two queues, `in_flight` strictly ahead of `fresh`: an in-progress computation finishes
/// before a new unit starts, and a tail successor pushed to the front of `in_flight` runs before
/// any sibling work.
struct Queue {
    in_flight: VecDeque<CellHandle>,
    fresh: VecDeque<CellHandle>,
}

impl Queue {
    fn new() -> Self {
        Queue {
            in_flight: VecDeque::new(),
            fresh: VecDeque::new(),
        }
    }

    fn pop(&mut self) -> Option<CellHandle> {
        self.in_flight
            .pop_front()
            .or_else(|| self.fresh.pop_front())
    }

    fn push_fresh(&mut self, cell: CellHandle) {
        self.fresh.push_back(cell);
    }

    fn push_in_flight(&mut self, cell: CellHandle) {
        self.in_flight.push_back(cell);
    }

    /// A tail successor goes to the *front*: it is the continuation of the step that just ran, and
    /// its predecessor is not released until it has run.
    fn push_hop(&mut self, cell: CellHandle) {
        self.in_flight.push_front(cell);
    }
}

/// The drain's own failure: the queue ran dry with work outstanding, or a step could not proceed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrainStalled {
    /// The queue emptied with cells still live.
    CellsLive,
    /// The queue emptied with units still in the table. Their dependencies name each other, so no
    /// finish will ever release them.
    UnitsPending,
    /// The slab refused a birth.
    SlabFull,
    /// A cell a step asked to spawn could not be created under its spawner.
    Unspawnable,
    /// A slab cell asked to hop. It is under nothing, so it has no sibling to hop to.
    Unhoppable,
    /// A cell the drain queued was not enterable.
    Unenterable,
    /// A release the drain declared was refused.
    Unreleasable,
    /// A step reported that it could not proceed.
    Step(StepError),
}
