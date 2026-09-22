//! What a cell will do when it next runs, the work it is born to run, and the step bundle both are
//! over.
//!
//! The scheduler names no state and no step of the layers above it: it takes them as one
//! [`StepBundle`] — the family of a cell's state at rest in storage, the family of what a parked
//! cell keeps in its scratch habitat, and how the state crosses between cells. The continuation —
//! [`ContinuationFamily`] — re-anchors at the executing cell's region brand and is what the drain
//! reads to run a step; it is one shape with no arm to add.
//!
//! The continuation family and the provenance inside it are the scheduler's own: a step reaches
//! neither slot, and parks through [`Step::park`], which builds the continuation itself.

use std::marker::PhantomData;

use crate::memory::{
    CellHandle, Covariant, CrossedOperand, Dormant, DropFree, Reattachable, ReattachableOverBoth,
    Writer, reattachable,
};
use crate::scheduler::action::{Action, Step, Use};

/// What a scheduler runs: the layer above it, as one bundle — the family of a cell's state at rest
/// in storage, the family of what a parked cell keeps in its scratch habitat, and how the state
/// crosses between cells. The scheduler names no state and no step of its own.
///
/// Where the scheduler wakes a state it also needs `Erased<'graph, Self::State>: Copy`, which a
/// trait cannot state for its users; every concrete bundle whose state family is `Copy` meets it.
pub trait StepBundle<'graph>: 'graph {
    /// The family of a cell's state, at rest in storage over one brand. Covariant, because a state
    /// crosses pinned from its spawner into the child.
    type State: Reattachable<'graph> + Covariant<'graph> + DropFree;
    /// The family of a parked cell's in-progress state, over both step brands.
    type Scratch: ReattachableOverBoth<'graph>;

    /// How many bytes a copy of `state` writes — what the verdict prices its crossing by.
    fn weight<'cell>(state: &<Self::State as Reattachable<'graph>>::At<'cell>) -> usize
    where
        'graph: 'cell;

    /// `view` at the destination's brand: the pinned view as it is, a copied view rebuilt through
    /// `writer`. The one place above the scheduler a state is copied, and the veneer calls it only
    /// from inside a priced crossing.
    fn cross<'cell>(
        writer: Writer<'cell>,
        view: &CrossedOperand<'graph, 'cell, '_, Self::State>,
    ) -> <Self::State as Reattachable<'graph>>::At<'cell>
    where
        'graph: 'cell;
}

/// A native step: a function pointer over a [`Step`], the shape a builtin body takes.
///
/// It is handed its `Step` by value — every door it may use, the state its cell holds and the
/// scratch state it parked, and the only way to end. A state is a projection of the bundle, which a
/// higher-ranked function pointer cannot take as a parameter (rustc #100013), so the step takes
/// both from the `Step`, whose [`Hold`](crate::scheduler::Hold) markers — lifetime-free, so they
/// may appear here — default to holding both.
///
/// Higher-ranked over the three brands one `enter` quantifies, so one pointer runs in any cell at
/// any step. It is not ranked over `'graph`: the state it reads, the children it asks for and the
/// action it returns all name the graph whose cells they belong to.
pub type NativeStep<'graph, B> = for<'step, 'here, 'scratch> fn(
    Step<'_, 'graph, 'step, 'here, 'scratch, B>,
) -> Action<'graph, B>;

/// The work one cell is born to run: the step it starts at, and what it starts holding.
pub struct Work<'graph, 'cell, B: StepBundle<'graph>>
where
    'graph: 'cell,
{
    /// The step the cell runs first.
    pub step: NativeStep<'graph, B>,
    /// What the cell starts holding, at the brand of whoever hands it over: a spawner's or a
    /// predecessor's `'here`, or `'graph` for a root work.
    pub state: <B::State as Reattachable<'graph>>::At<'cell>,
}

/// The family of what a cell parks in its storage: its continuation.
pub(super) struct ContinuationFamily<B>(PhantomData<B>);

reattachable!(ContinuationFamily<B: StepBundle<'graph>> => Continuation<'graph, 'cell, B>);

/// What a cell will do when it next runs: one shape, no arm to add — the step, the cell's
/// provenance, and the bundle's state.
///
/// A *birth* continuation is this at `'cell = 'graph`, because every birth door takes one there:
/// its state is a root work's, at `'graph`, or at rest as a carrier. The successor [`Step::park`]
/// stores is at `'here` and may hold region borrows freely, because the brand it comes back at is
/// the same one it was minted under.
pub(super) struct Continuation<'graph, 'cell, B: StepBundle<'graph>>
where
    'graph: 'cell,
{
    pub step: NativeStep<'graph, B>,
    pub provenance: Provenance,
    pub state: Rested<'graph, 'cell, B>,
}

/// How a continuation holds its state.
pub(super) enum Rested<'graph, 'cell, B: StepBundle<'graph>>
where
    'graph: 'cell,
{
    /// Awake at the brand it was stored at: a successor [`Step::park`] stores, or a root work's
    /// birth state at `'graph`.
    Awake(<B::State as Reattachable<'graph>>::At<'cell>),
    /// At rest as a carrier: what a spawn or a tail hop hands the new cell, woken by its first
    /// entry at the verdict's price.
    Dormant(Dormant<'graph, B::State>),
}

/// What a cell carries about itself for the drain's use: where it sits, where its result goes and
/// what it is for, and where the result is built.
///
/// Every field is brand-free, so it survives a tail hop verbatim — which is what "the successor
/// inherits its receipt" means. The drain fills it and a step never sees it: a [`Step`] holds the
/// cell's copy privately, to park under and to deliver to.
#[derive(Clone, Copy)]
pub(super) struct Provenance {
    /// The cell this one was born under — a tree parent or a tenant's host — which is also the
    /// consumer it reports to and the place a successor is born beside. `cellgraph` exposes no
    /// parent accessor, so a cell remembers its place here rather than in a table beside the graph.
    pub parent: CellHandle,
    /// The slot of the parent's receipt run this cell fills and the `Use` it was asked with.
    /// `None` for a root work, which reports to nobody.
    pub report: Option<Report>,
    /// Where this cell's result is built.
    pub home: CellHandle,
}

/// One slot of one consumer's receipt run, and what the consumer will do with what fills it.
#[derive(Clone, Copy)]
pub(super) struct Report {
    pub slot: usize,
    pub use_: Use,
}
