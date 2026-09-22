//! What a cell will do when it next runs, and what it carries across a park.
//!
//! The continuation — [`ContinuationFamily`] — re-anchors at the executing cell's region brand and
//! is what the drain reads to run a step. The scratch state — [`ScratchFamily`] — is a family over
//! **both** step brands, so what it carries names storage at `'here` and the habitat at
//! `'scratch`, each at its own brand. One slot per habitat, so the wrong form is unrepresentable
//! in each.
//!
//! The families, the continuation and the provenance inside it are the scheduler's own: a step
//! reaches neither slot, and parks through [`Step::park`], which builds the continuation itself.

use crate::knot::KValue;
use crate::memory::{Dormant, DropFree, reattachable};
use crate::scheduler::action::{Action, Step};
use crate::scheduler::submit::UnitId;

/// A native step: a function pointer over the cell's state, the shape a builtin body takes.
///
/// It is handed its [`Step`] by value — every door it may use, and the only way to end — and both
/// slots the cell parked in: the [`State`] the previous step left in storage, or the one the cell
/// was born with, and the scratch state the previous step parked in the habitat. A step that parks
/// again hands that scratch state, or its successor, back to [`Step::park`]; one that drops it lets
/// this step's end hand the bump back.
///
/// Higher-ranked over the three brands one `enter` quantifies, so one pointer runs in any cell at
/// any step. It is not ranked over `'graph`: the state it reads, the children it describes and the
/// action it returns all name the graph whose cells they belong to.
pub type NativeStep<'graph> = for<'step, 'here, 'scratch> fn(
    Step<'_, 'graph, 'step, 'here, 'scratch>,
    State<'graph, 'here>,
    Option<ScratchState<'graph, 'here, 'scratch>>,
) -> Action<'graph>;

/// The family of what a cell parks in its storage: its continuation.
pub(super) struct ContinuationFamily;

reattachable!(ContinuationFamily => Continuation<'graph, 'cell>);

impl DropFree for ContinuationFamily {}

/// What a cell will do when it next runs: a step, its place in the graph, and the state it runs
/// over.
///
/// A *birth* continuation is this at `'cell = 'graph`, so its state holds only words, program
/// storage and dormant carriers; the successor [`Step::park`] stores may hold region borrows freely,
/// because the brand it comes back at is the same one it was minted under.
pub(super) enum Continuation<'graph, 'cell> {
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
    pub(super) fn continuation(self, provenance: Provenance) -> Continuation<'graph, 'graph> {
        Continuation::Native {
            step: self.step,
            provenance,
            state: self.state,
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
    Parked(Dormant<'graph, crate::knot::KValueFamily>),
}

/// What a cell carries about itself for the drain's use: where it sits, where its result goes, and
/// which submission it answers for.
///
/// Every field is brand-free, so it survives a tail hop verbatim. The drain fills it and a step
/// never sees it: a [`Step`] holds the cell's copy privately, to park under and to deliver to.
#[derive(Clone, Copy)]
pub(super) struct Provenance {
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
pub(super) enum CellPlace {
    /// A slab cell: under nothing, so it has no successor place but the slab.
    Slab,
    /// A tree child of this parent, or a tenant of this host.
    Under(crate::memory::CellHandle),
}

/// One slot of one consumer's receipt run: where a producer's result goes. Named apart from
/// [`crate::memory::Receipt`], which is what the consumer reads back out of that slot.
#[derive(Clone, Copy)]
pub(super) struct Destination {
    pub consumer: crate::memory::CellHandle,
    pub slot: usize,
}

/// The family of what a cell parks in its scratch habitat, over both step brands.
pub(super) struct ScratchFamily;

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
