//! A step's side of the drain: the doors it may use, the children it may ask for, and what it hands
//! back.
//!
//! A step is handed a [`Step`] by value and nothing else of the cell's. The raw context, the
//! cell's provenance and the drain's request buffer are its private fields, so every door that
//! registers a run, stores a slot, crosses a value or fills a receipt is the veneer's, performed
//! inside one of its methods from the provenance the drain filled. Those methods that end the step
//! consume it, so a step ends exactly once. Its two states are taken the same way: each take
//! hands back the `Step` in a form without that take, so a step takes each at most once.
//!
//! **No step names a place that is not its own.** No method takes or returns a handle, and none is
//! a carrier door: a step holds values at its own brands and nothing else, and says what it needs —
//! a child, a wait, a successor, a result — beside the hints that place it.
//!
//! Every arm of an `Action` carries no region borrow. `enter` quantifies a step's three brands per
//! call, so a step's return type can name none of them: what crosses back out is a dormant carrier,
//! a function pointer or a hint, and nothing else.

use crate::knot::{KValue, KValueFamily};
use crate::memory::{
    Active, CellHandle, CrossedOperand, DeliverError, Delivered, Dormant, Erased, Operand, Ready,
    ReattachableOverBoth, Receipt, Stale, StepContext, Writer,
};
use crate::scheduler::continuation::{
    BirthAt, Continuation, ContinuationFamily, NativeStep, Provenance, Report, Rested, StateAt,
    StepBundle, Work,
};
use crate::scheduler::delivery::KDelivery;

/// The raw context a step runs in: every door `cellgraph` hands one, over the bundle's families.
/// Only a [`Step`] and the drain hold one.
pub(super) type Context<'graph, 'step, 'here, 'scratch, B> = StepContext<
    'graph,
    'step,
    'here,
    'scratch,
    ContinuationFamily<B>,
    <B as StepBundle<'graph>>::Scratch,
    KDelivery,
>;

/// A bundle's scratch state at a step's two brands.
type ScratchAt<'graph, 'here, 'scratch, B> =
    <<B as StepBundle<'graph>>::Scratch as ReattachableOverBoth<'graph>>::At<'here, 'scratch>;

