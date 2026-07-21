//! The step-branded construction doors: [`StepAllocator`], the library [`StepContext`] over a
//! step's destination frame with the Koan [`RegionBrand`] / [`FoldingBrand`] allocation capability
//! handed to each door's closure. The brand/region substrate lives in the parent `arena`
//! module.

use std::marker::PhantomData;
use std::rc::Rc;

use super::{FoldingBrand, FrameStorage, KoanStorageProfile, RegionBrand};
use crate::machine::core::kfunction::action::scope_frame;
use crate::machine::core::Scope;
use crate::machine::execute::StepCarried;
use crate::machine::model::{Carried, CarriedFamily, KObject, KType};
use crate::machine::DeliveredCarried;
use crate::witnessed::{Reattachable, StepContext};

/// The step-branded construction context: the library [`StepContext`] over the step's destination
/// frame, confined to the scheduler step that minted it by the brand lifetime `'step`. The koan
/// construction doors live here — not on `StepContext` itself — because [`RegionBrand`]'s
/// constructor is private to the `arena` module (see [`FrameStorage::brand`]): each door's closure
/// receives a [`RegionBrand`] / [`FoldingBrand`] (the koan allocation capability) rather than the
/// bare `&KoanRegion` the library-level context hands out, so a step construction site allocates
/// through the one capability every other site uses. Named with full words (`alloc_carried`, not
/// `alloc`) to avoid colliding with the generic verb each wraps.
///
/// Every door returns its carrier as a [`StepCarried`] at `'step` — in production the step tail's
/// rank-2 open lifetime, so a door product cannot be stashed past its construction step (the
/// within-step transient invariant, borrow-checker-enforced) and the sole exit to node storage is
/// the seal door in `step_carried.rs`. [`Self::alloc_carried_with`] is how a finish folds a dep's
/// reach into a carrier it builds from that dep's value: the dep views only exist inside the
/// shared brand, so a caller cannot smuggle one out and seal it under a narrower reach than the
/// fold produces.
///
/// The type is `pub` and the one door an external `compile_fail` guard drives
/// ([`Self::alloc_object_scalar`]) is `pub`, so the `#[doc(hidden)]` `step_fixture` can hand a guard
/// an allocator and have it door-allocate; the remaining doors are crate-visible. The constructors
/// are crate-confined, so no external caller can mint one. The confinement rests on the brand
/// lifetime and on the constructors' visibility — builtins receive an allocator already branded at
/// their step (`BodyCtx.ctx` / `FinishCtx.ctx`) and cannot mint one at a lifetime of their choosing.
#[derive(Clone)]
pub struct StepAllocator<'step> {
    context: StepContext<FrameStorage>,
    step: PhantomData<&'step ()>,
}

impl<'step> StepAllocator<'step> {
    /// Mint over the step's destination frame at a caller-chosen brand — the harness door (the
    /// scheduler view's step door mints at the step's own `'step`). `pub(in crate::machine)` keeps
    /// the free-brand mint out of builtins' reach.
    pub(in crate::machine) fn over_frame(frame: Rc<FrameStorage>) -> Self {
        StepAllocator {
            context: StepContext::new(frame),
            step: PhantomData,
        }
    }

    /// Mint over `scope`'s own frame, branded at the scope reference's lifetime. Sound to expose
    /// in-crate: every production `&Scope` is minted at a step's rank-2 open (the step-brand
    /// design's verified premise), so the allocator inherits a genuinely step-confined brand.
    pub(crate) fn for_scope(scope: &'step Scope<'step>) -> Self {
        Self::over_frame(scope_frame(scope))
    }

    /// The held destination-frame `Rc` itself — for callers that pin or compare the frame.
    pub(crate) fn frame(&self) -> Rc<FrameStorage> {
        self.context.frame()
    }

    /// [`StepContext::alloc`] with the closure receiving a [`RegionBrand`]: reach = own region only.
    pub(crate) fn alloc_carried(
        &self,
        build: impl for<'b> FnOnce(RegionBrand<'b>) -> <CarriedFamily as Reattachable>::At<'b>,
    ) -> StepCarried<'step> {
        StepCarried::born(
            self.context
                .alloc_handle::<KoanStorageProfile, CarriedFamily>(|handle| {
                    build(RegionBrand(handle))
                }),
        )
    }

    /// [`StepContext::alloc_with`] with the closure receiving a [`FoldingBrand`] and the deps'
    /// views: the built carrier names every listed dep's reach **and residence host** (each dep
    /// arrives as its delivery envelope and folds at `Residence::Kept`), by construction — so a
    /// value the closure builds from those deps' operands is covered by the fold, and
    /// [`FoldingBrand`]'s folded-placement methods store it without a per-value audit. Plain
    /// [`RegionBrand`] methods stay reachable through `Deref`, so a closure building an unrelated
    /// `'static` value is unaffected.
    pub(crate) fn alloc_carried_with(
        &self,
        deps: &[&DeliveredCarried],
        build: impl for<'b> FnOnce(FoldingBrand<'b>, Vec<Carried<'b>>) -> Carried<'b>,
    ) -> StepCarried<'step> {
        StepCarried::born(
            self.context
                .alloc_with_handle::<KoanStorageProfile, CarriedFamily, CarriedFamily>(
                    deps,
                    |placement, views| build(FoldingBrand::in_fold_closure(placement), views),
                ),
        )
    }

    /// Wrap a `Copy` [`KType`] handle in a step terminal: reach = own region only. A handle carries
    /// no region content, so it rides `Carried::Type` by value with no storage door.
    pub(crate) fn type_carried(&self, kt: KType) -> StepCarried<'step> {
        self.alloc_carried(|_| Carried::Type(kt))
    }

    /// The no-fold arm for a shallow scalar (Number / KString / Bool / Null): such a value embeds
    /// no borrow, so it rebuilds owned and seals with an empty reach instead of over-retaining a
    /// producer arena. `None` when the value is not a shallow scalar (the caller takes a fold door
    /// instead).
    pub fn alloc_object_scalar(&self, value: &KObject<'_>) -> Option<StepCarried<'step>> {
        // A shallow scalar embeds no borrow, so the dep-witness union would be pure over-retention:
        // route it to the no-fold path so an escaped scalar seals with an empty reach and stops
        // pinning its producer arena. `is_shallow_scalar`'s four variants hold only owned payloads,
        // so rebuilding fresh (rather than coercing the `'_`-tagged `value`) is always valid at
        // `'static` — `KObject` has no general owned rebuild, so this is a by-hand rebuild scoped to
        // exactly the owned variants `is_shallow_scalar` names. `None` when the value is not a
        // shallow scalar, so the caller takes a fold door.
        if !value.is_shallow_scalar() {
            return None;
        }
        let owned = match value {
            KObject::Number(n) => KObject::Number(*n),
            KObject::KString(s) => KObject::KString(s.clone()),
            KObject::Bool(b) => KObject::Bool(*b),
            KObject::Null => KObject::Null,
            _ => unreachable!("is_shallow_scalar guarantees one of the four owned variants"),
        };
        Some(self.alloc_carried(|b| Carried::Object(b.alloc_object(owned))))
    }
}
