//! The test bundle: what the scheduler's tests run over, with no layer of koan's above present.
//!
//! A cell's state is a value, and its scratch state is either a value built in the habitat or a
//! run gathered there over values at rest in storage — so the tests drive both of the scratch
//! family's positions.

use crate::knot::{KValue, KValueFamily};
use crate::memory::{CrossedOperand, Writer, reattachable};
use crate::scheduler::{Graph, NativeStep, StepBundle};

/// The bundle the scheduler's tests run over.
pub struct Native;

impl<'graph> StepBundle<'graph> for Native {
    type State = KValueFamily;
    type Scratch = ScratchFamily;

    fn weight<'cell>(state: &KValue<'graph, 'cell>) -> usize
    where
        'graph: 'cell,
    {
        state.weight().bytes()
    }

    fn cross<'cell>(
        writer: Writer<'cell>,
        view: &CrossedOperand<'graph, 'cell, '_, KValueFamily>,
    ) -> KValue<'graph, 'cell>
    where
        'graph: 'cell,
    {
        crate::values::cross_view(writer, view)
    }
}

/// A native step over the test bundle.
pub type TestStep<'graph> = NativeStep<'graph, Native>;

/// A graph over the test bundle.
pub type TestGraph<'graph> = Graph<'graph, Native>;

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
    /// `'here`, so the step that builds in storage embeds it as it is — no crossing at the park and
    /// none at the wake.
    Gathered(&'scratch [KValue<'graph, 'here>]),
}
