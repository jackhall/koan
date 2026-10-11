//! The test bundle: what the scheduler's tests run over, with no layer of koan's above present.
//!
//! A cell's state is a value, and its scratch state is either a value built in the habitat or a
//! run gathered there over values at rest in storage — so the tests drive both of the scratch
//! family's positions.
//!
//! The crossing doors read types over a frozen root, and a test step, a bare `fn`, has no program
//! to take one from, so each thread keeps a seeded root of its own ([`test_root`]), leaked for the
//! process's life and anchored in a static, so Miri's leak check reads it as reachable.

use std::ptr;
use std::sync::Mutex;
use std::sync::atomic::AtomicPtr;

use crate::knot::{KValue, KValueFamily};
use crate::memory::{Bump, CrossedOperand, Header, Writer, reattachable};
use crate::scheduler::{Born, Graph, NativeStep, StepBundle};
use crate::type_lattice::Root;
use crate::values::{Half, TypeHeader, Types};

/// Every thread's root bump, held for the process's life: the root a test step reads is `'static`,
/// so its bump is never released, and a pointer to it here is what keeps that from being a leak.
static ANCHORS: Mutex<Vec<AtomicPtr<Bump>>> = Mutex::new(Vec::new());

thread_local! {
    static ROOT: &'static Root<'static> = {
        let bump: &'static Bump = Box::leak(Box::new(Bump::new()));
        let anchor = AtomicPtr::new(ptr::from_ref(bump).cast_mut());
        ANCHORS.lock().expect("no thread panics holding the anchors").push(anchor);
        Root::seeded(bump)
    };
}

/// This thread's seeded root, which every test step's chain reads first.
pub fn test_root() -> &'static Root<'static> {
    ROOT.with(|root| *root)
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

    /// As koan's own bundle crosses a birth: a pinned birth links its home's branch, or merges it
    /// where a hop's or a left birth's branch already links one; a copied one goes through the
    /// value crossing.
    fn cross<'cell>(
        writer: Writer<'cell>,
        header: Header<'graph, 'cell, TypeHeader>,
        view: &CrossedOperand<'graph, 'cell, '_, KValueFamily, TypeHeader>,
        born: Born,
    ) -> KValue<'graph, 'cell>
    where
        'graph: 'cell,
    {
        let types = Types::new(test_root(), Half::of(writer, header));
        match view {
            CrossedOperand::Pinned { view, home, .. } => {
                home.read(|header, _| match born {
                    Born::Child => types.link(header.into_value()),
                    Born::Hop | Born::Left => types.link_or_merge(header.into_value()),
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
