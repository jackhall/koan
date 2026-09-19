//! What a cell will do when it next runs, and what it carries across a park.
//!
//! The continuation — [`ContinuationFamily`] — re-anchors at the executing cell's region brand and
//! is what the drain reads to run a step. The scratch state — [`ScratchFamily`] — is a family over
//! **both** step brands, so what it carries names storage at `'here` and the habitat at
//! `'scratch`, each at its own brand. One slot per habitat, so the wrong form is unrepresentable
//! in each.

use crate::function::KValue;
use crate::memory::{Dormant, DropFree, StepContext, reattachable};
use crate::scheduler::action::{Action, Spawns};
use crate::scheduler::delivery::KDelivery;
use crate::scheduler::submit::UnitId;

/// The context a native step runs against: koan's continuation families over koan's delivery
/// bundle, at the brands one [`enter`](crate::memory::CellGraph::enter) quantifies.
pub type Context<'graph, 'step, 'here, 'scratch> =
    StepContext<'graph, 'step, 'here, 'scratch, ContinuationFamily, ScratchFamily, KDelivery>;

/// A native step: a function pointer over the cell's state, the shape a builtin body takes.
///
/// Higher-ranked over the three brands one `enter` quantifies, so one pointer runs in any cell at
/// any step. It is not ranked over `'graph`: the state it reads, the children it describes and the
/// action it returns all name the graph whose cells they belong to.
pub type NativeStep<'graph> = for<'step, 'here, 'scratch> fn(
    &mut Context<'graph, 'step, 'here, 'scratch>,
    Resume<'graph, 'here, 'scratch>,
    &mut Spawns<'graph>,
) -> Action<'graph>;

/// The family of what a cell parks in its storage: its continuation.
pub struct ContinuationFamily;

reattachable!(ContinuationFamily => Continuation<'graph, 'cell>);

impl DropFree for ContinuationFamily {}

/// What a cell will do when it next runs: a step, its place in the graph, and the state it runs
/// over.
///
/// A *birth* continuation is this at `'cell = 'graph`, so its state holds only words, program
/// storage and dormant carriers; a continuation a step stores for itself may hold region borrows
/// freely, because the brand it comes back at is the same one it was minted under.
pub enum Continuation<'graph, 'cell> {
    /// A native step over a state value. The rewrite's second layer adds a component arm beside
    /// this one, and nothing else.
    Native {
        step: NativeStep<'graph>,
        provenance: Provenance,
        state: State<'graph, 'cell>,
    },
}

/// The work one cell is born to run: the step it starts at, and what it starts holding.
///
/// The two things that describe a birth — a [`Request`] and a [`Unit`] — each name a place and
/// this. It is what the drain turns into a continuation, in the one place any continuation is
/// built.
///
/// [`Request`]: crate::scheduler::Request
/// [`Unit`]: crate::scheduler::Unit
#[derive(Clone, Copy)]
pub struct Work<'graph> {
    /// The step the cell runs first.
    pub step: NativeStep<'graph>,
    /// What it is born holding, at `'graph` — its arguments reach it as dormant carriers.
    pub state: State<'graph, 'graph>,
}

impl<'graph> Work<'graph> {
    /// This work as a birth continuation, under the provenance the drain filled for it. Every
    /// cell the drain creates is created from one of these, so the drain names no step and no
    /// birth state of its own.
    pub(crate) fn continuation(self, provenance: Provenance) -> Continuation<'graph, 'graph> {
        Continuation::Native {
            step: self.step,
            provenance,
            state: self.state,
        }
    }
}

/// What the drain hands a native step: everything both slots held but the step pointer itself.
pub struct Resume<'graph, 'here, 'scratch> {
    /// The cell's place in the graph, to be carried into any successor the step stores or asks for.
    pub provenance: Provenance,
    /// What the previous step left in storage, or what the cell was born with.
    pub state: State<'graph, 'here>,
    /// What the previous step parked in the scratch habitat, taken off the cell. A step that parks
    /// again hands this, or its successor, back to
    /// [`Action::park`](crate::scheduler::Action::park); one that leaves it here lets this step's
    /// end hand the bump back.
    pub scratch: Option<ScratchState<'graph, 'here, 'scratch>>,
}

