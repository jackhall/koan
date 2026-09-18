//! The drain: the loop that runs queued cells, and the graph state it runs them over.
//!
//! Every field here is one scheduler's own. There is no static, no thread-local and no lazily
//! minted cell, so a second scheduler runs beside the first with nothing shared but the program
//! text and the shapes in it.

use std::collections::VecDeque;

use crate::memory::{
    CellGraph, CellHandle, EnterError, ReleaseAbsorption, SlabHandle, Stale, TenantHandle,
    TreeHandle,
};
use crate::scheduler::action::{Action, Spawns, StepError};
use crate::scheduler::continuation::{
    CellPlace, Continuation, ContinuationFamily, NativeStep, Provenance, Resume, ScratchFamily,
    State,
};
use crate::scheduler::delivery::KDelivery;

/// The drain and the graph of cells it runs.
pub struct Scheduler<'graph> {
    graph: CellGraph<'graph, ContinuationFamily, ScratchFamily, KDelivery>,
    queue: Queue,
    spawns: Spawns<'graph>,
}

impl<'graph> Scheduler<'graph> {
    /// A scheduler over a slab of `cap` cells, under koan's own crossing verdict.
    pub fn new(cap: u32) -> Self {
        Scheduler {
            graph: CellGraph::new(cap, crate::values::verdict),
            queue: Queue::new(),
            spawns: Spawns::new(),
        }
    }

    /// Admit a unit of work as a slab cell and queue it. The drain's only door onto a birth that
    /// no running step asked for: everything else a program spawns comes from a step's `Action`.
    pub fn admit(
        &mut self,
        step: NativeStep<'graph>,
        state: State<'graph, 'graph>,
    ) -> Result<SlabHandle, DrainStalled> {
        let continuation = Continuation::Native {
            step,
            provenance: Provenance {
                place: CellPlace::Slab,
                receipt: None,
            },
            state,
        };
        let handle = self
            .graph
            .create(Some(continuation))
            .map_err(|_| DrainStalled::SlabFull)?;
        self.queue.push_fresh(handle.into());
        Ok(handle)
    }

    /// Run until the queue empties.
    ///
    /// Success is the queue empty with the graph empty beside it. Anything else is the drain's one
    /// failure: a koan error is a tagged value and travels between cells as data, so no `Result`
    /// passes from one cell to another and this is the only error the scheduler defines.
    pub fn run(&mut self) -> Result<(), DrainStalled> {
        while let Some(cell) = self.queue.pop() {
            let (action, provenance) = self.step(cell)?;
            match action {
                Action::Done => self.retire(cell, provenance)?,
                Action::Park => todo!("the drain creates the children the step described"),
                Action::Tail(_) => {
                    todo!("the drain creates the successor, then releases the predecessor")
                }
                Action::Failed(error) => return Err(DrainStalled::Step(error)),
            }
        }
        if self.graph.is_empty() {
            Ok(())
        } else {
            Err(DrainStalled::CellsLive)
        }
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
                match continuation {
                    Continuation::Native {
                        step,
                        provenance,
                        state,
                    } => (
                        step(context, Resume { provenance, state }, spawns),
                        provenance,
                    ),
                }
            })
            .map_err(DrainStalled::from)
    }

    /// Release a finished cell by its kind. Its step has already filled its consumer's receipt, so
    /// nothing is in flight at the death.
    fn retire(&mut self, cell: CellHandle, _provenance: Provenance) -> Result<(), DrainStalled> {
        match cell {
            CellHandle::Slab(handle) => self
                .graph
                .release(handle, ReleaseAbsorption::IntoHolder)
                .map_err(|_| DrainStalled::Unreleasable),
            CellHandle::Tree(handle) => self
                .graph
                .release_tree(handle)
                .map_err(|_| DrainStalled::Unreleasable),
            CellHandle::Tenant(handle) => self
                .graph
                .release_tenant(handle)
                .map_err(|_| DrainStalled::Unreleasable),
        }
    }

    /// Whether every cell the drain created has been reclaimed.
    pub fn is_empty(&self) -> bool {
        self.graph.is_empty()
    }

    /// Whether the named cell is still live — what a test reads to watch a release land.
    pub fn is_live(&self, cell: impl Into<CellHandle>) -> bool {
        self.graph.is_live(cell)
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
}

/// The drain's own failure: the queue ran dry with work outstanding, or a step could not proceed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrainStalled {
    /// The queue emptied with cells still live.
    CellsLive,
    /// The slab refused a birth.
    SlabFull,
    /// A cell the drain queued was not enterable.
    Unenterable,
    /// A release the drain declared was refused.
    Unreleasable,
    /// A step reported that it could not proceed.
    Step(StepError),
}

impl From<EnterError> for DrainStalled {
    fn from(_: EnterError) -> Self {
        DrainStalled::Unenterable
    }
}

impl From<Stale<SlabHandle>> for DrainStalled {
    fn from(_: Stale<SlabHandle>) -> Self {
        DrainStalled::Unreleasable
    }
}

impl From<Stale<TreeHandle>> for DrainStalled {
    fn from(_: Stale<TreeHandle>) -> Self {
        DrainStalled::Unreleasable
    }
}

impl From<Stale<TenantHandle>> for DrainStalled {
    fn from(_: Stale<TenantHandle>) -> Self {
        DrainStalled::Unreleasable
    }
}