/// What a step hands the drain when it returns.
///
/// Opaque: the field is private and every way to build one is a method of [`Step`], so the
/// bookkeeping an arm implies has always happened by the time the drain sees it. A park has
/// registered its run and stored both its successor and its scratch state.
pub struct Action<'graph, B: StepBundle<'graph>>(Kind<'graph, B>);

/// What an [`Action`] turned out to be, read by the drain and by nothing else. Anything in
/// `scheduler` can name a `Kind`; only this file can wrap one into an `Action`.
pub(super) enum Kind<'graph, B: StepBundle<'graph>> {
    /// Finished. The drain releases the cell, and when `completed` — the step's delivery filled the
    /// last slot of its consumer's receipt run — pushes that consumer, which is the only way a
    /// parked cell wakes. The consumer is read off the drain's own copy of the provenance, never
    /// off anything the step held.
    Finished { completed: bool },
    /// Parked on the receipt run the step registered, for the children it asked for. The drain
    /// pushes their requests and leaves this cell alone until the run completes.
    Park,
    /// A tail call: the drain pushes the successor, and releases this cell once the successor's
    /// first step has run. The successor inherits this cell's provenance verbatim, so the `Use` its
    /// request carries is never read.
    Tail(Asked<'graph, B>),
    /// A root work's end that leaves a birth at rest in its home, for the graph's owner to resume a
    /// later root work from. The drain releases the cell and hands the carrier back out of `run`.
    Left(Dormant<'graph, B::Birth>),
    /// The step could not proceed. The drain abandons the work.
    Failed(StepError),
}

impl<'graph, B: StepBundle<'graph>> Action<'graph, B> {
    /// What this action is. The drain's only way in, and the reason the field is private.
    pub(super) fn kind(self) -> Kind<'graph, B> {
        self.0
    }

    /// The machinery's refusal, for the drain to report a step it could not start: a birth state
    /// that did not redeem, before any `Step` exists to end by.
    pub(super) fn refused(error: StepError) -> Self {
        Action(Kind::Failed(error))
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

/// What a native step runs against: its cell's writers, its state, its children's results, and the
/// ways to end.
///
/// It borrows the raw context, the cell's provenance and the drain's request buffer, all private,
/// and holds the cell's two states until the step takes them. A step asks for
/// children through [`spawn`](Self::spawn), reads what they delivered through
/// [`results`](Self::results), and ends through one of [`park`](Self::park),
/// [`tail`](Self::tail), [`finish_fresh`](Self::finish_fresh),
/// [`finish_in_home`](Self::finish_in_home), [`finish`](Self::finish), [`done`](Self::done),
/// [`leave`](Self::leave) and [`failed`](Self::failed) — each takes the `Step` by value, and they are the only way to build an
/// [`Action`], so a step ends exactly once and nothing follows its end. It cannot store a slot,
/// register a run, cross a value or fill a receipt except as one of those does, and it never learns
/// a handle, so it cannot deliver anywhere but where the drain said.
///
/// ```
/// use koan::program::Steps;
/// use koan::scheduler::{Action, Step};
///
/// fn once<'graph>(step: Step<'_, 'graph, '_, '_, '_, Steps>) -> Action<'graph, Steps> {
///     step.done()
/// }
/// ```
///
/// A second end does not compile:
///
/// ```compile_fail,E0382
/// use koan::program::Steps;
/// use koan::scheduler::{Action, Step};
///
/// fn twice<'graph>(step: Step<'_, 'graph, '_, '_, '_, Steps>) -> Action<'graph, Steps> {
///     let _ = step.done();
///     step.done()
/// }
/// ```
///
/// It holds the state its cell was woken with and the scratch state the previous step parked, and
/// each is taken once, by type: [`state`](Step::state) and [`scratch`](Step::scratch) hand the value
/// back beside the `Step` in its [`Taken`] form, which has no second take to call. A step that
/// never takes either names neither form — the two parameters default to [`Holding`].
///
/// ```
/// use koan::program::Steps;
/// use koan::scheduler::{Action, Step};
///
/// fn state_once<'graph>(step: Step<'_, 'graph, '_, '_, '_, Steps>) -> Action<'graph, Steps> {
///     let (step, _) = step.state();
///     step.done()
/// }
///
/// fn scratch_once<'graph>(step: Step<'_, 'graph, '_, '_, '_, Steps>) -> Action<'graph, Steps> {
///     let (step, _) = step.scratch();
///     step.done()
/// }
/// ```
///
/// A second take of the state does not compile:
///
/// ```compile_fail,E0599
/// use koan::program::Steps;
/// use koan::scheduler::{Action, Step};
///
/// fn state_twice<'graph>(step: Step<'_, 'graph, '_, '_, '_, Steps>) -> Action<'graph, Steps> {
///     let (step, _) = step.state();
///     let (step, _) = step.state();
///     step.done()
/// }
/// ```
///
/// Nor does a second take of the scratch state:
///
/// ```compile_fail,E0599
/// use koan::program::Steps;
/// use koan::scheduler::{Action, Step};
///
/// fn scratch_twice<'graph>(step: Step<'_, 'graph, '_, '_, '_, Steps>) -> Action<'graph, Steps> {
///     let (step, _) = step.scratch();
///     let (step, _) = step.scratch();
///     step.done()
/// }
/// ```
pub struct Step<
    'a,
    'graph,
    'step,
    'here,
    'scratch,
    B: StepBundle<'graph>,
    S: Hold = Holding,
    X: Hold = Holding,
> where
    'graph: 'step + 'here,
    'here: 'scratch,
{
    context: &'a mut Context<'graph, 'step, 'here, 'scratch, B>,
    provenance: &'a Provenance,
    spawns: &'a mut Spawns<'graph, B>,
    state: S::Of<StateAt<'graph, 'here, B>>,
    scratch: X::Of<Option<ScratchAt<'graph, 'here, 'scratch, B>>>,
}

/// Whether a [`Step`] still holds one of its two states. [`Holding`] keeps the field's type and
/// [`Taken`] replaces it with `()`, so the method that takes it is not there to call twice.
///
/// The markers carry no lifetime: the bundle's projection stays a field of `Step`, the only place
/// rustc #100013 lets it be named under [`NativeStep`]'s higher-ranked brands.
pub trait Hold {
    type Of<T>;
}

/// A [`Step`] still holding a state.
pub struct Holding;

/// A [`Step`] whose state has been taken.
pub struct Taken;

impl Hold for Holding {
    type Of<T> = T;
}

impl Hold for Taken {
    type Of<T> = ();
}

impl<'a, 'graph, 'step, 'here, 'scratch, B: StepBundle<'graph>>
    Step<'a, 'graph, 'step, 'here, 'scratch, B>
where
    'graph: 'step + 'here,
    'here: 'scratch,
{
    /// The step the drain hands a cell's continuation, over the provenance it read off that same
    /// continuation, the buffer it cleared for this step, and both slots the cell parked in.
    pub(super) fn new(
        context: &'a mut Context<'graph, 'step, 'here, 'scratch, B>,
        provenance: &'a Provenance,
        spawns: &'a mut Spawns<'graph, B>,
        state: StateAt<'graph, 'here, B>,
        scratch: Option<ScratchAt<'graph, 'here, 'scratch, B>>,
    ) -> Self {
        Step {
            context,
            provenance,
            spawns,
            state,
            scratch,
        }
    }
}

impl<'a, 'graph, 'step, 'here, 'scratch, B: StepBundle<'graph>, X: Hold>
    Step<'a, 'graph, 'step, 'here, 'scratch, B, Holding, X>
where
    'graph: 'step + 'here,
    'here: 'scratch,
{
    /// The state this cell holds: the one the previous step parked with, or the one the cell was
    /// born with, woken to this step's `'here` at the verdict's price. It comes back beside the
    /// `Step` in its [`Taken`] form, which has no `state` to call again; a park hands the next
    /// state back through [`park`](Step::park).
    pub fn state(
        self,
    ) -> (
        Step<'a, 'graph, 'step, 'here, 'scratch, B, Taken, X>,
        StateAt<'graph, 'here, B>,
    ) {
        let Step {
            context,
            provenance,
            spawns,
            state,
            scratch,
        } = self;
        let step = Step {
            context,
            provenance,
            spawns,
            state: (),
            scratch,
        };
        (step, state)
    }
}

impl<'a, 'graph, 'step, 'here, 'scratch, B: StepBundle<'graph>, S: Hold>
    Step<'a, 'graph, 'step, 'here, 'scratch, B, S, Holding>
where
    'graph: 'step + 'here,
    'here: 'scratch,
{
    /// The scratch state the previous step parked, if it parked one, beside the `Step` in its
    /// [`Taken`] form, like [`state`](Step::state). A step that parks again hands it, or its
    /// successor, back to [`park`](Step::park), and one that drops it lets this step's end hand the
    /// bump back.
    pub fn scratch(
        self,
    ) -> (
        Step<'a, 'graph, 'step, 'here, 'scratch, B, S, Taken>,
        Option<ScratchAt<'graph, 'here, 'scratch, B>>,
    ) {
        let Step {
            context,
            provenance,
            spawns,
            state,
            scratch,
        } = self;
        let step = Step {
            context,
            provenance,
            spawns,
            state,
            scratch: (),
        };
        (step, scratch)
    }
}

impl<'a, 'graph, 'step, 'here, 'scratch, B: StepBundle<'graph>, S: Hold, X: Hold>
    Step<'a, 'graph, 'step, 'here, 'scratch, B, S, X>
where
    'graph: 'step + 'here,
    'here: 'scratch,
{
    /// This cell's region, at its own brand.
    pub fn writer(&self) -> Writer<'here> {
        self.context.writer()
    }

    /// This cell's scratch habitat, at its own brand.
    pub fn scratch_writer(&self) -> Writer<'scratch> {
        self.context.scratch_writer()
    }

    /// Ask for one child, and take back the slot of this step's run it will report to. The drain
    /// creates it after the step returns, under this cell.
    ///
    /// The child's birth is handed over as this step holds it, at `'here`, and put to rest here as
    /// a carrier in this cell; the child's first entry wakes it at its own `'here`.
    pub fn spawn(&mut self, request: Request<'graph, 'here, B>) -> Slot {
        let state = self.rest(request.work.state);
        self.spawns.push(Asked {
            placement: request.placement,
            use_: request.use_,
            step: request.work.step,
            state,
        })
    }

    /// The results of the children this cell parked on, in the order it asked for them: a scratch
    /// fill at `'scratch`, a carrier fill redeemed and crossed to `'here`. Each slot is taken as it
    /// is read, so a second pass finds it empty.
    pub fn results(
        &mut self,
    ) -> impl Iterator<Item = Result<Received<'graph, 'here, 'scratch>, StepError>> {
        let context = &mut *self.context;
        let count = context.receipt_count().unwrap_or(0);
        (0..count).map(move |slot| match context.receipt(slot) {
            Ok(Receipt::Value(value)) => Ok(Received::Scratch(value)),
            Ok(Receipt::Carrier(Ok(carrier))) => {
                Ok(Received::Here(crate::values::cross_here(context, &carrier)))
            }
            _ => Err(StepError::Unredeemable),
        })
    }

    /// Park on the children already asked for, resuming at `step` over `state`.
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
        step: NativeStep<'graph, B>,
        state: StateAt<'graph, 'here, B>,
        scratch: Option<ScratchAt<'graph, 'here, 'scratch, B>>,
    ) -> Action<'graph, B> {
        // Required, never read: holding one is the proof that the buffer is non-empty.
        let _ = asked;
        // After the spawns, which is what sizes the run. `register_receipts` only records the
        // count; it is applied to the cell when the step returns.
        if self.context.register_receipts(self.spawns.len()).is_err() {
            return self.failed(StepError::Undeliverable);
        }
        self.context.store_successor(Continuation {
            step,
            provenance: *self.provenance,
            state: Rested::Awake(state),
        });
        if let Some(scratch) = scratch {
            self.context.store_scratch_state(scratch);
        }
        Action(Kind::Park)
    }

    /// Hand this cell's work to a successor at `placement`, which inherits its provenance verbatim.
    /// The successor's birth is handed over at `'here` and put to rest here, as a spawn's is.
    pub fn tail(mut self, placement: Placement, work: Work<'graph, 'here, B>) -> Action<'graph, B> {
        if !self.spawns.is_empty() {
            return self.failed(StepError::Unparked);
        }
        let state = self.rest(work.state);
        Action(Kind::Tail(Asked {
            placement,
            // Never read: a successor reports under its predecessor's provenance.
            use_: Use::Reads,
            step: work.step,
            state,
        }))
    }

    /// Finish with a result built by `build`, which takes no operands, and deliver it where the
    /// drain said.
    ///
    /// The door is the veneer's, from the `Use` this cell was asked with: under `Reads` the value
    /// is built in the consumer's scratch habitat and read back at its `'scratch`; under `Keeps` or
    /// `Forwards` it is built in this cell's home and filed as a carrier the consumer reads at its
    /// `'here`. One build serves both, because it is quantified over the brand it is built at.
    pub fn finish_fresh(
        self,
        build: impl for<'their> FnOnce(
            Writer<'their>,
            &'their &'graph (),
        ) -> Active<'graph, 'their, KValueFamily>,
    ) -> Action<'graph, B> {
        let report = match self.report() {
            Ok(report) => report,
            Err(error) => return self.failed(error),
        };
        if report.use_ == Use::Reads {
            let filled = self
                .context
                .deliver_scratch(self.provenance.parent, report.slot, build);
            return Action::settle(filled);
        }
        let placed = self.context.alloc_into::<KValueFamily, KValueFamily>(
            self.provenance.home,
            &[],
            // The witness is minted here: `CrossedOperand` declares `'graph: 'cell`, so the slice
            // argument's type implies the bound the witness states. Were that where-clause to go,
            // this would stop compiling — a compile error, never unsoundness.
            |writer, _: &[CrossedOperand<'graph, '_, '_, KValueFamily>]| build(writer, &&()),
        );
        self.file(report, placed)
    }

    /// Finish with a result built by `build` over `operands`, values this step holds, in this
    /// cell's home, and deliver it as a carrier. Each operand reaches the build pinned or copied as
    /// the verdict ruled, at the home's brand. Every `Use` takes this door: a build over operands
    /// never goes through scratch, where what it embeds could not follow.
    pub fn finish_in_home<const N: usize>(
        self,
        operands: [KValue<'graph, 'here>; N],
        build: impl for<'their> FnOnce(
            Writer<'their>,
            [KValue<'graph, 'their>; N],
            &'their &'graph (),
        ) -> Active<'graph, 'their, KValueFamily>,
    ) -> Action<'graph, B> {
        let report = match self.report() {
            Ok(report) => report,
            Err(error) => return self.failed(error),
        };
        let weights = operands.map(|value| value.weight().bytes());
        let lifted = operands.map(|value| self.context.lift::<KValueFamily>(value));
        let crossing: [Operand<'graph, '_, 'step, KValueFamily>; N] =
            std::array::from_fn(|index| Operand {
                carrier: &lifted[index],
                copy_bytes: weights[index],
            });
        let placed = self.context.alloc_into::<KValueFamily, KValueFamily>(
            self.provenance.home,
            &crossing,
            |writer, views| {
                let values =
                    std::array::from_fn(|index| crate::values::cross_view(writer, &views[index]));
                build(writer, values, &&())
            },
        );
        self.file(report, placed)
    }

    /// Finish with `value`, crossed into this cell's home at the verdict's price, and deliver it as
    /// a carrier.
    pub fn finish(self, value: KValue<'graph, 'here>) -> Action<'graph, B> {
        self.finish_in_home([value], |_, [value], _| Active::new(value))
    }

    /// End a root work by leaving `birth` at rest in its home — the cell it was born under — for
    /// the graph's owner to resume a later root work from through [`Scheduler::resume`]. The birth
    /// crosses into the home at the verdict's price, so it outlives this cell: free from a tenant
    /// of the home, which writes the home's own storage. Refused for a cell that reports to
    /// somebody — only a root work has nobody to deliver to — and for a step that asked for
    /// children without parking on them.
    ///
    /// [`Scheduler::resume`]: crate::scheduler::Scheduler::resume
    pub fn leave(self, birth: BirthAt<'graph, 'here, B>) -> Action<'graph, B>
    where
        Erased<'graph, B::Birth>: Copy,
    {
        if self.provenance.report.is_some() {
            return self.failed(StepError::Undeliverable);
        }
        if !self.spawns.is_empty() {
            return self.failed(StepError::Unparked);
        }
        let copy_bytes = B::weight(&birth);
        let lifted = self.context.lift::<B::Birth>(birth);
        let placed = self.context.alloc_into::<B::Birth, B::Birth>(
            self.provenance.home,
            &[Operand {
                carrier: &lifted,
                copy_bytes,
            }],
            |writer, views| Active::new(B::cross(writer, &views[0])),
        );
        let Ok(placed) = placed else {
            return self.failed(StepError::Stale);
        };
        let dormant = self.context.keep(placed);
        Action(Kind::Left(dormant))
    }

    /// Finish, with nothing delivered.
    pub fn done(self) -> Action<'graph, B> {
        if !self.spawns.is_empty() {
            return self.failed(StepError::Unparked);
        }
        Action(Kind::Finished { completed: false })
    }

    /// Could not proceed. A koan error is a tagged value and travels between cells as data, so this
    /// is the machinery's own failure and it stalls the drain.
    pub fn failed(self, error: StepError) -> Action<'graph, B> {
        Action(Kind::Failed(error))
    }

    /// Put a birth this step holds at `'here` to rest as a carrier in this cell, free of every step
    /// brand, for the cell it is handed to.
    fn rest(&mut self, birth: BirthAt<'graph, 'here, B>) -> Dormant<'graph, B::Birth> {
        let carrier = self.context.lift::<B::Birth>(birth);
        self.context.keep(carrier)
    }

    /// The slot this cell reports to, for an end that delivers — refused for a cell asked for
    /// nobody, and for a step that asked for children and did not park on them.
    fn report(&self) -> Result<Report, StepError> {
        let report = self.provenance.report.ok_or(StepError::Undeliverable)?;
        if !self.spawns.is_empty() {
            return Err(StepError::Unparked);
        }
        Ok(report)
    }

    /// Put a result built in the home to rest, and file it in the slot this cell reports to.
    fn file(
        self,
        report: Report,
        placed: Result<Ready<'graph, 'step, KValueFamily>, Stale<CellHandle>>,
    ) -> Action<'graph, B> {
        let Ok(placed) = placed else {
            return self.failed(StepError::Stale);
        };
        let carrier = self.context.keep(placed);
        let filled = self
            .context
            .deliver_carrier(self.provenance.parent, report.slot, carrier);
        Action::settle(filled)
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

