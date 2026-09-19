//! What a step hands the drain, and how it describes the children it wants.
//!
//! Every arm carries no region borrow. `enter` quantifies a step's three brands per call, so a
//! step's return type can name none of them: what crosses back out is a handle, an index, a
//! dormant carrier or a borrow of program storage, and nothing else.

use crate::memory::CellHandle;
use crate::scheduler::continuation::Work;

/// What a step hands the drain when it returns.
pub enum Action<'graph> {
    /// Finished, with nothing waiting on it. The drain releases the cell.
    Done,
    /// Finished, and its last delivery filled the final slot of this consumer's receipt run. The
    /// drain releases the cell and queues the consumer, which is the only way a parked cell wakes.
    Wakes(CellHandle),
    /// Parked on the receipt run the step registered, for the children it pushed into
    /// [`Spawns`]. The drain creates them and leaves this cell alone until the run completes.
    Park,
    /// A tail call: the drain creates the successor, then releases this cell, in that order.
    Tail(Hop<'graph>),
    /// The step could not proceed. The drain abandons the run.
    Failed(StepError),
}

/// Where a spawned cell lives, from the one-bit hint its spawner supplies.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Placement {
    /// Its own region, as a tree child of the spawner: a fresh result, placed operand-free, and the
    /// whole region reclaimed at death.
    Fresh,
    /// No region of its own, as a tenant of the spawner: it writes the spawner's storage at the
    /// spawner's own brand, so it embeds an argument at no price.
    Shares,
}

/// One child a step asked for.
///
/// It names the work, not the place: the drain derives the child's [`Provenance`] from the spawner
/// it was pushed in and the slot named here, so a child always reports to the cell that asked for
/// it.
///
/// [`Provenance`]: crate::scheduler::Provenance
#[derive(Clone, Copy)]
pub struct Request<'graph> {
    pub placement: Placement,
    /// What the child runs, and what it is born holding.
    pub work: Work<'graph>,
    /// The slot of the spawner's run this child fills.
    pub slot: usize,
}

/// The successor of a tail call. It inherits its predecessor's place and destination, so it names
/// neither.
#[derive(Clone, Copy)]
pub struct Hop<'graph> {
    pub placement: Placement,
    /// What the successor runs, and what it is born holding.
    pub work: Work<'graph>,
}

/// The drain's own buffer of requests, handed to a step by `&mut` and cleared before each step.
///
/// It is not an arm of [`Action`] because a slice of requests would have to be branded somewhere,
/// and a step's return type can name no brand.
pub struct Spawns<'graph> {
    requests: Vec<Request<'graph>>,
}

impl<'graph> Spawns<'graph> {
    pub(crate) fn new() -> Self {
        Spawns {
            requests: Vec::new(),
        }
    }

    /// Ask for one child. The drain creates it after the step returns.
    pub fn push(&mut self, request: Request<'graph>) {
        self.requests.push(request);
    }

    pub fn len(&self) -> usize {
        self.requests.len()
    }

    pub fn is_empty(&self) -> bool {
        self.requests.is_empty()
    }

    /// One request by position, copied out so the drain reads it while it holds the graph.
    pub(crate) fn get(&self, index: usize) -> Request<'graph> {
        self.requests[index]
    }

    pub(crate) fn clear(&mut self) {
        self.requests.clear();
    }
}

/// What a step reports when it cannot proceed. A koan error is a tagged value and travels between
/// cells as data; this is the machinery's own failure, not the program's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepError {
    /// A cell the step named is no longer live.
    Stale,
    /// A delivery door refused the fill.
    Undeliverable,
    /// A dormant carrier did not redeem.
    Unredeemable,
}
