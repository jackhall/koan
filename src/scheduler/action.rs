//! A step's side of the drain: the doors it may use, the children it may ask for, and what it hands
//! back.
//!
//! A step is handed a [`Step`] by value and nothing else of the cell's. The raw context, the
//! cell's provenance and the drain's request buffer are its private fields, so every door that
//! registers a run, stores a slot or fills a receipt is one of its methods, and each does its
//! bookkeeping in the same call that builds the [`Action`] implying it. Those methods consume the
//! `Step`, so a step ends exactly once.
//!
//! Every arm of an `Action` carries no region borrow. `enter` quantifies a step's three brands per
//! call, so a step's return type can name none of them: what crosses back out is a handle, an
//! index, a dormant carrier or a borrow of program storage, and nothing else.

use crate::knot::{KValue, KValueFamily};
use crate::memory::{
    Active, CellHandle, CrossedOperand, DeliverError, Delivered, Dormant, DropFree, Erased,
    Operand, Ready, Reattachable, Receipt, ReceiptError, RedeemError, Stale, StepContext, Writer,
};
use crate::scheduler::continuation::{
    Continuation, ContinuationFamily, Destination, NativeStep, Provenance, ScratchFamily,
    ScratchState, State, Work,
};
use crate::scheduler::delivery::KDelivery;

/// The raw context a step runs in: every door `cellgraph` hands one, over koan's families. Only a
/// [`Step`] holds one.
type Context<'graph, 'step, 'here, 'scratch> =
    StepContext<'graph, 'step, 'here, 'scratch, ContinuationFamily, ScratchFamily, KDelivery>;

/// What a step hands the drain when it returns.
///
/// Opaque: the field is private and every way to build one is a method of [`Step`], so the
/// bookkeeping an arm implies has always happened by the time the drain sees it. A park has
/// registered its run and stored both its successor and its scratch state.
pub struct Action<'graph>(Kind<'graph>);

/// What an [`Action`] turned out to be, read by the drain and by nothing else. Anything in
/// `scheduler` can name a `Kind`; only this file can wrap one into an `Action`.
pub(super) enum Kind<'graph> {
    /// Finished. The drain releases the cell, and when `completed` — the step's delivery filled the
    /// last slot of its consumer's receipt run — queues that consumer, which is the only way a
    /// parked cell wakes. The consumer is read off the drain's own copy of the provenance, never
    /// off anything the step held.
    Finished { completed: bool },
    /// Parked on the receipt run the step registered, for the children it spawned. The drain
    /// creates them and leaves this cell alone until the run completes.
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

    /// What a fill leaves the producer to hand back: a finish that completed the consumer's run or
    /// one that left slots outstanding, or the machinery's failure when the door refused.
    fn settle(delivered: Result<Delivered, DeliverError>) -> Self {
        match delivered {
            Ok(Delivered::Complete) => Action(Kind::Finished { completed: true }),
            Ok(Delivered::Outstanding) => Action(Kind::Finished { completed: false }),
            Err(_) => Action(Kind::Failed(StepError::Undeliverable)),
        }
    }
}

/// What a native step runs against: the doors of the cell it is running in, and the ways to end.
///
/// It borrows the raw context, the cell's provenance and the drain's request buffer, all private,
/// and is three pointers wide. A step reads and writes regions through the doors below, asks for
/// children through [`spawn`](Self::spawn), and ends through one of [`park`](Self::park),
/// [`deliver_scratch`](Self::deliver_scratch), [`deliver_carrier`](Self::deliver_carrier),
/// [`tail`](Self::tail), [`done`](Self::done) and [`failed`](Self::failed) — each takes the `Step`
/// by value, and they are the only way to build an [`Action`], so a step ends exactly once and
/// nothing follows its end. It cannot store a slot, register a run or fill a receipt except as one
/// of those does, and it never learns its destination's slot, so it cannot deliver anywhere but
/// where the drain said.
///
/// ```
/// use koan::scheduler::{Action, ScratchState, State, Step};
///
/// fn once<'graph>(
///     step: Step<'_, 'graph, '_, '_, '_>,
///     _: State<'graph, '_>,
///     _: Option<ScratchState<'graph, '_, '_>>,
/// ) -> Action<'graph> {
///     step.done()
/// }
/// ```
///
/// A second end does not compile:
///
/// ```compile_fail,E0382
/// use koan::scheduler::{Action, ScratchState, State, Step};
///
/// fn twice<'graph>(
///     step: Step<'_, 'graph, '_, '_, '_>,
///     _: State<'graph, '_>,
///     _: Option<ScratchState<'graph, '_, '_>>,
/// ) -> Action<'graph> {
///     let _ = step.done();
///     step.done()
/// }
/// ```
pub struct Step<'a, 'graph, 'step, 'here, 'scratch>
where
    'graph: 'step + 'here,
    'here: 'scratch,
{
    context: &'a mut Context<'graph, 'step, 'here, 'scratch>,
    provenance: &'a Provenance,
    spawns: &'a mut Spawns<'graph>,
}