/// What the spawner will do with a child's result — which only the spawner knows — and so where
/// the result's home is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Use {
    /// Inspect it and drop it: the home is the spawner's scratch where the result is fresh, else
    /// the spawner.
    Reads,
    /// Embed it, bind it or hold it across parks: the home is the spawner.
    Keeps,
    /// Embed it in the spawner's own result: the home is the spawner's own home.
    Forwards,
}

/// What a step asks the drain for: a [`Work`] and two hints. Never a place and never a destination.
///
/// The provenance is the drain's to fill: handed to [`Step::spawn`], the child is born under the
/// spawner and reports to the slot of its spawn position, with its home the one `use_` names.
pub struct Request<'graph, 'here, B: StepBundle<'graph>>
where
    'graph: 'here,
{
    pub placement: Placement,
    pub use_: Use,
    /// What the cell runs, and what it is born holding, at the spawner's `'here`.
    pub work: Work<'graph, 'here, B>,
}

/// One slot of the run a step is about to park on, handed back by [`Step::spawn`].
///
/// The field is private and `spawn` is its only source, so holding one proves this step's buffer
/// is non-empty: the drain clears the buffer before every step, and a native step is a bare `fn`
/// holding no state of its own. A bundle whose state family carried a `Slot` across a park could
/// forge the proof; the park it bought registers a run nothing fills, which the drain reports as
/// [`DrainStalled::Unfinished`](crate::scheduler::DrainStalled::Unfinished).
#[derive(Clone, Copy)]
pub struct Slot(usize);

