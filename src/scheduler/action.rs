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
    Active, CellHandle, CrossedOperand, DeliverError, Delivered, Dormant, Erased, Header, Operand,
    Ready, ReattachableOverBoth, Receipt, Stale, StepContext, Writer,
};
use crate::scheduler::continuation::{
    BirthAt, Born, Continuation, ContinuationFamily, NativeStep, Provenance, Report, Rested,
    StateAt, StepBundle, Work,
};
use crate::scheduler::delivery::KDelivery;
use crate::scheduler::seek::{Lending, Seek};
use crate::type_lattice::Root;
use crate::type_lattice::{Habitat, Halves, Own};
use crate::values::{BuildRef, Chain, Half, SpecKey, TypeHeader, Types};

/// The raw context a step runs in: every door `cellgraph` hands one, over the bundle's families.
/// Only a [`Step`] and the drain hold one.
pub(super) type Context<'graph, 'step, 'here, 'scratch, 'lent, B> = StepContext<
    'graph,
    'step,
    'here,
    'scratch,
    'lent,
    ContinuationFamily<B>,
    <B as StepBundle<'graph>>::Scratch,
    KDelivery,
    TypeHeader,
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
    /// A specialization's step has entered itself in its parent's table and stays alive with no
    /// continuation, reporting to nobody. Only the drain's own synchronous entry of a
    /// specialization sees it.
    Quiesced,
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
/// [`leave`](Self::leave) and [`failed`](Self::failed) — each takes the `Step` by value, and they
/// are the only way to build an [`Action`], so a step ends exactly once and nothing follows its
/// end. It cannot store a slot, register a run, cross a value or fill a receipt except as one of
/// those does, and it never learns a handle, so it cannot deliver anywhere but where the drain
/// said.
///
/// ```
/// use koan::program::KBundle;
/// use koan::scheduler::{Action, Step};
///
/// fn once<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, KBundle>) -> Action<'graph, KBundle> {
///     step.done()
/// }
/// ```
///
/// A second end does not compile:
///
/// ```compile_fail,E0382
/// use koan::program::KBundle;
/// use koan::scheduler::{Action, Step};
///
/// fn twice<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, KBundle>) -> Action<'graph, KBundle> {
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
/// use koan::program::KBundle;
/// use koan::scheduler::{Action, Step};
///
/// fn state_once<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, KBundle>) -> Action<'graph, KBundle> {
///     let (step, _) = step.state();
///     step.done()
/// }
///
/// fn scratch_once<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, KBundle>) -> Action<'graph, KBundle> {
///     let (step, _) = step.scratch();
///     step.done()
/// }
/// ```
///
/// A second take of the state does not compile:
///
/// ```compile_fail,E0599
/// use koan::program::KBundle;
/// use koan::scheduler::{Action, Step};
///
/// fn state_twice<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, KBundle>) -> Action<'graph, KBundle> {
///     let (step, _) = step.state();
///     let (step, _) = step.state();
///     step.done()
/// }
/// ```
///
/// Nor does a second take of the scratch state:
///
/// ```compile_fail,E0599
/// use koan::program::KBundle;
/// use koan::scheduler::{Action, Step};
///
/// fn scratch_twice<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, KBundle>) -> Action<'graph, KBundle> {
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
    'lent,
    B: StepBundle<'graph>,
    S: Hold = Holding,
    X: Hold = Holding,