impl<'a, 'graph, 'step, 'here, 'scratch> Step<'a, 'graph, 'step, 'here, 'scratch>
where
    'graph: 'step + 'here,
    'here: 'scratch,
{
    /// The step the drain hands a cell's continuation, over the provenance it read off that same
    /// continuation and the buffer it cleared for this step.
    pub(super) fn new(
        context: &'a mut Context<'graph, 'step, 'here, 'scratch>,
        provenance: &'a Provenance,
        spawns: &'a mut Spawns<'graph>,
    ) -> Self {
        Step {
            context,
            provenance,
            spawns,
        }
    }

    /// The cell this step is running in.
    pub fn cell(&self) -> CellHandle {
        self.context.cell()
    }

    /// The cell this one's result goes to, for a step that builds its result in the consumer's
    /// region before filing it. `None` for a cell nothing waits on by receipt.
    pub fn consumer(&self) -> Option<CellHandle> {
        self.provenance.destination.map(|to| to.consumer)
    }

    /// This cell's region, at its own brand.
    pub fn writer(&self) -> Writer<'here> {
        self.context.writer()
    }

    /// This cell's scratch habitat, at its own brand.
    pub fn scratch_writer(&self) -> Writer<'scratch> {
        self.context.scratch_writer()
    }

    /// Build in this cell's region over `operands`, each crossed at the verdict's price.
    pub fn alloc_here<R, V>(
        &mut self,
        operands: &[Operand<'graph, '_, 'step, V>],
        build: impl for<'severed> FnOnce(
            Writer<'here>,
            &[CrossedOperand<'graph, 'here, 'severed, V>],
        ) -> R,
    ) -> R
    where
        V: Reattachable<'graph> + DropFree,
        Erased<'graph, V>: Copy,
    {
        self.context.alloc_here(operands, build)
    }

    /// Build in `dest`'s region over `operands`, and hand back the carrier resting there.
    pub fn alloc_into<T, V>(
        &mut self,
        dest: impl Into<CellHandle>,
        operands: &[Operand<'graph, '_, 'step, V>],
        build: impl for<'cell, 'severed> FnOnce(
            Writer<'cell>,
            &[CrossedOperand<'graph, 'cell, 'severed, V>],
        ) -> Active<'graph, 'cell, T>,
    ) -> Result<Ready<'graph, 'step, T>, Stale<CellHandle>>
    where
        T: Reattachable<'graph> + DropFree,
        V: Reattachable<'graph> + DropFree,
        Erased<'graph, V>: Copy,
    {
        self.context.alloc_into(dest, operands, build)
    }

    /// A value already in this cell's storage, as a carrier homed here.
    pub fn lift<T>(&self, value: T::At<'here>) -> Ready<'graph, 'step, T>
    where
        T: Reattachable<'graph> + DropFree,
    {
        self.context.lift(value)
    }

    /// Put a carrier to rest, free of every step brand.
    pub fn keep<T>(&mut self, carrier: Ready<'graph, 'step, T>) -> Dormant<'graph, T>
    where
        T: Reattachable<'graph> + DropFree,
    {
        self.context.keep(carrier)
    }

    /// Redeem an at-rest carrier into this step, or refuse.
    pub fn redeem<T>(
        &self,
        dormant: Dormant<'graph, T>,
    ) -> Result<Ready<'graph, 'step, T>, RedeemError>
    where
        T: Reattachable<'graph> + DropFree,
    {
        self.context.redeem(dormant)
    }

    /// Read a carrier out at the reading borrow.
    pub fn read<'cell, T>(
        &'cell self,
        carrier: &'cell Ready<'graph, 'step, T>,
    ) -> Active<'graph, 'cell, T>
    where
        T: Reattachable<'graph> + DropFree,
        Erased<'graph, T>: Copy,
    {
        self.context.read(carrier)
    }

    /// A value carrier made reachable at this cell's `'here`: pinned where it already lives, and
    /// copied in where the verdict says a copy is cheaper — [`values::cross_here`] over this cell.
    ///
    /// [`values::cross_here`]: crate::values::cross_here
    pub fn cross_here(
        &mut self,
        carrier: &Ready<'graph, 'step, KValueFamily>,
    ) -> KValue<'graph, 'here> {
        crate::values::cross_here(self.context, carrier)
    }

    /// Take one slot of this cell's receipt run, leaving it empty.
    pub fn receipt(
        &self,
        index: usize,
    ) -> Result<Receipt<'graph, 'step, 'scratch, KDelivery>, ReceiptError> {
        self.context.receipt(index)
    }

    /// How many slots this cell's receipt run has, or `None` when none is at rest.
    pub fn receipt_count(&self) -> Option<usize> {
        self.context.receipt_count()
    }

    /// Ask for one child, and take back the slot of this step's run it will report to. The drain
    /// creates it after the step returns, under this cell.
    pub fn spawn(&mut self, request: Request<'graph>) -> Slot {
        self.spawns.push(request)
    }

    /// Park on the children already spawned, resuming at `step` over `state`.
    ///
    /// The run is registered and the successor stored here, so a park that did neither is
    /// unrepresentable. `asked` is the last slot [`spawn`](Self::spawn) handed back: it is never
    /// read, only required, and a step with no child to wait on has none to give — which is how an
    /// empty park is refused at compile time rather than at run time.
    ///
    /// A park stores **both** slots: `state` is what the woken step resumes over in storage, and
    /// `scratch` is what it carries in the scratch habitat. The drain took the scratch slot off
    /// before this step began and nothing else fills it, so `None` leaves it empty and this step's
    /// end hands the bump back before the registered run is laid down in it.
    pub fn park(
        self,
        asked: Slot,
        step: NativeStep<'graph>,
        state: State<'graph, 'here>,
        scratch: Option<ScratchState<'graph, 'here, 'scratch>>,
    ) -> Action<'graph> {
        // Required, never read: holding one is the proof that the buffer is non-empty.
        let _ = asked;
        // After the spawns, which is what sizes the run. `register_receipts` only records the
        // count; it is applied to the cell when the step returns.
        if self.context.register_receipts(self.spawns.len()).is_err() {
            return self.failed(StepError::Undeliverable);
        }
        self.context.store_successor(Continuation::Native {
            step,
            provenance: *self.provenance,
            state,
        });
        if let Some(scratch) = scratch {
            self.context.store_scratch_state(scratch);
        }
        Action(Kind::Park)
    }

    /// Fill this cell's destination slot with a value built operand-free in the consumer's scratch
    /// habitat, and finish.
    pub fn deliver_scratch(
        self,
        build: impl for<'their> FnOnce(
            Writer<'their>,
            &'their &'graph (),
        ) -> Active<'graph, 'their, KValueFamily>,
    ) -> Action<'graph> {
        self.deliver(|context, to| context.deliver_scratch(to.consumer, to.slot, build))
    }

    /// File a carrier this step already holds at rest in this cell's destination slot, and finish.
    pub fn deliver_carrier(self, carrier: Dormant<'graph, KValueFamily>) -> Action<'graph> {
        self.deliver(|context, to| context.deliver_carrier(to.consumer, to.slot, carrier))
    }

    /// Fill the destination slot through `fill`, and finish by what the fill did to the consumer's
    /// run. A cell with no destination has nowhere to deliver, which is the machinery's failure.
    fn deliver(
        self,
        fill: impl FnOnce(
            &Context<'graph, 'step, 'here, 'scratch>,
            Destination,
        ) -> Result<Delivered, DeliverError>,
    ) -> Action<'graph> {
        match self.provenance.destination {
            Some(to) => Action::settle(fill(self.context, to)),
            None => self.failed(StepError::Undeliverable),
        }
    }

    /// Hand this cell's work to a successor, which inherits its provenance verbatim.
    pub fn tail(self, request: Request<'graph>) -> Action<'graph> {
        Action(Kind::Tail(request))
    }

    /// Finish, with nothing delivered.
    pub fn done(self) -> Action<'graph> {
        Action(Kind::Finished { completed: false })
    }

    /// Could not proceed. A koan error is a tagged value and travels between cells as data, so this
    /// is the machinery's own failure and it stalls the drain.
    pub fn failed(self, error: StepError) -> Action<'graph> {
        Action(Kind::Failed(error))
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
/// The provenance is the drain's to fill, from how the request was handed over. Handed to
/// [`Step::spawn`], it is a child of the spawner, reporting to the slot of its spawn position;
/// handed to [`Step::tail`], it is a sibling or a co-tenant inheriting its predecessor's provenance
/// verbatim. Either way a cell reports to the run that asked for it.
#[derive(Clone, Copy)]
pub struct Request<'graph> {
    pub placement: Placement,
    /// What the cell runs, and what it is born holding.
    pub work: Work<'graph>,
}