impl<'graph> Resume<'graph, '_, '_> {
    /// This cell's next step over `state`, under the provenance the drain handed in. The one place
    /// a step-side continuation gets its provenance, so no step invents one of its own; a park
    /// goes through it by way of [`Action::park`](crate::scheduler::Action::park).
    ///
    /// The brand is the stored successor's, not this resume's: what a step carries forward is
    /// written at the brand it will come back at.
    pub fn successor<'here>(
        &self,
        step: NativeStep<'graph>,
        state: State<'graph, 'here>,
    ) -> Continuation<'graph, 'here> {
        Continuation::Native {
            step,
            provenance: self.provenance,
            state,
        }
    }
}

/// What a native step runs over.
#[derive(Clone, Copy)]
pub enum State<'graph, 'cell> {
    /// Nothing at rest: the step runs on its pointer alone.
    Empty,
    /// A value the previous step left, or the cell was born holding.
    Value(KValue<'graph, 'cell>),
    /// A value at rest in the cell a hop or a spawn came from, to be redeemed and copied in.
    Parked(Dormant<'graph, crate::function::KValueFamily>),
}

/// What a cell carries about itself for the drain's use: where it sits, where its result goes, and
/// which submission it answers for.
///
/// Every field is brand-free, so it survives a tail hop verbatim.
#[derive(Clone, Copy)]
pub struct Provenance {
    /// The tree parent a `Fresh` successor is born a sibling under, or the host a `Shares`
    /// successor is born a tenant of. `cellgraph` exposes no parent accessor, so a cell remembers
    /// its place here rather than in a table beside the graph.
    pub place: CellPlace,
    /// Where this cell's result goes: its consumer's handle and the receipt slot it fills. A tail
    /// hop hands this to its successor unchanged, which is what "the successor inherits its
    /// receipt" means.
    pub destination: Option<Destination>,
    /// The submission this cell completes, for a cell the drain created out of the table. The
    /// dependents of that unit are released when the cell finishes — after a chain of tail hops,
    /// by whichever successor finishes it, because this travels with the work.
    pub unit: Option<UnitId>,
}

/// Where a cell was born, and so where a sibling or co-tenant of it is born.
#[derive(Clone, Copy)]
pub enum CellPlace {
    /// A slab cell: under nothing, so it has no successor place but the slab.
    Slab,
    /// A tree child of this parent, or a tenant of this host.
    Under(crate::memory::CellHandle),
}

/// One slot of one consumer's receipt run: where a producer's result goes. Named apart from
/// [`crate::memory::Receipt`], which is what the consumer reads back out of that slot.
#[derive(Clone, Copy)]
pub struct Destination {
    pub consumer: crate::memory::CellHandle,
    pub slot: usize,
}

/// The family of what a cell parks in its scratch habitat, over both step brands.
pub struct ScratchFamily;

reattachable!(both ScratchFamily => ScratchState<'graph, 'here, 'scratch>);

/// A parked cell's in-progress state, held in its scratch habitat across the park: scratch at
/// `'scratch`, and storage at `'here`.
///
/// The slot is an `Option`, so an empty park is the absence of one of these rather than an arm.
#[derive(Clone, Copy)]
pub enum ScratchState<'graph, 'here, 'scratch> {
    /// A value built in the habitat that the woken step reads back.
    Value(KValue<'graph, 'scratch>),
    /// Values at rest in the cell's storage, gathered by a run in the habitat. Each is held at
    /// `'here`, so the step that builds in storage embeds it as it is — no `keep` at the park and
    /// no `redeem` at the wake.
    Gathered(&'scratch [KValue<'graph, 'here>]),
}