impl Slot {
    /// Which slot of the spawner's receipt run the child fills.
    pub fn index(self) -> usize {
        self.0
    }
}

/// One slot of the run a woken step parked on, as the consumer can use it.
pub enum Received<'graph, 'here, 'scratch>
where
    'graph: 'here,
    'here: 'scratch,
{
    /// A scratch fill: built in this cell's scratch habitat by a child asked with
    /// [`Use::Reads`].
    Scratch(KValue<'graph, 'scratch>),
    /// A carrier fill, redeemed and crossed to this cell's own brand.
    Here(KValue<'graph, 'here>),
}

/// A request at rest: what the drain's buffer holds and a tail carries, the state already put to
/// rest as a carrier.
pub(super) struct Asked<'graph, B: StepBundle<'graph>> {
    pub placement: Placement,
    pub use_: Use,
    pub step: NativeStep<'graph, B>,
    pub state: Dormant<'graph, B::Birth>,
}

/// The drain's own buffer of requests, reached by a step only through [`Step::spawn`] and emptied
/// before each step.
///
/// It is not an arm of [`Action`] because a slice of requests would have to be branded somewhere,
/// and a step's return type can name no brand.
pub(super) struct Spawns<'graph, B: StepBundle<'graph>> {
    requests: Vec<Asked<'graph, B>>,
}

