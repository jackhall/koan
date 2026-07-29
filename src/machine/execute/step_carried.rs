//! The step-scoped brand for the Done-arm value carrier.
//!
//! A member-less resident carrier (a region-pure value under a description hosted in its own
//! region) pins nothing: it is sound only as a within-step transient — the run loop's
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

use crate::machine::core::{run_root_storage, FrameCoverage, FrameStorage, StepAllocator};
use crate::machine::model::CarriedFamily;
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
    /// The carrier's owned foreign coverage — pinning every region the value reaches beyond the
    /// producer's own. Empty for a region-pure / empty-reach producer (the majority of Done sites);
    /// carried in hand for a reach-carrying producer (a resident binding read, a merge product, a
    /// splice). [`Self::seal_at_step`] consumes it into the delivery envelope, so the terminal's
    /// reach is threaded from here — never re-derived from the carrier's description.
    pins: FrameCoverage,
    step: PhantomData<&'step ()>,
}

impl<'step, T: Reattachable> StepCarried<'step, T> {
    /// Wrap a **no-foreign-reach** carrier into the step brand — the majority of Done sites (literals,
    /// type carriers, region-pure `alloc_object_witnessed` products, and a value that borrows only its
    /// own home, e.g. a fresh closure capturing its defining scope), whose owned pin bundle is
    /// therefore empty. Unrestricted in-crate: wrapping only
    /// ever *adds* confinement, so any construction site may brand a carrier it holds. `'step` is
    /// inferred from the context the wrapper flows into — the Done-arm enums
    /// ([`Outcome`](super::outcome::Outcome), [`NodeStep`](super::nodes::NodeStep)) carry it at the
    /// step open's rank-2 brand.
    ///
    /// The premise is established at the doors that build such a carrier, not checked here: a
    /// carrier eligible for this wrapper comes from
    /// [`Scope::resident`](crate::machine::core::Scope) / [`RegionBrand::seal_resident`](crate::machine::core::RegionBrand),
    /// whose mint composes no source and so yields a description with no members at all, or from the
    /// checked-alloc door, whose only possible member is the birth region itself. Neither can name a
    /// foreign region, so neither has owned pins to thread. A producer whose value *does* reach
    /// elsewhere takes [`Self::born_pinned`] or [`Self::born_delivered`], which carry the bundle.
    /// (Re-checking it here is not available anyway: the membership queries live on the opened
    /// carrier, and this door holds no pin to open one under.)
    pub(crate) fn born(inner: Witnessed<T, CarrierWitness>) -> Self {
        StepCarried {
            inner,
            pins: FrameCoverage::empty(),
            step: PhantomData,
        }
    }

    /// Wrap a **reach-carrying** carrier into the step brand, threading its owned
    /// [`FrameCoverage`] — a resident binding read (the entry's coverage) or a splice (the source
    /// envelope's whole coverage, its producer's own region among them), where the carrier and its
    /// pins arrive separately rather than inside an envelope [`Self::born_delivered`] could consume
    /// whole. The coverage rides the step and [`Self::seal_at_step`] consumes it into the delivery
    /// envelope, so the terminal's reach is owned end-to-end rather than re-derived.
    pub(crate) fn born_pinned(inner: Witnessed<T, CarrierWitness>, pins: FrameCoverage) -> Self {
        StepCarried {
            inner,
            pins,
            step: PhantomData,
        }
    }

    /// Wrap the **product of a composition into the step's own destination region**: a fold, a
    /// relocation, a merge. The product envelope's residence *is* that destination, which
    /// [`Self::seal_at_step`] pins again as the terminal's host, so it is released here
    /// ([`Delivered::coverage_releasing_home`]) and what rides the step is the product's foreign
    /// coverage alone — the same set the composition's self rule already stripped from the bundle it
    /// composed.
    pub(crate) fn born_delivered(envelope: Delivered<T, CarrierWitness, FrameStorage>) -> Self {
        let pins = envelope.coverage_releasing_home();
        StepCarried {
            inner: envelope.into_cell().unseal(),
            pins,
            step: PhantomData,
        }
    }

    /// The only exit from the step brand: pair the carrier with the anchor's storage pin and its
    /// owned foreign pins, and hand it to finalize. `pub(super)` so the seal/finalize sites in
    /// [`super`] can call it while `crate::builtins` cannot — a builtin holding a `StepCarried`
    /// cannot strip the brand.
    ///
    /// This door trusts its caller to pass the *right* host (the anchor's owner); binding that free
    /// parameter is a separate concern. The door's contract here is only that it is the unique way a
    /// `StepCarried` reaches node storage.
    pub(super) fn seal_at_step(
        self,
        host: Rc<FrameStorage>,
    ) -> Delivered<T, CarrierWitness, FrameStorage> {
        Delivered::seal(self.inner, host, self.pins)
    }

    /// Borrow the carrier the door built and read its pointee under an externally supplied `pin`,
    /// exactly as [`Witnessed::with_pinned`] does — the borrowed inspection a door test uses to
    /// assert a product's contents without ever extracting the carrier. `read` sees a `for<'b>`
    /// re-anchored view and returns owned data; the lifetime-free [`Witnessed`] never leaves the
    /// wrapper. `#[cfg(test)]`-gated, so it is absent from every production build: the no-stash
    /// compile guarantee AC 1 names holds for all non-test code, pinned by the `compile_fail`
    /// fixtures. A `machine::core` door test cannot reach the `pub(super)` [`Self::seal_at_step`]
    /// exit, so this borrowed read is how it inspects the carrier the door built.
    #[cfg(test)]
    pub(crate) fn inspect_pinned<Pin, R>(
        &self,
        pin: &Pin,
        read: impl for<'b> FnOnce(&'b <T as Reattachable>::At<'b>) -> R,
    ) -> R
    where
        Pin: crate::witnessed::Witness,
    {
        self.inner.with_pinned(pin, read)
    }

    /// Consume the wrapper through the [`Self::seal_at_step`] exit under a `#[cfg(test)]` gate, so a
    /// `machine::core` door test (outside `super`, where `seal_at_step` is reachable) can drive the
    /// finalize shape it exercises. Returns the [`Delivered`] envelope, never the lifetime-free
    /// [`Witnessed`]: sealing only ever *adds* the storage pin, so it cannot leak a reattachable
    /// carrier.
    #[cfg(test)]
    pub(crate) fn seal_for_test(
        self,
        host: Rc<FrameStorage>,
    ) -> Delivered<T, CarrierWitness, FrameStorage> {
        self.seal_at_step(host)
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