/// One slot of the run a step is about to park on, handed back by [`Step::spawn`].
///
/// The field is private and `spawn` is its only source, so holding one proves this step's buffer
/// is non-empty: the drain clears the buffer before every step, a native step is a bare `fn`
/// holding no state of its own, and no [`State`] arm can carry a `Slot` across a park.
#[derive(Clone, Copy)]
pub struct Slot(usize);

impl Slot {
    /// Which slot of the spawner's receipt run the child fills.
    pub fn index(self) -> usize {
        self.0
    }
}

/// The drain's own buffer of requests, reached by a step only through [`Step::spawn`] and cleared
/// before each step.
///
/// It is not an arm of [`Action`] because a slice of requests would have to be branded somewhere,
/// and a step's return type can name no brand.
pub(super) struct Spawns<'graph> {
    requests: Vec<Request<'graph>>,
}

impl<'graph> Spawns<'graph> {
    pub(super) fn new() -> Self {
        Spawns {
            requests: Vec::new(),
        }
    }

    /// Ask for one child, and take back the slot it will report to.
    fn push(&mut self, request: Request<'graph>) -> Slot {
        let slot = Slot(self.requests.len());
        self.requests.push(request);
        slot
    }

    pub(super) fn len(&self) -> usize {
        self.requests.len()
    }

    /// One request by position, copied out so the drain reads it while it holds the graph.
    pub(super) fn get(&self, index: usize) -> Request<'graph> {
        self.requests[index]
    }

    pub(super) fn clear(&mut self) {
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
