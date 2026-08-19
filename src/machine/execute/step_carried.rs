//! The step-scoped brand for the Done-arm value carrier.
//!
//! A member-less resident carrier (a region-pure value under a description hosted in its own
//! region) pins nothing: it is sound only as a within-step transient — the step's held frame set
//! pins the producing region across the step, and [`StepCarried::seal_at_step`] folds that frame
//! into the carrier's reach before it leaves the step. [`StepCarried`] makes that transient a type:
//! the carrier crossing the Done arm rides a brand lifetime `'step` that is the step's rank-2 open
//! lifetime ([`Host::step`](super::harness::Host)), unnameable outside that closure, so the borrow
//! checker rejects any attempt to stash it past its construction step.

use std::marker::PhantomData;
use std::rc::Rc;

use crate::machine::CarrierWitness;
use crate::machine::core::{FrameStorage, StepAllocator, run_root_storage};
use crate::machine::model::CarriedFamily;
use crate::witnessed::{Delivered, DropFree, Reattachable, Unhosted, Witnessed};

/// A value carrier confined to the scheduler step that built it, so it cannot be stored past that
/// step.
///
/// The `inner` pair is **private to this module** — that privacy is the mechanism. The sole exit
/// is [`Self::seal_at_step`], which consumes the wrapper into a delivery envelope; no accessor hands
/// back the lifetime-free [`Witnessed`] for a holder to stash. `PhantomData<&'step ()>` is
/// covariant, matching [`FoldToken`](crate::witnessed::FoldToken): escaping the brand would require
/// *lengthening* `'step`, which covariance forbids and unnameability prevents.
///
/// The type is `pub` only so the `#[doc(hidden)]` `step_fixture` can drive it from a `compile_fail`
/// external crate (the `machine::execute` module is `pub(crate)`, so it is not part of koan's real
/// API). The confinement rests on [`born`](Self::born) (`pub(crate)`) and
/// [`seal_at_step`](Self::seal_at_step) (`pub(super)`) being unreachable outside the crate, plus the
/// brand lifetime — never on the type being unnameable.
pub struct StepCarried<'step, T: Reattachable + DropFree = CarriedFamily> {
    /// The host is the *finalizing* node's anchor owner, not known at any door that builds a
    /// Done-arm value, so the carrier rides fused to its owned coverage minus that pin.
    /// [`Self::seal_at_step`] supplies the host and the pair becomes the delivery envelope, so the
    /// terminal's reach is threaded from here — never re-derived from the carrier's description.
    inner: Unhosted<T, CarrierWitness, FrameStorage>,
    step: PhantomData<&'step ()>,
}

impl<'step, T: Reattachable + DropFree> StepCarried<'step, T> {
    /// Wrap a **no-foreign-reach** carrier, whose owned pin bundle is therefore empty. Unrestricted
    /// in-crate: wrapping only ever *adds* confinement.
    ///
    /// The premise is established at the doors that build such a carrier, not checked here: a
    /// carrier eligible for this wrapper comes from
    /// [`Scope::resident`](crate::machine::core::Scope) / [`RegionBrand::seal_resident`](crate::machine::core::RegionBrand),
    /// whose mint composes no source and so yields a description with no members at all, or from the
    /// checked-alloc door, whose only possible member is the birth region itself. Neither can name a
    /// foreign region, so neither has owned pins to thread. A producer whose value *does* reach
    /// elsewhere takes [`Self::born_delivered`], which carries the bundle. (Re-checking it here is
    /// not available anyway: the membership queries live on the opened carrier, and this door holds
    /// no pin to open one under.)
    pub(crate) fn born(inner: Witnessed<T, CarrierWitness>) -> Self {
        StepCarried {
            inner: Unhosted::born(inner),
            step: PhantomData,
        }
    }

    /// Wrap a **reach-carrying** carrier by consuming its whole delivery envelope. The envelope is
    /// the only source, and it is never taken apart: [`Delivered::unhost`] drops the home pin and
    /// keeps the carrier fused to its coverage, so the two cannot arrive from different values and a
    /// caller cannot drop the pins and brand the carrier alone. The coverage travels whole,
    /// residence included — [`Self::seal_at_step`] pins a host back on, so the terminal's reach is
    /// owned end-to-end rather than re-derived.
    pub(crate) fn born_delivered(envelope: Delivered<T, CarrierWitness, FrameStorage>) -> Self {
        StepCarried {
            inner: envelope.unhost(),
            step: PhantomData,
        }
    }

    /// The only exit from the step brand. `pub(super)` so the seal/finalize sites in [`super`] can
    /// call it while `crate::builtins` cannot — a builtin holding a `StepCarried` cannot strip the
    /// brand.
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

    /// Seal over the anchor's owner and read the delivered value by reference, without ever
    /// extracting the carrier: `read` sees a `for<'b>` re-anchored view and returns owned data, so
    /// the lifetime-free [`Witnessed`] never leaves the wrapper. `host` is the anchor's owner, the
    /// same trust [`Self::seal_at_step`] places. `#[cfg(test)]`-gated, so the no-stash compile
    /// guarantee holds for every production build; it exists because a `machine::core` door test
    /// cannot reach the `pub(super)` [`Self::seal_at_step`] exit.
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
/// `'b` is universally quantified over `guard`, so a guard body can allocate through the allocator's
/// doors but cannot store a door product past the closure (doing so makes `'b` escape — the
/// `compile_fail` pin for the door half of the brand). Lives here rather than in the crate-root
/// `step_fixture` because [`StepAllocator::over_frame`] is `pub(in crate::machine)`.
/// `#[doc(hidden)]` fixture surface, not real API.
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
    /// delivery envelope pinned by its own storage.
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
