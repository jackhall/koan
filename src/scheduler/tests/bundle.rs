//! The test bundle: what the scheduler's tests run over, with no layer of koan's above present.
//!
//! A cell's state is a value, and its scratch state is either a value built in the habitat or a
//! run gathered there over values at rest in storage — so the tests drive both of the scratch
//! family's positions.
//!
//! A test seeds a root of its own over a local bump and builds its graph over it; the drain hands
//! it to every step and crossing, so a bare `fn` step reads types with no program around it.

use crate::knot::{KValue, KValueFamily};
use crate::memory::{CrossedOperand, Header, Writer, reattachable};
use crate::scheduler::{Action, Born, Graph, NativeStep, Step, StepBundle};
use crate::scope::RegionHeader;
use crate::type_lattice::Halves;
use crate::values::Half;

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

    /// As koan's own bundle crosses a birth: a pinned birth links its home's branch, or merges it
    /// where a hop's or a left birth's half has already laid a branch; a copied one goes through
    /// the value crossing.
    fn cross<'cell>(
        writer: Writer<'cell, '_>,
        header: Header<'graph, 'cell, '_, RegionHeader>,
        view: &CrossedOperand<'graph, 'cell, '_, KValueFamily, RegionHeader>,
        born: Born,
        tiers: crate::values::TypeTiers<'graph>,
    ) -> KValue<'graph, 'cell>
    where
        'graph: 'cell,
    {
        let types = tiers.chain(Halves::alone(Half::of(writer, header)));
        match view {
            CrossedOperand::Pinned { view, home, .. } => {
                home.read(|header, _| {
                    let Some(branch) = header.into_value().branch() else {
                        return;
                    };
                    match born {
                        Born::Child => types.link(branch),
                        Born::Hop | Born::Left => types.link_or_merge(branch),
                    }
                });
                #[cfg(debug_assertions)]
                crate::values::check_pinned_habitat::<crate::knot::KnottedFamily>(&types, view);
                *view
            }
            _ => crate::values::cross_view(writer, &types, view),
        }
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

/// A test step over every graph: what a helper that builds its own graph, over a root it seeds,
/// takes to start a root work at.
pub type AnyStep = for<'graph, 'step, 'here, 'scratch, 'lent> fn(
    Step<'_, 'graph, 'step, 'here, 'scratch, 'lent, Native>,
) -> Action<'graph, Native>;

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
