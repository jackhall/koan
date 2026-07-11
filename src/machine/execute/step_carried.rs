//! The step-scoped brand for the Done-arm value carrier.
//!
//! An empty-witness carrier ([`Witnessed::resident`](crate::witnessed::Witnessed::resident) over a
//! region-pure value) pins nothing: it is sound only as a within-step transient — the run loop's
//! held frame set pins the producing region across the step, and [`finalize_terminal`] folds that
//! frame into the carrier's reach before it is stored on a node. [`StepCarried`] makes that transient
//! a type: the carrier crossing the Done arm ([`Outcome::Done`](super::outcome::Outcome) →
//! [`NodeStep::DoneWitnessed`](super::nodes::NodeStep) → finalize) rides a brand lifetime `'step`
//! that is the step tail's rank-2 open lifetime (`run_loop.rs`), unnameable outside that closure, so
//! the borrow checker rejects any attempt to stash it past its construction step. The one exit to
//! node storage is [`StepCarried::seal_at_step`], which pairs the carrier with its anchor's storage
//! pin and hands it to finalize.
//!
//! [`finalize_terminal`]: super::finalize::NodeFinalize::finalize_terminal

use std::marker::PhantomData;
use std::rc::Rc;

use crate::machine::core::{run_root_storage, FrameStorage, StepAllocator};
use crate::machine::model::values::CarriedFamily;
use crate::machine::CarrierWitness;
use crate::witnessed::{Delivered, Reattachable, Witnessed};

/// A value carrier confined to the scheduler step that built it. The brand lifetime `'step` is the
/// step tail's rank-2 open lifetime (`run_loop.rs`), unnameable outside that closure, so a
/// `StepCarried` cannot be stored past its construction step: the within-step transient invariant,
/// enforced by the borrow checker.
///
/// The `inner` carrier is **private to this module** — that privacy is the mechanism. The sole exit
/// is [`Self::seal_at_step`], which consumes the wrapper into a delivery envelope; no accessor hands
/// back the lifetime-free [`Witnessed`] for a builtin to stash. `PhantomData<&'step ()>` is
/// covariant, matching [`FoldToken`](crate::witnessed::FoldToken): escaping the brand would require
/// *lengthening* `'step`, which covariance forbids and unnameability prevents.
///
/// The generic `T` (default [`CarriedFamily`]) lets other step-confined operands ride the same
/// wrapper.
///
/// The type is `pub` only so the `#[doc(hidden)]` `step_fixture` can drive it from a `compile_fail`
/// external crate (the `machine::execute` module is `pub(crate)`, so it is not part of koan's real
/// API). The confinement rests on [`born`](Self::born) (`pub(crate)`) and
/// [`seal_at_step`](Self::seal_at_step) (`pub(super)`) being unreachable outside the crate, plus the
/// brand lifetime — never on the type being unnameable.
pub struct StepCarried<'step, T: Reattachable = CarriedFamily> {
    inner: Witnessed<T, CarrierWitness>,
    step: PhantomData<&'step ()>,
}

impl<'step, T: Reattachable> StepCarried<'step, T> {
    /// Wrap a carrier into the step brand. Unrestricted in-crate: wrapping only ever *adds*
    /// confinement, so any construction site may brand a carrier it holds. `'step` is inferred from
    /// the context the wrapper flows into — the Done-arm enums ([`Outcome`](super::outcome::Outcome),
    /// [`NodeStep`](super::nodes::NodeStep)) carry it at the step open's rank-2 brand.
    pub(crate) fn born(inner: Witnessed<T, CarrierWitness>) -> Self {
        StepCarried {
            inner,
            step: PhantomData,
        }
    }

    /// The only exit from the step brand: pair the carrier with the anchor's storage pin and hand it
    /// to finalize. `pub(super)` so the seal/finalize sites in [`super`] can call it while
    /// `crate::builtins` cannot — a builtin holding a `StepCarried` cannot strip the brand.
    ///
    /// This door trusts its caller to pass the *right* host (the anchor's owner); binding that free
    /// parameter is a separate concern. The door's contract here is only that it is the unique way a
    /// `StepCarried` reaches node storage.
    pub(super) fn seal_at_step(
        self,
        host: Rc<FrameStorage>,
    ) -> Delivered<T, CarrierWitness, FrameStorage> {
        Delivered::seal(self.inner, host)
    }

    /// Unwrap to the raw carrier for a unit test that inspects a door product's reach directly.
    /// `#[cfg(test)]`-gated, so it is absent from every production build: the compile guarantee AC 1
    /// names (no stash past the step) holds for all non-test code, pinned by the `compile_fail`
    /// fixtures. A `machine::core` door test cannot reach the `pub(super)` [`Self::seal_at_step`]
    /// exit, so this is how it reads back the carrier the door built.
    #[cfg(test)]
    pub(crate) fn into_witnessed_for_test(self) -> Witnessed<T, CarrierWitness> {
        self.inner
    }
}

/// Hand a step allocator to `guard` at a `for<'b>` rank-2 brand — the step tail's confinement shape.
/// Its [`StepAllocator::over_frame`] mint is `pub(in crate::machine)`, so this driver lives here
/// (inside `crate::machine`) rather than in the crate-root `step_fixture`; the fixture re-exports it.
/// `'b` is universally quantified over `guard`, so a guard body can allocate through the allocator's
/// doors but cannot store a door product past the closure (doing so makes `'b` escape — the
/// `compile_fail` pin for the door half of the brand). `#[doc(hidden)]` fixture surface, not real API.
#[doc(hidden)]
pub fn drive_step_allocator(guard: impl for<'b> FnOnce(StepAllocator<'b>)) {
    guard(StepAllocator::over_frame(run_root_storage()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::core::{run_root_storage, FrameStorageExt};
    use crate::machine::model::KObject;

    /// The legal shape: born a region-pure carrier, then exit through the sole seal door into a
    /// delivery envelope pinned by its own storage. Mirrors the run loop's `DoneWitnessed` arm.
    #[test]
    fn born_then_seal_at_step_round_trips() {
        let storage = run_root_storage();
        let step_carried: StepCarried =
            storage.brand().alloc_object_witnessed(KObject::Number(7.0));
        let envelope = step_carried.seal_at_step(Rc::clone(&storage));
        let value = envelope.open(|c| match c {
            crate::machine::model::Carried::Object(KObject::Number(n)) => *n,
            _ => panic!("expected a Number object"),
        });
        assert_eq!(value, 7.0);
    }
}
