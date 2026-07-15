//! The step-branded construction doors: [`StepAllocator`], the library [`StepContext`] over a
//! step's destination frame with the Koan [`RegionBrand`] / [`FoldingBrand`] allocation capability
//! handed to each door's closure. The brand/region substrate lives in the parent `arena`
//! module.

use std::marker::PhantomData;
use std::rc::Rc;

use super::{
    FoldingBrand, FrameStorage, FrameStorageExt, KoanRegionExt, KoanStorageProfile, RegionBrand,
    ScopeFoldOperands, ScopeFoldViews, TypeOperand,
};
use crate::machine::core::kfunction::action::scope_frame;
use crate::machine::core::{KoanRegion, Scope};
use crate::machine::execute::StepCarried;
use crate::machine::model::{Carried, CarriedFamily, KObject, KType};
use crate::machine::{DeliveredCarried, KError};
use crate::witnessed::{Reattachable, StepContext, Witnessed};

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

    /// [`Self::alloc_carried_with`] plus the consumer's scope crossed as its own delivered envelope:
    /// the build closure receives the scope re-anchored at the fold brand, so scope reads are
    /// declared operands rather than ambient captures. Reach = own region ∪ the scope's host ∪ every
    /// dep's reach and host. Scope reads resolving to *ancestor* bindings stay covered by the
    /// destination's ambient coverage, exactly as everywhere else — folding the immediate scope host
    /// is strictly more coverage than folding the deps alone, never less.
    pub(crate) fn alloc_carried_with_scope<'sc>(
        &self,
        deps: &[&DeliveredCarried],
        scope: &'sc Scope<'sc>,
        build: impl for<'b> FnOnce(FoldingBrand<'b>, Vec<Carried<'b>>, &'b Scope<'b>) -> Carried<'b>,
    ) -> StepCarried<'step> {
        // Hand-rolls `StepContext::alloc_with`'s fold shape, extended with the scope operand: yoke the
        // dest-region handle up front, fold each dep view onto it at `Residence::Kept` (the dep keeps
        // living in its producer region, so that host materializes as a member), then fold the scope's
        // own delivered envelope so the build brand receives it re-anchored at `'b`.
        let acc0 = KoanRegion::yoke_branded::<ScopeFoldViews, _>(self.frame(), |brand| {
            (brand.handle(), Vec::with_capacity(deps.len()))
        });
        let views = deps.iter().fold(acc0, |acc, dep| {
            dep.transfer_into::<ScopeFoldViews, ScopeFoldViews, KoanStorageProfile>(
                acc,
                crate::witnessed::Residence::Kept,
                |view, (handle, mut views), _brand| {
                    views.push(view);
                    (handle, views)
                },
            )
        });
        let operands = scope
            .seal_scope_ref_delivered()
            .transfer_into::<ScopeFoldViews, ScopeFoldOperands, KoanStorageProfile>(
                views,
                crate::witnessed::Residence::Kept,
                |scope_view, (handle, views), _brand| (handle, (views, scope_view)),
            );
        // The engine mints the placement from the operand's own head handle — the handle yoked over
        // the frame's region that heads `ScopeFoldOperands` — so the destination is the region the
        // accumulated witness covers, by construction. The build value comes only from this fold's
        // declared operands — the dep views and the crossed scope envelope, both re-anchored at this
        // brand — whose reach the enclosing fold already composes into the result's witness. No
        // ambient-lifetime borrow reaches this closure.
        StepCarried::born(
            self.context
                .map_pinned_placing::<ScopeFoldOperands, CarriedFamily, KoanStorageProfile>(
                    operands,
                    |(_handle, (views, scope)), placement| {
                        build(FoldingBrand::in_fold_closure(placement), views, scope)
                    },
                ),
        )
    }

    /// [`Self::alloc_carried`] specialized to the one-`KType`-carrier shape: reach = own region
    /// only. `kt`'s `'static` bound is region-purity, compile-enforced — the common case for a
    /// bind-time or synchronously-resolved type. A `kt` that borrows another region takes
    /// [`Self::alloc_type_checked`] instead.
    pub(crate) fn alloc_type(&self, kt: KType<'static>) -> StepCarried<'step> {
        self.alloc_carried(|b| Carried::Type(b.alloc_ktype(kt)))
    }

    /// The step twin of [`RegionBrand::alloc_ktype_checked`]: runtime-audits `kt`'s region
    /// borrows against this frame's own region and seals the result under the empty (own-region
    /// only) reach — the same [`Carried::Type`] wrap [`Self::alloc_type`] uses — erroring instead
    /// of storing an unvetted foreign-region dangle. For a `kt` [`KType::to_static`] declines (a
    /// module-family pointer, a signature pointer, or an `Rc`-shared set).
    ///
    /// Confined to identity-preserving stores: a caller reaches here to store a value that cannot
    /// rebuild at `'static`. A site assembling a new composite [`KType`] from ambiently-read parts
    /// takes a brand door instead ([`Self::alloc_type_composed`], [`Self::alloc_carried_with`], or
    /// the field-list fold), so no from-scratch composite rides the runtime audit.
    pub(crate) fn alloc_type_checked(&self, kt: KType<'_>) -> Result<StepCarried<'step>, KError> {
        // Unlike `alloc_carried`'s `for<'b>` brand construction, the checked veneer doesn't need
        // to build `kt` from brand-derived references — `kt` already exists and is audited by
        // address, so the resident reference it hands back is erased straight into the empty
        // (own-region-only) witness via `Witnessed::resident`, mirroring
        // `RegionBrand::alloc_object_witnessed`'s erase-on-store without the brand-closure
        // indirection `alloc_carried` needs for a from-scratch construction.
        let frame = self.frame();
        let stored = frame.brand().alloc_ktype_checked(kt)?;
        Ok(StepCarried::born(Witnessed::resident(Carried::Type(
            stored,
        ))))
    }

    /// Seal a delivered *type* terminal's value as this step's own carrier. The type is rebuilt at
    /// the fold brand from the dep's view — never captured at an ambient lifetime — so reach = own
    /// region ∪ the dep's reach and host. Scalar gate: a region-free scalar type references no
    /// region, so it routes to the no-fold path and seals with an empty reach.
    pub(crate) fn alloc_type_of(&self, dep: &DeliveredCarried) -> StepCarried<'step> {
        // Scalar gate: a region-free scalar type references no region, so folding the dep's reach in
        // would only over-retain. Route it to the no-fold path so it seals with an empty reach.
        // `is_region_free_scalar` is exactly `to_static`'s owned-leaf class, so the rebuild always
        // succeeds.
        if let Some(owned) = dep.open(|c| match c {
            Carried::Type(kt) if kt.is_region_free_scalar() => kt.to_static(),
            _ => None,
        }) {
            return self.alloc_type(owned);
        }
        self.alloc_carried_with(&[dep], |b, views| match views[0] {
            Carried::Type(kt) => Carried::Type(b.alloc_ktype_folded(kt.clone())),
            Carried::Object(_) => {
                unreachable!("alloc_type_of precondition: the dep terminal is a type")
            }
        })
    }

    /// The correlated multi-operand type build: `operands` lists **every** type the composite
    /// embeds, in embedding order, and `compose` receives exactly one `&'b KType<'b>` per operand
    /// at the same position — so the composite is built at the brand from declared operands only.
    /// `compose` is a plain `fn` so it cannot capture: an ambient-lifetime `KType` smuggled past
    /// the operand list is a compile error, not an audit obligation. Reach = own region ∪ every
    /// `Reaching` operand's reach and host; `Pure` operands add none (the scalar gate's
    /// exact-reach behavior, by construction).
    pub(crate) fn alloc_type_composed(
        &self,
        operands: Vec<TypeOperand<'_>>,
        compose: for<'b> fn(FoldingBrand<'b>, &[&'b KType<'b>]) -> KType<'b>,
    ) -> StepCarried<'step> {
        // One pass keeps the invariant the compose relies on: the deps list is exactly the
        // `Reaching` subsequence of `operands`, in order, and `plan` holds each `Pure` value at
        // its operand position (`None` = "take the next view").
        let mut deps: Vec<&DeliveredCarried> = Vec::new();
        let mut plan: Vec<Option<KType<'static>>> = Vec::with_capacity(operands.len());
        for operand in operands {
            match operand {
                TypeOperand::Reaching(dep) => {
                    deps.push(dep);
                    plan.push(None);
                }
                TypeOperand::Pure(kt) => plan.push(Some(kt)),
            }
        }
        self.alloc_carried_with(&deps, move |brand, views| {
            // Captures: `plan` (owned 'static data) and `compose` (fn pointer). Neither can carry
            // an ambient-lifetime borrow; the composed value comes only from the views and the
            // brand's own 'static allocs.
            let mut view_iter = views.into_iter();
            let parts: Vec<&KType> = plan
                .into_iter()
                .map(|slot| match slot {
                    Some(kt) => brand.alloc_ktype(kt),
                    None => match view_iter.next().expect(
                        "alloc_type_composed: one view per Reaching operand, by the partition above",
                    ) {
                        Carried::Type(kt) => kt,
                        Carried::Object(_) => unreachable!(
                            "alloc_type_composed precondition: every Reaching operand \
                             is the carrier of a value the call site proved to be a type",
                        ),
                    },
                })
                .collect();
            Carried::Type(brand.alloc_ktype_folded(compose(brand, &parts)))
        })
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
        // `'static` — `KObject` has no general `to_static` (unlike `KType`; see `KType::to_static`'s
        // doc), so this is a by-hand rebuild scoped to exactly the owned variants `is_shallow_scalar`
        // names. `None` when the value is not a shallow scalar, so the caller takes a fold door.
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
