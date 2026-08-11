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

use crate::machine::CarrierWitness;
use crate::machine::core::{FrameStorage, StepAllocator, run_root_storage};
use crate::machine::model::CarriedFamily;
use crate::witnessed::{Delivered, DropFree, Reattachable, Unhosted, Witnessed};

/// A value carrier confined to the scheduler step that built it. The brand lifetime `'step` is the
/// step tail's rank-2 open lifetime (`run_loop.rs`), unnameable outside that closure, so a
/// `StepCarried` cannot be stored past its construction step: the within-step transient invariant,
/// enforced by the borrow checker.
///
/// The `inner` pair is **private to this module** — that privacy is the mechanism. The sole exit
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
pub struct StepCarried<'step, T: Reattachable + DropFree = CarriedFamily> {
    /// The carrier fused with its owned coverage, minus the home pin — the library's
    /// [`Unhosted`] state, which is exactly this wrapper's shape: the host is the *finalizing*
    /// node's anchor owner, not known at any door that builds a Done-arm value. The coverage pins
    /// every region the value reaches beyond the producer's own — empty for a region-pure producer
    /// (the majority of Done sites), carried in hand for a reach-carrying one (a resident binding
    /// read, a merge product, a splice). [`Self::seal_at_step`] supplies the host and the pair
    /// becomes the delivery envelope, so the terminal's reach is threaded from here — never
    /// re-derived from the carrier's description.
    inner: Unhosted<T, CarrierWitness, FrameStorage>,
    step: PhantomData<&'step ()>,
}

impl<'step, T: Reattachable + DropFree> StepCarried<'step, T> {
    /// Wrap a **no-foreign-reach** carrier into the step brand — the majority of Done sites (literals,
    /// type carriers, region-pure `alloc_scalar_witnessed` products, and a value that borrows only its
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
    /// elsewhere takes [`Self::born_delivered`], which carries the bundle.
    /// (Re-checking it here is not available anyway: the membership queries live on the opened
    /// carrier, and this door holds no pin to open one under.)
    pub(crate) fn born(inner: Witnessed<T, CarrierWitness>) -> Self {
        StepCarried {
            inner: Unhosted::born(inner),
            step: PhantomData,
        }
    }

    /// Wrap a **reach-carrying** carrier into the step brand by consuming its whole delivery
    /// envelope — a lifted binding (the value's home region among the members), a splice recovered
    /// from its producer's envelope, or the product of a composition into the step's own destination
    /// region (a fold, a relocation, a merge). The envelope is the only source, and it is never
    /// taken apart: [`Delivered::unhost`] drops the home pin and keeps the carrier fused to its
    /// coverage, so the two cannot arrive from different values and a caller cannot drop the pins
    /// and brand the carrier alone. [`Self::seal_at_step`] pins a host back on, so the terminal's
    /// reach is owned end-to-end rather than re-derived.
    ///
    /// The coverage travels whole, residence included. Where residence *is* the step's own
    /// destination region, [`Self::seal_at_step`] re-pins it as the terminal's host and the union
    /// collapses the duplicate; where it is some outer region the value was lifted out of (a binding
    /// scope), it is a genuine member and stripping it would drop the only pin naming it.
    pub(crate) fn born_delivered(envelope: Delivered<T, CarrierWitness, FrameStorage>) -> Self {
        StepCarried {
            inner: envelope.unhost(),
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
        self.inner.host(host)
    }

    /// Seal the carrier the door built over its anchor's owner and read the delivered value by
    /// reference — the inspection a door test uses to assert a product's contents without ever
    /// extracting the carrier. `host` is the anchor's owner, the same trust [`Self::seal_at_step`]
    /// places; the read runs under the envelope's own pins, so there is no free pin parameter to
    /// mis-supply. `read` sees a `for<'b>` re-anchored view and returns owned data; the
    /// lifetime-free [`Witnessed`] never leaves the wrapper. Consuming, since every caller reads
    /// once and drops the carrier. `#[cfg(test)]`-gated, so it is absent from every production
    /// build: the no-stash compile guarantee AC 1 names holds for all non-test code, pinned by the
    /// `compile_fail` fixtures. A `machine::core` door test cannot reach the `pub(super)`
    /// [`Self::seal_at_step`] exit, so this read is how it inspects the carrier the door built.
    #[cfg(test)]
    pub(crate) fn inspect_at<R>(
        self,
        host: Rc<FrameStorage>,
        read: impl for<'b> FnOnce(&'b <T as Reattachable>::At<'b>) -> R,
    ) -> R {
        self.inner.host(host).open_ref(read)
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
    use crate::machine::core::{FrameStorageExt, run_root_storage};
    use crate::machine::model::KObject;
    use crate::machine::model::Scalar;

    /// The legal shape: born a region-pure carrier, then exit through the sole seal door into a
    /// delivery envelope pinned by its own storage. Mirrors the run loop's `DoneWitnessed` arm.
    #[test]
    fn born_then_seal_at_step_round_trips() {
        let storage = run_root_storage();
        let step_carried: StepCarried = storage.brand().alloc_scalar_witnessed(Scalar::Number(7.0));
        let envelope = step_carried.seal_at_step(Rc::clone(&storage));
        let value = envelope.open(|c| match c {
            crate::machine::model::Carried::Object(KObject::Number(n)) => *n,
            _ => panic!("expected a Number object"),
        });
        assert_eq!(value, 7.0);
    }
}
