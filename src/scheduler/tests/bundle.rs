//! The test bundle: what the scheduler's tests run over, with no layer of koan's above present.
//!
//! A cell's state is a value, and its scratch state is either a value built in the habitat or a
//! run gathered there over values at rest in storage — so the tests drive both of the scratch
//! family's positions.
//!
//! The crossing doors read a type registry, and a test step, a bare `fn`, has no program to take
//! one from, so each thread keeps one of its own ([`with_types`]). The tests build no container,
//! so a crossing never reads it.

use self_cell::self_cell;

use crate::knot::{KValue, KValueFamily};
use crate::memory::{Bump, CrossedOperand, Writer, reattachable};
use crate::scheduler::{Graph, NativeStep, StepBundle};
use crate::type_lattice::TypeRegistry;

self_cell!(
    /// A type registry over a bump of its own.
    struct TestTypes {
        owner: Bump,
        #[not_covariant]
        dependent: TypeRegistry,
    }
);

thread_local! {
    static TYPES: TestTypes = TestTypes::new(Bump::new(), |bump| TypeRegistry::in_region(bump));
}

/// Run `body` over this thread's test registry.
pub fn with_types<R>(body: impl for<'run> FnOnce(&TypeRegistry<'run>) -> R) -> R {
    TYPES.with(|types| types.with_dependent(|_, registry| body(registry)))
}

/// The bundle the scheduler's tests run over.
pub struct Native;

impl<'graph> StepBundle<'graph> for Native {
    type Birth = KValueFamily;
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
        with_types(|types| crate::values::cross_view(writer, view, types))
    }

    fn born<'cell>(birth: KValue<'graph, 'cell>) -> KValue<'graph, 'cell>
    where
        'graph: 'cell,
    {
        birth
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