> where
    'graph: 'step + 'here + 'lent,
    'here: 'scratch,
    'lent: 'scratch,
{
    context: &'a mut Context<'graph, 'step, 'here, 'scratch, 'lent, B>,
    /// The frozen type root of the graph this step runs over, which every chain it builds reads
    /// first.
    type_root: &'graph Root<'graph>,
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

impl<'a, 'graph, 'step, 'here, 'scratch, 'lent, B: StepBundle<'graph>>
    Step<'a, 'graph, 'step, 'here, 'scratch, 'lent, B>
where
    'graph: 'step + 'here + 'lent,
    'here: 'scratch,
    'lent: 'scratch,
{
    /// The step the drain hands a cell's continuation, over its graph's type root, the provenance
    /// it read off that same continuation, the buffer it cleared for this step, and both slots the
    /// cell parked in.
    pub(super) fn new(
        context: &'a mut Context<'graph, 'step, 'here, 'scratch, 'lent, B>,
        type_root: &'graph Root<'graph>,
        provenance: &'a Provenance,
        spawns: &'a mut Spawns<'graph, B>,
        state: StateAt<'graph, 'here, B>,
        scratch: Option<ScratchAt<'graph, 'here, 'scratch, B>>,
    ) -> Self {
        Step {
            context,
            type_root,
            provenance,
            spawns,
            state,
            scratch,
        }
    }
}

impl<'a, 'graph, 'step, 'here, 'scratch, 'lent, B: StepBundle<'graph>, X: Hold>
    Step<'a, 'graph, 'step, 'here, 'scratch, 'lent, B, Holding, X>
where
    'graph: 'step + 'here + 'lent,
    'here: 'scratch,
    'lent: 'scratch,
{
    /// The state this cell holds: the one the previous step parked with, or the one the cell was
    /// born with, woken to this step's `'here` at the verdict's price. It comes back beside the
    /// `Step` in its [`Taken`] form, which has no `state` to call again; a park hands the next
    /// state back through [`park`](Step::park).
    pub fn state(
        self,
    ) -> (
        Step<'a, 'graph, 'step, 'here, 'scratch, 'lent, B, Taken, X>,
        StateAt<'graph, 'here, B>,
    ) {
        let Step {
            context,
            type_root,
            provenance,
            spawns,
            state,
            scratch,
        } = self;
        let step = Step {
            context,
            type_root,
            provenance,
            spawns,
            state: (),
            scratch,
        };
        (step, state)
    }
}

impl<'a, 'graph, 'step, 'here, 'scratch, 'lent, B: StepBundle<'graph>, S: Hold>
    Step<'a, 'graph, 'step, 'here, 'scratch, 'lent, B, S, Holding>
where
    'graph: 'step + 'here + 'lent,
    'here: 'scratch,
    'lent: 'scratch,
{
    /// The scratch state the previous step parked, if it parked one, beside the `Step` in its
    /// [`Taken`] form, like [`state`](Step::state). A step that parks again hands it, or its
    /// successor, back to [`park`](Step::park), and one that drops it lets this step's end hand the
    /// bump back.
    pub fn scratch(
        self,
    ) -> (
        Step<'a, 'graph, 'step, 'here, 'scratch, 'lent, B, S, Taken>,
        Option<ScratchAt<'graph, 'here, 'scratch, B>>,
    ) {
        let Step {
            context,
            type_root,
            provenance,
            spawns,
            state,
            scratch,
        } = self;
        let step = Step {
            context,
            type_root,
            provenance,
            spawns,
            state,
            scratch: (),
        };
        (step, scratch)
    }
}

impl<'a, 'graph, 'step, 'here, 'scratch, 'lent, B: StepBundle<'graph>, S: Hold, X: Hold>
    Step<'a, 'graph, 'step, 'here, 'scratch, 'lent, B, S, X>
where
    'graph: 'step + 'here + 'lent,
    'here: 'scratch,
    'lent: 'scratch,
{
    /// This cell's region, at its own brand.
    pub fn writer(&self) -> Writer<'here, 'step> {
        self.context.writer()
    }

    /// This cell's scratch habitat, at its own brand.
    pub fn scratch_writer(&self) -> Writer<'scratch, 'step> {
        self.context.scratch_writer()
    }

    /// The header door of this cell's write home — its region, or its host's for a tenant — at
    /// its own brand: where the branch of the types the step builds there rests.
    pub fn header(&self) -> Header<'graph, 'here, 'step, TypeHeader> {
        self.context.header()
    }

    /// The chain this step reads and interns types through for what it lays at `'here`: the
    /// graph's type root, then its write home's branch. Built once per step, never parked.
    pub fn types(&self) -> Types<'graph, 'here, 'step> {
        Types::new(
            self.type_root,
            Halves::alone(Half::of(self.writer(), self.header())),
        )
    }

    /// The chain for what the step lays in its scratch habitat: the graph's type root, its write
    /// home's branch, then the scratch habitat's own, released with it at the step's end.
    pub fn scratch_types(&self) -> Chain<'graph, 'scratch, 'scratch, 'here, 'step> {
        Chain::scratch(
            self.type_root,
            Half::paired(self.writer(), self.header()),
            Half::of(self.scratch_writer(), self.context.scratch_header()),
        )
    }

    /// The chain a `Shares` caching body runs on: [`scratch_types`](Self::scratch_types)'s tiers,
    /// paired to this step's region writer, so a value laid at `'here` takes its types from the
    /// write home's scratch and nothing the body mints lands in its region.
    pub fn shares_types(&self) -> Chain<'graph, 'scratch, 'here, 'here, 'step> {
        Chain::scratch(
            self.type_root,
            Half::of(self.writer(), self.header()),
            Half::paired(self.scratch_writer(), self.context.scratch_header()),
        )
    }

    /// Hold this cell's scratch for the rest of its life: `true` for a tree cell, whose own
    /// scratch a `Fresh` caching frame keeps its cold types in; `false`, changing nothing, for a
    /// tenant, whose scratch is its host's.
    pub fn hold_scratch(&mut self) -> bool {
        self.context.hold_scratch()
    }

    /// Link this cell's scratch branch to its region branch, laying both, so the
    /// [reading chain](Self::reading_types) reaches the region through the link. Called once, by a
    /// `Fresh` caching frame right after its [hold](Self::hold_scratch), before anything lays its
    /// scratch branch: the link refuses a laid branch.
    pub fn link_reading_types(&self) {
        let Own::Branch(region) = Half::of(self.writer(), self.header()).own_or_lay() else {
            unreachable!("a step's region half lays a branch");
        };
        Chain::new(
            self.type_root,
            Halves::alone(Half::paired(
                self.scratch_writer(),
                self.context.scratch_header(),
            )),
        )
        .link(region);
    }

    /// The half this cell borrows, whichever it is: a specialization's region, or the scratch of
    /// the cell its asker parked in. `None` for a cell with no lender.
    fn lent(
        &self,
    ) -> Option<(
        Writer<'lent, 'step>,
        Header<'graph, 'lent, 'step, TypeHeader>,
    )> {
        (self.context.lent_region()).or_else(|| self.context.lent_scratch())
    }

    /// The chain a held `Fresh` caching frame's own body reads and lays through: the graph's type
    /// root, the lent branch — its specialization's region, or the scratch of the cell its asker
    /// parked in — then this cell's own scratch, whose branch links its region. Paired to the
    /// region writer. `None` for a cell with no lender.
    pub fn reading_types(&self) -> Option<Chain<'graph, 'scratch, 'here, 'lent, 'step>> {
        let (lent_writer, lent_header) = self.lent()?;
        let scratch_header = self.context.scratch_header();
        debug_assert!(
            scratch_header.get().is_some(),
            "a held frame's scratch branch is laid by its link"
        );
        Some(Chain::scratch(
            self.type_root,
            Half::paired(lent_writer, lent_header),
            Half::paired(self.scratch_writer(), scratch_header),
        ))
    }

    /// The warm tier alone, over the graph's type root: the lent branch, which outlives every hop
    /// born under the same specialization, or under the asker's park. What an invariant site's
    /// types are adopted into, so the next hop finds them. Paired to the region writer; `None` for
    /// a cell with no lender.
    pub fn warm_types(&self) -> Option<Chain<'graph, 'lent, 'here, 'lent, 'step>> {
        let (lent_writer, lent_header) = self.lent()?;
        Some(Chain::new(
            self.type_root,
            Halves::alone(Half::paired(lent_writer, lent_header)),
        ))
    }

    /// Ask for one child, and take back the slot of this step's run it will report to. The drain
    /// creates it after the step returns, under this cell.
    ///
    /// The child's birth is handed over as this step holds it, at `'here`, and put to rest here as
    /// a carrier in this cell; the child's first entry wakes it at its own `'here`.
    pub fn spawn(&mut self, request: Request<'graph, 'here, B>) -> Slot {
        self.spawn_lending(request, Lending::Parent)
    }

    /// [`spawn`](Self::spawn) a frame born borrowing the specialization `seek` finds: the drain
    /// looks it up, or builds it, before the child is born. A `Shares` child is a tenant and
    /// borrows through its host, so `seek` is not read.
    pub fn spawn_seeking(
        &mut self,
        request: Request<'graph, 'here, B>,
        seek: Seek<'graph, B>,
    ) -> Slot {
        self.spawn_lending(request, Lending::Seek(seek))
    }

    fn spawn_lending(
        &mut self,
        request: Request<'graph, 'here, B>,
        lending: Lending<'graph, B>,
    ) -> Slot {
        let state = self.rest(request.work.state);
        self.spawns.push(Asked {
            placement: request.placement,
            use_: request.use_,
            step: request.work.step,
            state,
            lending,
        })
    }

    /// The build cell this cell's frame runs under: the specialization whose region it borrows,
    /// where it is a tree cell born borrowing one, and `fallback` otherwise. A tenant borrows
    /// through its host, so it always gets `fallback`.
    pub fn build_ref(&self, fallback: BuildRef) -> BuildRef {
        let CellHandle::Tree(_) = self.context.cell() else {
            return fallback;
        };
        (self.context.lent_region())
            .and_then(|(_, header)| header.get()?.me())
            .map_or(fallback, BuildRef::of)
    }

    /// The results of the children this cell parked on, in the order it asked for them: a scratch
    /// fill at `'scratch`, a carrier fill redeemed and crossed to `'here`, its types made to
    /// resolve over the graph's type root. Each slot is taken as it is read, so a second pass finds
    /// it empty.
    pub fn results(
        &mut self,
    ) -> impl Iterator<Item = Result<Received<'graph, 'here, 'scratch>, StepError>> {
        self.results_against(None)
    }

    /// [`results`](Self::results), a pinned carrier's debug check reading `check`: the chain of
    /// a caching body, whose children's values carry types in its scratch tiers.
    pub fn results_with<'c>(
        &'c mut self,
        check: &'c Chain<'_, '_, '_, '_, '_>,
    ) -> impl Iterator<Item = Result<Received<'graph, 'here, 'scratch>, StepError>> + 'c {
        self.results_against(Some(check))
    }

    fn results_against<'c>(
        &'c mut self,
        check: Option<&'c Chain<'_, '_, '_, '_, '_>>,
    ) -> impl Iterator<Item = Result<Received<'graph, 'here, 'scratch>, StepError>> + 'c {
        let root = self.type_root;
        let context = &mut *self.context;
        let count = context.receipt_count().unwrap_or(0);
        (0..count).map(move |slot| match context.receipt(slot) {
            Ok(Receipt::Value(value)) => Ok(Received::Scratch(value)),
            Ok(Receipt::Carrier(Ok(carrier))) => Ok(Received::Here(crate::values::cross_here(
                context, &carrier, root, check,
            ))),
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
    pub fn tail(self, placement: Placement, work: Work<'graph, 'here, B>) -> Action<'graph, B> {
        self.tail_lending(placement, work, Lending::Parent)
    }

    /// [`tail`](Self::tail) into a frame born borrowing the specialization `seek` finds, as
    /// [`spawn_seeking`](Self::spawn_seeking) asks for a child.
    pub fn tail_seeking(
        self,
        placement: Placement,
        work: Work<'graph, 'here, B>,
        seek: Seek<'graph, B>,
    ) -> Action<'graph, B> {
        self.tail_lending(placement, work, Lending::Seek(seek))
    }

    /// [`tail`](Self::tail) into a frame of the same callee as the frame running under `build`,
    /// born borrowing that same specialization with no lookup; or its parent's scratch, where that
    /// specialization is no longer live.
    pub fn tail_under(
        self,
        placement: Placement,
        work: Work<'graph, 'here, B>,
        build: BuildRef,
    ) -> Action<'graph, B> {
        self.tail_lending(placement, work, Lending::Under(build))
    }

    fn tail_lending(
        mut self,
        placement: Placement,
        work: Work<'graph, 'here, B>,
        lending: Lending<'graph, B>,
    ) -> Action<'graph, B> {
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
            lending,
        }))
    }

    /// End a specialization's step: enter this cell in its parent's table under `key`, laying the
    /// table on the first entry and renewing the key's entry where it has one, and record in its own
    /// region header that the region is this specialization. The cell stays alive with no
    /// continuation. Refused for a cell that asked for children, and for one that borrows no
    /// region.
    pub fn enlist(self, key: SpecKey) -> Action<'graph, B> {
        if !self.spawns.is_empty() {
            return self.failed(StepError::Unparked);
        }
        let (CellHandle::Tree(me), Some((lent_writer, lent_header))) =
            (self.context.cell(), self.context.lent_region())
        else {
            return self.failed(StepError::Refused);
        };
        let parent = Half::of(lent_writer, lent_header);
        let _ = parent.own_or_lay();
        (lent_header.get().expect("the parent's header is laid")).insert(parent, key, me);
        let _ = Half::of(self.writer(), self.header()).own_or_lay();
        (self.header().get().expect("the header is laid")).set_me(me);
        Action(Kind::Quiesced)
    }

    /// Finish with a result built by `build`, which takes no operands, and deliver it where the
    /// drain said.
    ///
    /// The door is the veneer's, from the `Use` this cell was asked with: under `Reads` the value
    /// is built in the consumer's scratch habitat and read back at its `'scratch`; under `Keeps` or
    /// `Forwards` it is built in this cell's home and filed as a carrier the consumer reads at its
    /// `'here`. One build serves both, because it is quantified over the brand it is built at; it
    /// is handed the chain of the half it builds in, over the graph's type root, which reads no
    /// other half's branch, so it takes no handle from this step's chain.
    pub fn finish_fresh(
        self,
        build: impl for<'their> FnOnce(
            Writer<'their, '_>,
            &Types<'graph, 'their, '_>,
            &'their &'graph (),
        ) -> Active<'graph, 'their, KValueFamily>,
    ) -> Action<'graph, B> {
        let root = self.type_root;
        let report = match self.report() {
            Ok(report) => report,
            Err(error) => return self.failed(error),
        };
        if report.use_ == Use::Reads {
            let filled = self.context.deliver_scratch(
                self.provenance.parent,
                report.slot,
                |writer, header, witness| {
                    build(
                        writer,
                        &Types::new(root, Halves::alone(Half::of(writer, header))),
                        witness,
                    )
                },
            );
            return Action::settle(filled);
        }
        let placed = self.context.alloc_into::<KValueFamily, KValueFamily>(
            self.provenance.home,
            &[],
            // The witness is minted here: `CrossedOperand` declares `'graph: 'cell`, so the slice
            // argument's type implies the bound the witness states. Were that where-clause to go,
            // this would stop compiling — a compile error, never unsoundness.
            |writer, header, _: &[CrossedOperand<'graph, '_, '_, KValueFamily, TypeHeader>]| {
                build(
                    writer,
                    &Types::new(root, Halves::alone(Half::of(writer, header))),
                    &&(),
                )
            },
        );
        self.file(report, placed)
    }

    /// Finish with a result built by `build` over `operands`, values this step holds, in this
    /// cell's home, and deliver it as a carrier. Each operand reaches the build pinned or copied as
    /// the verdict ruled, at the home's brand, its types made to resolve there over the graph's
    /// type root; the build is handed the home's chain beside its writer. Every `Use` takes this
    /// door: a build over operands never goes through scratch, where what it embeds could not
    /// follow.
    pub fn finish_in_home<const N: usize>(
        self,
        operands: [KValue<'graph, 'here>; N],
        build: impl for<'their> FnOnce(
            Writer<'their, '_>,
            &Types<'graph, 'their, '_>,
            [KValue<'graph, 'their>; N],
            &'their &'graph (),
        ) -> Active<'graph, 'their, KValueFamily>,
    ) -> Action<'graph, B> {
        self.finish_in_home_against(operands, None, build)
    }

    fn finish_in_home_against<const N: usize>(
        self,
        operands: [KValue<'graph, 'here>; N],
        check: Option<&Chain<'_, '_, '_, '_, '_>>,
        build: impl for<'their> FnOnce(
            Writer<'their, '_>,
            &Types<'graph, 'their, '_>,
            [KValue<'graph, 'their>; N],
            &'their &'graph (),
        ) -> Active<'graph, 'their, KValueFamily>,
    ) -> Action<'graph, B> {
        let report = match self.report() {
            Ok(report) => report,
            Err(error) => return self.failed(error),
        };
        let root = self.type_root;
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
            |writer, header, views| {
                let types = Types::new(root, Halves::alone(Half::of(writer, header)));
                let values = std::array::from_fn(|index| {
                    crate::values::cross_view_against(writer, &types, &views[index], check)
                });
                build(writer, &types, values, &&())
            },
        );
        self.file(report, placed)
    }

    /// Finish with `value`, crossed into this cell's home at the verdict's price, its types made
    /// to resolve there over the graph's type root, and deliver it as a carrier.
    pub fn finish(self, value: KValue<'graph, 'here>) -> Action<'graph, B> {
        self.finish_in_home([value], |_, _, [value], _| Active::new(value))
    }

    /// [`finish`](Self::finish), a pinned value's debug check reading `check`: the chain of a
    /// caching body finishing a value that stays inside it, whose types lie in its scratch tiers.
    pub fn finish_with(
        self,
        value: KValue<'graph, 'here>,
        check: &Chain<'_, '_, '_, '_, '_>,
    ) -> Action<'graph, B> {
        self.finish_in_home_against([value], Some(check), |_, _, [value], _| Active::new(value))
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
        let root = self.type_root;
        let copy_bytes = B::weight(&birth);
        let lifted = self.context.lift::<B::Birth>(birth);
        let placed = self.context.alloc_into::<B::Birth, B::Birth>(
            self.provenance.home,
            &[Operand {
                carrier: &lifted,
                copy_bytes,
            }],
            |writer, header, views| {
                Active::new(B::cross(writer, header, &views[0], Born::Left, root))
            },
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
    /// What the cell borrows, which the drain resolves at its creation.
    pub lending: Lending<'graph, B>,
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
    /// The layer above refused the work; its reason is that layer's to report.
    Refused,
}
