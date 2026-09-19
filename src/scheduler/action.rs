//! What a step hands the drain, and how it describes the children it wants.
//!
//! Every arm carries no region borrow. `enter` quantifies a step's three brands per call, so a
//! step's return type can name none of them: what crosses back out is a handle, an index, a
//! dormant carrier or a borrow of program storage, and nothing else.

use crate::function::KValueFamily;
use crate::memory::{Active, CellHandle, DeliverError, Delivered, Dormant, Writer};
use crate::scheduler::continuation::{Context, NativeStep, Resume, State, Work};

/// What a step hands the drain when it returns.
///
/// Opaque: the field is private and every way to build one is a constructor below, so the
/// bookkeeping an arm implies has always happened by the time the drain sees it. A `Park` has
/// registered its run and stored its successor; a `Wakes` names the consumer of a fill that
/// completed that consumer's run, so neither a forged nor a spurious wake is representable.
pub struct Action<'graph>(Kind<'graph>);

/// What an [`Action`] turned out to be, read by the drain and by nothing else. Anything in
/// `scheduler` can name a `Kind`; only this file can wrap one into an `Action`.
pub(super) enum Kind<'graph> {
    /// Finished, with nothing waiting on it. The drain releases the cell.
    Done,
    /// Finished, and its last delivery filled the final slot of this consumer's receipt run. The
    /// drain releases the cell and queues the consumer, which is the only way a parked cell wakes.
    Wakes(CellHandle),
    /// Parked on the receipt run the step registered, for the children it pushed into
    /// [`Spawns`]. The drain creates them and leaves this cell alone until the run completes.
    Park,
    /// A tail call: the drain creates the successor, then releases this cell, in that order.
    Tail(Request<'graph>),
    /// The step could not proceed. The drain abandons the run.
    Failed(StepError),
}

impl<'graph> Action<'graph> {
    /// What this action is. The drain's only way in, and the reason the field is private.
    pub(super) fn kind(self) -> Kind<'graph> {
        self.0
    }

    /// Finished, with nothing waiting on it.
    pub fn done() -> Self {
        Action(Kind::Done)
    }

    /// Could not proceed. A koan error is a tagged value and travels between cells as data, so this
    /// is the machinery's own failure and it stalls the drain.
    pub fn failed(error: StepError) -> Self {
        Action(Kind::Failed(error))
    }

    /// Hand this cell's work to a successor, which inherits its provenance verbatim.
    pub fn tail(request: Request<'graph>) -> Self {
        Action(Kind::Tail(request))
    }

    /// Park on the children already pushed into `spawns`, resuming at `step` over `state`.
    ///
    /// The run is registered and the successor stored here, so a park that did neither is
    /// unrepresentable. `asked` is the last slot [`Spawns::push`] handed back: it is never read,
    /// only required, and a step with no child to wait on has none to give — which is how an empty
    /// park is refused at compile time rather than at run time.
    ///
    /// A step carrying in-progress state across the park stores it with `store_scratch_successor`
    /// itself, before this call.
    pub fn park<'here>(
        context: &mut Context<'graph, '_, 'here, '_>,
        resume: &Resume<'graph, '_>,
        spawns: &Spawns<'graph>,
        asked: Slot,
        step: NativeStep<'graph>,
        state: State<'graph, 'here>,
    ) -> Self {
        // Required, never read: holding one is the proof that the buffer is non-empty.
        let _ = asked;
        // After the pushes, which is what sizes the run. `register_receipts` only records the
        // count; it is applied to the cell when the step returns.
        if context.register_receipts(spawns.len()).is_err() {
            return Action::failed(StepError::Undeliverable);
        }
        context.store_successor(resume.successor(step, state));
        Action(Kind::Park)
    }

    /// Fill this cell's destination slot with a value built operand-free in the consumer's scratch
    /// habitat, and report what the fill did to the consumer's run.
    pub fn deliver_scratch(
        context: &Context<'graph, '_, '_, '_>,
        resume: &Resume<'graph, '_>,
        build: impl for<'their> FnOnce(
            Writer<'their>,
            &'their &'graph (),
        ) -> Active<'graph, 'their, KValueFamily>,
    ) -> Self {
        let Some(destination) = resume.provenance.destination else {
            return Action::failed(StepError::Undeliverable);
        };
        Action::settle(
            context.deliver_scratch(destination.consumer, destination.slot, build),
            destination.consumer,
        )
    }

    /// File a carrier this step already holds at rest in this cell's destination slot, the same
    /// way.
    pub fn deliver_carrier(
        context: &Context<'graph, '_, '_, '_>,
        resume: &Resume<'graph, '_>,
        carrier: Dormant<'graph, KValueFamily>,
    ) -> Self {
        let Some(destination) = resume.provenance.destination else {
            return Action::failed(StepError::Undeliverable);
        };
        Action::settle(
            context.deliver_carrier(destination.consumer, destination.slot, carrier),
            destination.consumer,
        )
    }

    /// What a fill leaves the producer to hand back: the wake when it completed the consumer's run,
    /// a plain finish when slots remain, and the machinery's failure when the door refused.
    fn settle(delivered: Result<Delivered, DeliverError>, consumer: CellHandle) -> Self {
        match delivered {
            Ok(Delivered::Complete) => Action(Kind::Wakes(consumer)),
            Ok(Delivered::Outstanding) => Action(Kind::Done),
            Err(_) => Action(Kind::Failed(StepError::Undeliverable)),
        }
    }
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

/// One cell a step asked the drain to create: a placement and a [`Work`], never a place and never a
/// destination.
///
/// The provenance is the drain's to fill, from how the request was handed over. Pushed into
/// [`Spawns`], it is a child of the pusher, reporting to the slot of its push position; handed to
/// [`Action::tail`], it is a sibling or a co-tenant inheriting its predecessor's provenance
/// verbatim. Either way a cell reports to the run that asked for it.
///
/// [`Provenance`]: crate::scheduler::Provenance
#[derive(Clone, Copy)]
pub struct Request<'graph> {
    pub placement: Placement,
    /// What the cell runs, and what it is born holding.
    pub work: Work<'graph>,
}

/// One slot of the run a step is about to park on, handed back by [`Spawns::push`].
///
/// The field is private and `push` is its only source, so holding one proves this step's buffer is
/// non-empty: the drain clears [`Spawns`] before every step, a native step is a bare `fn` holding
/// no state of its own, and no [`State`] arm can carry a `Slot` across a park.
#[derive(Clone, Copy)]
pub struct Slot(usize);

impl Slot {
    /// Which slot of the pusher's receipt run the child fills.
    pub fn index(self) -> usize {
        self.0
    }
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

    /// Ask for one child, and take back the slot of this step's run it will report to. The drain
    /// creates it after the step returns.
    pub fn push(&mut self, request: Request<'graph>) -> Slot {
        let slot = Slot(self.requests.len());
        self.requests.push(request);
        slot
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
