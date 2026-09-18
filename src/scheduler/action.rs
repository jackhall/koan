//! What a step hands the drain, and how it describes the children it wants.
//!
//! Every arm carries no region borrow. `enter` quantifies a step's three brands per call, so a
//! step's return type can name none of them: what crosses back out is a handle, an index, a
//! dormant carrier or a borrow of program storage, and nothing else.

use crate::scheduler::continuation::Continuation;

/// What a step hands the drain when it returns.
pub enum Action<'graph> {
    /// Finished. The step has already filled its consumer's receipt, if it has one, so nothing is
    /// in flight when the drain releases the cell.
    Done,
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
pub struct Request<'graph> {
    pub placement: Placement,
    /// The child's birth continuation, at `'graph` — its arguments reach it as dormant carriers.
    pub continuation: Continuation<'graph, 'graph>,
    /// The slot of the spawner's run this child fills.
    pub slot: usize,
}

/// The successor of a tail call.
pub struct Hop<'graph> {
    pub placement: Placement,
    /// The successor's birth continuation, at `'graph`, carrying the predecessor's receipt forward.
    pub continuation: Continuation<'graph, 'graph>,
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