impl<'graph, B: StepBundle<'graph>> Spawns<'graph, B> {
    pub(super) fn new() -> Self {
        Spawns {
            requests: Vec::new(),
        }
    }

    /// Ask for one child, and take back the slot it will report to.
    fn push(&mut self, request: Asked<'graph, B>) -> Slot {
        let slot = Slot(self.requests.len());
        self.requests.push(request);
        slot
    }

    pub(super) fn len(&self) -> usize {
        self.requests.len()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.requests.is_empty()
    }

    /// Every request, by slot, leaving the buffer empty.
    pub(super) fn take(
        &mut self,
    ) -> impl DoubleEndedIterator<Item = (usize, Asked<'graph, B>)> + '_ {
        self.requests.drain(..).enumerate()
    }

    pub(super) fn clear(&mut self) {
        self.requests.clear();
    }
}

/// What a step reports when it cannot proceed. A koan error is a tagged value and travels between
/// cells as data; this is the machinery's own failure, not the program's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepError {
    /// The home a result was bound for is no longer live.
    Stale,
    /// The cell has no slot to fill, or the door refused the fill.
    Undeliverable,
    /// A carrier did not redeem: a state at birth, or a result read back.
    Unredeemable,
    /// The step asked for children and ended without parking on them.
    Unparked,
}
