//! The Koan instantiation of the generic [`Region`](crate::witnessed::Region)
//! storage substrate: `KoanRegion = Region<KoanStorageProfile>`, the per-family
//! [`Stored`](crate::witnessed::Stored) impls (which library-owned cell a family lands in), and
//! the Koan-typed `alloc_*` wrappers. `CallFrame`
//! — the per-call frame shell over a refcounted `FrameStorage` (the `KoanRegion` plus the ancestor
//! chain), holding the child `Scope` — also lives here.
//!
//! The generic erase-store engine lives in [`crate::witnessed::region`]; this file supplies the
//! Koan policy it runs.
//!
//! See [per-call-region/README.md](../../../design/per-call-region/README.md) for the carrier
//! set, escaping-value retention, ancestor chain, and TCO frame reuse;
//! [memory-model.md § Region lifetime erasure](../../../design/memory-model.md#region-lifetime-erasure)
//! for the heap-pinning / drop-order invariants.

use crate::machine::{CarrierWitness, DeliveredCarried};
use std::cell::Cell;
use std::rc::Rc;

use super::scope::Scope;
use super::scope_id::ScopeId;
use super::scope_ptr::ScopeRefFamily;
use crate::machine::core::kfunction::{KFunction, NodeId};
use crate::machine::model::operators::OperatorGroup;
use crate::machine::model::types::KType;
use crate::machine::model::values::{Carried, CarriedFamily, KObject, Module, ModuleSignature};
use crate::witnessed::reattachable;
use crate::witnessed::SealedExtern;
use crate::witnessed::{
    Delivered, Erased, FamilyArena, HasRegionHandle, Reattachable, Region, RegionHandle,
    RegionHandleFamily, RegionHost, RegionSet, Sealed, StepContext, StorageOf, StorageProfile,
    Stored, WitnessRegion, Witnessed,
};

/// The Koan workload: the family set whose library-derived bundle a [`Region`] owns — one library
/// [`FamilyArena`] cell per family. The `KType` cell backs per-type identity binding storage
/// (`Bindings::types`); the `OperatorGroup` cell backs the per-scope operator registry
/// (`Bindings::operators`).
pub struct KoanStorageProfile;

impl StorageProfile for KoanStorageProfile {
    type Families = (
        KObject<'static>,
        (
            KFunction<'static>,
            (
                Scope<'static>,
                (
                    Module<'static>,
                    (
                        ModuleSignature<'static>,
                        (KType<'static>, (OperatorGroup, (FrameSet, ()))),
                    ),
                ),
            ),
        ),
    );
}

/// Run-lifetime allocator. A [`Region`] carrying the Koan family set; lives for one program
/// run. The `KoanRegion` references across the tree and the `Rc<CallFrame>` back-edge ride this
/// alias unchanged.
pub type KoanRegion = Region<KoanStorageProfile>;

/// Koan's typed veneer over the library [`RegionHandle`] allocation capability for a [`KoanRegion`] —
/// a `Copy` newtype adding only the Koan-family-typed `alloc_*` methods. The capability rules
/// themselves — owner-only minting, "a bare region cannot allocate" — are `workgraph`'s, enforced on
/// [`RegionHandle`] and compile-guarded there; this type carries no capability rule of its own.
///
/// **Frame-lifetime, not a per-alloc `for<'b>` brand.** A structural resident (a binding entry, a
/// `Module`'s child `&Scope`) must outlive any one brand window, so it needs a real `&'a` — which only
/// a frame-lifetime handle hands back. The per-alloc `for<'b>` brand is the right tool for *terminals*
/// (the witnessed surface, where [`Region::alloc`] hands a `for<'b>` brand and returns a `Witnessed`
/// carrier); this handle is for the co-located plumbing.
///
/// A bare `&KoanRegion` exposes **no** `alloc_*` — allocation is reachable only through this veneer.
/// Minting a `KoanRegion` at all is unreachable from Koan too: the library's bare-region constructor
/// is sealed to `workgraph`, so the only route to a region is a library-provisioned [`FrameStorage`],
/// never an ambient region reference Koan mints itself.
#[derive(Clone, Copy)]
pub struct RegionBrand<'a>(RegionHandle<'a, KoanStorageProfile>);

impl<'a> RegionBrand<'a> {
    /// The bare region this brand authorizes — for identity compares (`ptr::eq`, `pins_region`). A
    /// bare `&KoanRegion` cannot be turned *back* into a brand — the library's [`RegionHandle`] enforces
    /// that — so handing out the identity reference opens no hole.
    pub fn region(self) -> &'a KoanRegion {
        self.0.region()
    }

    /// The bare library allocation capability this brand wraps — the [`HasRegionHandle`] accessor
    /// a `Carrier` composition mints through, and the sole route to it outside this module (the
    /// field itself stays private, so a brand's only public surfaces are the Koan-typed `alloc_*`
    /// veneer and this raw handle for the compose seam).
    pub(crate) fn handle(self) -> RegionHandle<'a, KoanStorageProfile> {
        self.0
    }

    /// Store a [`KObject`] into the region; the value lands here (no value holds an owning `Rc` back
    /// to a region, so the store forms no back-edge). Yields a co-located `&'a` resident.
    pub fn alloc_object(self, o: KObject<'_>) -> &'a KObject<'a> {
        self.0.alloc_resident::<KObject<'static>>(o)
    }

    /// Store a [`KType`] into the region (a `Module` rides a bare borrow into its child scope's
    /// region — held alive by the carrier's witness set, not an embedded anchor).
    pub fn alloc_ktype(self, t: KType<'_>) -> &'a KType<'a> {
        self.0.alloc_resident::<KType<'static>>(t)
    }

    /// INVARIANT: a `KFunction` must be allocated into the same `KoanRegion` that owns its
    /// captured scope — otherwise a `KFunction` could reference a region other than the one
    /// that allocated it, undermining region-based reasoning about `&KFunction` liveness. The
    /// `debug_assert!` catches violations at the allocation site rather than later as
    /// use-after-free.
    pub fn alloc_function(self, f: KFunction<'_>) -> &'a KFunction<'a> {
        debug_assert!(
            std::ptr::eq(
                self.0.region() as *const KoanRegion,
                f.captured_scope().region() as *const KoanRegion
            ),
            "alloc_function invariant :KFunction must be allocated into the same KoanRegion \
             that owns its captured scope"
        );
        self.0.alloc_resident::<KFunction<'static>>(f)
    }

    pub fn alloc_scope(self, s: Scope<'_>) -> &'a Scope<'a> {
        self.0.alloc_resident::<Scope<'static>>(s)
    }

    pub fn alloc_module(self, m: Module<'_>) -> &'a Module<'a> {
        self.0.alloc_resident::<Module<'static>>(m)
    }

    pub fn alloc_signature(self, s: ModuleSignature<'_>) -> &'a ModuleSignature<'a> {
        self.0.alloc_resident::<ModuleSignature<'static>>(s)
    }

    /// Allocate an [`OperatorGroup`]. Lifetime-free and anchor-free, so the gate is a no-op, but it
    /// routes the same engine for a single uniform allocation path.
    pub fn alloc_operator_group(self, g: OperatorGroup) -> &'a OperatorGroup {
        self.0.alloc_resident::<OperatorGroup>(g)
    }

    /// Mint a frozen witness set into this brand's region arena — the Koan veneer over
    /// [`RegionSet::mint`]. `omit` is the scope's home/lexical-ancestor policy predicate;
    /// home-omission (self-cycle) is handled by the library. `None` when the composed reach is
    /// empty (a region-pure value pins nothing).
    pub(crate) fn mint(
        self,
        sources: &[&FrameSet],
        materialize_hosts: &[Rc<FrameStorage>],
        omit: impl Fn(&KoanRegion) -> bool,
    ) -> Option<&'a FrameSet> {
        RegionSet::mint(self.0, sources, materialize_hosts, omit)
    }

    /// The witnessed-allocation surface for an **owned, region-pure** object — a value referencing no
    /// other region: born witnessed by the **empty** (foreign-reach-only) set. The brand-confined
    /// [`alloc`](Region::alloc) stores `value` and hands the freshly-stored `&'b KObject<'b>` to the
    /// closure at the brand, which bundles it through [`Witnessed::resident`] — the empty-witness
    /// constructor that names the region-pure obligation, so the active frame is deliberately excluded.
    /// The producing frame is folded in only at finalize/close (the scope-reach seal), so a
    /// region-resident value never strong-owns its own frame (the `region → object → frame` cycle that
    /// would keep the frame's `Rc` alive forever and defeat the refcount-driven region free).
    ///
    /// Region-pure is this surface's precondition: the object — built fresh inside the brand —
    /// references nothing foreign, so the empty set is its exact reach. Soundness is the within-step
    /// transient invariant: the empty-witness carrier pins nothing, sound only because the active frame
    /// pins the region externally for the construction step and `finalize` folds the producer
    /// **before** the carrier is stored on a node. A value that *references* another region is the
    /// `yoke` / `merge` path, not this one.
    pub(crate) fn alloc_object_witnessed(
        self,
        value: KObject<'_>,
    ) -> Witnessed<CarriedFamily, CarrierWitness> {
        self.0
            .alloc::<KObject<'static>, _>(value, |live| Witnessed::resident(Carried::Object(live)))
    }

    /// Bundle a value **already resident in this brand's region** under `witness` — the terminal
    /// carrier a name / ATTR read hands back and an FN-def / LET define site seals its object with.
    /// Unlike [`alloc_object_witnessed`](Self::alloc_object_witnessed) the value is not stored here;
    /// it pre-exists in the region, so it is bundled through [`Witnessed::resident`] — the reading /
    /// defining frame pins the region for the step, and past the step the scheduler's retention hold
    /// (the delivery envelope's host) carries the pin. Confines [`Witnessed::resident`] to this arena
    /// surface, so no read / define builtin reaches for it. `witness` must name the value's
    /// home-omitted foreign reach; the caller
    /// ([`Scope::resident_value_carrier`](crate::machine::core::Scope)) folds it. The brand is the
    /// capability marker: only a handle into the region the value lives in may re-seal it resident.
    pub(crate) fn seal_resident(
        self,
        carried: Carried<'_>,
        witness: CarrierWitness,
    ) -> Witnessed<CarriedFamily, CarrierWitness> {
        let _ = self.0;
        Witnessed::from_erased(Erased::erase(carried), witness)
    }
}

// The lifetime family of each stored type, keyed on its `'static` form — the GAT the
// `Region` engine erases to `'static` for storage and re-anchors to the caller's `'a` on read.
// Each family is one type generic only in a single lifetime, so its layout is identical for every
// choice of that lifetime; `OperatorGroup` is lifetime-free, trivially invariant. The shared
// `reattachable!` macro discharges the layout-invariance `unsafe` obligation once (see its docs).
reattachable! {
    KObject<'static> => KObject<'r>,
    KType<'static> => KType<'r>,
    KFunction<'static> => KFunction<'r>,
    Scope<'static> => Scope<'r>,
    Module<'static> => Module<'r>,
    ModuleSignature<'static> => ModuleSignature<'r>,
    OperatorGroup => OperatorGroup,
}

/// A witnessed-construction operand bundling a destination region's [`RegionBrand`] with a
/// type-channel identity (a `SetRef` / declared type) that must cross the build brand. A
/// value-embedding construction `transfer_into`/`merge`s its object carrier with this operand so the
/// wrapped value lands — allocated through the brand — tagged by the identity, both re-anchored to the
/// build brand under the same witness; the dest frame's `outer` chain pins the identity's (ancestor)
/// region. Used by the newtype / tagged-union constructors and the `CATCH` `Result` build.
/// Layout-invariant: two thin pointers, representation independent of `'r`.
pub struct RegionTypeFamily;
reattachable!(RegionTypeFamily => (RegionBrand<'r>, &'r KType<'r>));

// SAFETY: the handle authorizes allocation into `self.0`'s own region — exactly the region a
// `Carrier` composed against this family's live form re-homes into, so a set minted through it is
// co-located with the composed carrier's `host` (this operand's `RegionBrand` IS that host).
unsafe impl<'b> HasRegionHandle<'b, KoanStorageProfile> for (RegionBrand<'b>, &'b KType<'b>) {
    fn region_handle(&self) -> RegionHandle<'b, KoanStorageProfile> {
        self.0.handle()
    }
}

// SAFETY: the handle authorizes allocation into the brand's own region — the region a `Carrier`
// composed against a bare region-ref operand re-homes into (the brand itself becomes the composed
// carrier's `host`). The one instance where the family's live form *is* the brand, not a tuple
// wrapping it (the destination-region relocation operand, `execute::run_loop::RegionRefFamily`).
unsafe impl<'b> HasRegionHandle<'b, KoanStorageProfile> for RegionBrand<'b> {
    fn region_handle(&self) -> RegionHandle<'b, KoanStorageProfile> {
        (*self).handle()
    }
}

// SAFETY: same obligation as the tuple impl above — `self.0`'s own region is where a composed
// `Carrier` re-homes. Covers every aggregate-builder family shaped `(RegionBrand<'r>, Vec<Held<'r>>)`
// (the list/dict aggregate accumulator, in both production and test fixtures) by structural type,
// not by family marker — two distinct private `AggBuildFamily` markers share this one impl because
// their `At<'r>` GAT projects to the identical concrete tuple type.
unsafe impl<'b> HasRegionHandle<'b, KoanStorageProfile>
    for (
        RegionBrand<'b>,
        Vec<crate::machine::model::values::Held<'b>>,
    )
{
    fn region_handle(&self) -> RegionHandle<'b, KoanStorageProfile> {
        self.0.handle()
    }
}

// SAFETY: same obligation, for the named-field builder shape `(RegionBrand<'r>, Vec<(String,
// Held<'r>)>)` (the record-literal field accumulator).
unsafe impl<'b> HasRegionHandle<'b, KoanStorageProfile>
    for (
        RegionBrand<'b>,
        Vec<(String, crate::machine::model::values::Held<'b>)>,
    )
{
    fn region_handle(&self) -> RegionHandle<'b, KoanStorageProfile> {
        self.0.handle()
    }
}

// SAFETY: same obligation, for the named-field builder shape `(RegionBrand<'r>, Vec<(String,
// KObject<'r>)>)` (the record-repr newtype's field accumulator, `RecordFieldsFamily`).
unsafe impl<'b> HasRegionHandle<'b, KoanStorageProfile>
    for (RegionBrand<'b>, Vec<(String, KObject<'b>)>)
{
    fn region_handle(&self) -> RegionHandle<'b, KoanStorageProfile> {
        self.0.handle()
    }
}

// Per-family `Stored` policy: which sub-arena each family lands in, plus `KObject`'s allocation
// address side-table hook. No stored family carries a self-targeting `Rc<FrameStorage>` — a stored
// closure / future / module is a bare borrow into its defining region, kept alive by its carrier's
// witness set rather than an owned anchor — so no allocation can self-cycle and the engine needs no
// cycle gate.

impl Stored<KoanStorageProfile> for KObject<'static> {
    fn cell(s: &StorageOf<KoanStorageProfile>) -> &FamilyArena<Self> {
        &s.0
    }
    fn record_local(frame: &KoanRegion, stored: &KObject<'static>) {
        frame.record_addr(stored as *const _ as usize);
    }
}

impl Stored<KoanStorageProfile> for KFunction<'static> {
    fn cell(s: &StorageOf<KoanStorageProfile>) -> &FamilyArena<Self> {
        &s.1 .0
    }
}

impl Stored<KoanStorageProfile> for Scope<'static> {
    fn cell(s: &StorageOf<KoanStorageProfile>) -> &FamilyArena<Self> {
        &s.1 .1 .0
    }
}

impl Stored<KoanStorageProfile> for Module<'static> {
    fn cell(s: &StorageOf<KoanStorageProfile>) -> &FamilyArena<Self> {
        &s.1 .1 .1 .0
    }
}

impl Stored<KoanStorageProfile> for ModuleSignature<'static> {
    fn cell(s: &StorageOf<KoanStorageProfile>) -> &FamilyArena<Self> {
        &s.1 .1 .1 .1 .0
    }
}

impl Stored<KoanStorageProfile> for KType<'static> {
    fn cell(s: &StorageOf<KoanStorageProfile>) -> &FamilyArena<Self> {
        &s.1 .1 .1 .1 .1 .0
    }
}

impl Stored<KoanStorageProfile> for OperatorGroup {
    fn cell(s: &StorageOf<KoanStorageProfile>) -> &FamilyArena<Self> {
        &s.1 .1 .1 .1 .1 .1 .0
    }
}

impl Stored<KoanStorageProfile> for FrameSet {
    fn cell(s: &StorageOf<KoanStorageProfile>) -> &FamilyArena<Self> {
        &s.1 .1 .1 .1 .1 .1 .1 .0
    }
}

/// Koan's at-will allocation entry and identity queries over the generic [`Region`] — an extension
/// trait because `Region` lives in the `workgraph` crate and a foreign type takes no inherent impls.
/// Every co-located `alloc_*` lives on [`RegionBrand`] (minted via [`FrameStorage::brand`]); a bare
/// `&KoanRegion` keeps only the identity surface here.
pub(crate) trait KoanRegionExt {
    /// The alloc-witnessed construction inversion's region-pure primitive: build a value into
    /// `owner`'s region *inside* the `yoke` closure, returning it bundled with the [`FrameSet`]
    /// singleton pinning `owner` so it is co-located by construction rather than paired with an
    /// asserted witness. The closure receives a per-construction [`RegionBrand`] confined to the
    /// `for<'b>` brand (it cannot escape the closure), so it allocates through the same handle as every
    /// other site. One primitive for both value families — the closure returns a `Carried::Object` (an
    /// [`alloc_object`](RegionBrand::alloc_object)) or a `Carried::Type` (an
    /// [`alloc_ktype`](RegionBrand::alloc_ktype)). A value that *references* another region's resident
    /// value folds that in with [`Witnessed::merge_pinned`] instead, unioning its reach; this primitive covers
    /// the case whose references are all region-derived or owned, so the `for<'b>` brand admits them.
    ///
    /// `build`'s return is spelled `<CarriedFamily as Reattachable>::At<'b>`, not the concrete
    /// `Carried<'b>`: the two are equal by the family's definition, but under the `for<'b>` binder the
    /// compiler does not normalize the projection lazily, so a `build` typed `-> Carried<'b>` fails to
    /// satisfy `yoke`'s `-> T::At<'b>` bound. Naming the projection makes the bounds syntactically
    /// identical. An inline closure returning a `Carried` still unifies fine at the call site.
    // Drives the object-family construction inversion
    // (design/per-node-memory.md): a region-pure leaf builds its `KObject` inside this closure.
    fn alloc_witnessed(
        owner: Rc<FrameStorage>,
        build: impl for<'b> FnOnce(RegionBrand<'b>) -> <CarriedFamily as Reattachable>::At<'b>,
    ) -> Witnessed<CarriedFamily, CarrierWitness>;

    /// `yoke` a value of **any** carrier family into `owner`'s region, handing the build closure a
    /// per-construction [`RegionBrand`] (confined to the `for<'b>` brand) so it allocates through the
    /// one capability. Generalizes [`alloc_witnessed`](Self::alloc_witnessed) (the `CarriedFamily`
    /// case) for the aggregate-accumulator yokes (`AggBuildFamily`) whose closures alloc into the dest
    /// region. The yoke hands a `&'b KoanRegion`; wrapping it as the brand is sound for the same reason
    /// the yoke is — the `for<'b>` quantifier admits only region-derived/owned references, so
    /// co-location holds by construction and nothing branded escapes the closure.
    fn yoke_branded<T: Reattachable, F>(
        owner: Rc<FrameStorage>,
        build: F,
    ) -> Witnessed<T, CarrierWitness>
    where
        F: for<'b> FnOnce(RegionBrand<'b>) -> T::At<'b>;

    /// Whether `ptr` was returned by a prior `alloc_object` on this region. `#[allow(dead_code)]`
    /// because trait methods, unlike inherent ones, are checked per compilation target, and the
    /// plain `--lib` build (no `cfg(test)`) can't see its only caller.
    #[allow(dead_code)]
    fn owns_object<'a>(&self, ptr: *const KObject<'a>) -> bool;
}

impl KoanRegionExt for KoanRegion {
    fn alloc_witnessed(
        owner: Rc<FrameStorage>,
        build: impl for<'b> FnOnce(RegionBrand<'b>) -> <CarriedFamily as Reattachable>::At<'b>,
    ) -> Witnessed<CarriedFamily, CarrierWitness> {
        Self::yoke_branded::<CarriedFamily, _>(owner, build)
    }

    fn yoke_branded<T: Reattachable, F>(
        owner: Rc<FrameStorage>,
        build: F,
    ) -> Witnessed<T, CarrierWitness>
    where
        F: for<'b> FnOnce(RegionBrand<'b>) -> T::At<'b>,
    {
        // `yoke_handle` into `owner`'s own region under the single-owner `Rc<FrameStorage>` witness
        // ([`WitnessRegion`]) — the brand proves the built value is region-derived — then
        // [`into_reference_only`](Witnessed::into_reference_only) re-bundles under the empty
        // reference-only carrier: the value's reach is exactly its own region, and its liveness is
        // external (the active frame during the step, the scheduler's retention hold once
        // finalized). Turbofish `T` at the yoke: inference does not drive `yoke`'s `T` from the
        // return type early enough to check `build`'s `-> T::At<'b>` bound, so it sees
        // `<_ as Reattachable>::At` and fails to match the projection.
        Witnessed::<T, Rc<FrameStorage>>::yoke_handle(owner, |handle| build(RegionBrand(handle)))
            .into_reference_only()
    }

    fn owns_object<'a>(&self, ptr: *const KObject<'a>) -> bool {
        // `KObject` is invariant in `'a`, so the through-`'static` cast is required despite
        // clippy's complaint.
        #[allow(clippy::unnecessary_cast)]
        let target = ptr as *const KObject<'static> as usize;
        self.owns_addr(target)
    }
}

/// Koan-branded wrappers over [`StepContext::alloc`]/[`StepContext::alloc_with`] — the closure
/// receives a [`RegionBrand`] (the koan allocation capability) rather than the bare `&KoanRegion`
/// the library-level context hands out, so a step construction site allocates through the one
/// capability every other site uses. Named with full words (`alloc_carried`, not `alloc`) to avoid
/// colliding with the generic verb each wraps. Lives here — not on `StepContext` itself — because
/// `RegionBrand`'s constructor is private to this module (see [`FrameStorage::brand`]).
/// [`Self::alloc_carried_with`] is how a finish folds a dep's reach into a carrier it builds from
/// that dep's value: the dep views only exist inside the shared brand, so a caller cannot smuggle
/// one out and seal it under a narrower reach than the fold produces.
pub(crate) trait KoanStepContextExt {
    /// [`StepContext::alloc`] with the closure receiving a [`RegionBrand`]: reach = own region only.
    fn alloc_carried(
        &self,
        build: impl for<'b> FnOnce(RegionBrand<'b>) -> <CarriedFamily as Reattachable>::At<'b>,
    ) -> Witnessed<CarriedFamily, CarrierWitness>;

    /// [`StepContext::alloc_with`] with the closure receiving a [`RegionBrand`] and the deps'
    /// views: the built carrier names every listed dep's reach **and residence host** (each dep
    /// arrives as its delivery envelope and folds at `Residence::Kept`), by construction.
    fn alloc_carried_with(
        &self,
        deps: &[&DeliveredCarried],
        build: impl for<'b> FnOnce(RegionBrand<'b>, Vec<Carried<'b>>) -> Carried<'b>,
    ) -> Witnessed<CarriedFamily, CarrierWitness>;

    /// [`Self::alloc_carried`] specialized to the one-`KType`-carrier shape: reach = own region
    /// only. For a `kt` that is region-pure (carries no borrow reaching outside this frame's own
    /// region) — the common case for a bind-time or synchronously-resolved type.
    fn alloc_type(&self, kt: KType<'_>) -> Witnessed<CarriedFamily, CarrierWitness>;

    /// [`Self::alloc_carried_with`] specialized to the one-`KType`-carrier shape: reach = own
    /// region unioned with every listed dep's reach. For a `kt` built from a dep terminal's value
    /// — the type's own borrows may reach into the dep's region, so the dep's carrier must fold
    /// into the result's witness. The dep views are unused here; the fold is what matters.
    fn alloc_type_with(
        &self,
        deps: &[&DeliveredCarried],
        kt: KType<'_>,
    ) -> Witnessed<CarriedFamily, CarrierWitness>;

    /// [`Self::alloc_carried_with`] specialized to the one-`KObject`-carrier shape: reach = own
    /// region unioned with every listed dep's reach. For a `value` built from (or projected out
    /// of) a dep terminal's value — its borrows may reach into the dep's region, so the dep's
    /// carrier must fold into the result's witness. The dep views are unused here; the fold is
    /// what matters.
    fn alloc_object_with(
        &self,
        deps: &[&DeliveredCarried],
        value: KObject<'_>,
    ) -> Witnessed<CarriedFamily, CarrierWitness>;
}

impl KoanStepContextExt for StepContext<FrameStorage> {
    fn alloc_carried(
        &self,
        build: impl for<'b> FnOnce(RegionBrand<'b>) -> <CarriedFamily as Reattachable>::At<'b>,
    ) -> Witnessed<CarriedFamily, CarrierWitness> {
        self.alloc_handle::<KoanStorageProfile, CarriedFamily>(|handle| build(RegionBrand(handle)))
    }

    fn alloc_carried_with(
        &self,
        deps: &[&DeliveredCarried],
        build: impl for<'b> FnOnce(RegionBrand<'b>, Vec<Carried<'b>>) -> Carried<'b>,
    ) -> Witnessed<CarriedFamily, CarrierWitness> {
        self.alloc_with_handle::<KoanStorageProfile, CarriedFamily, CarriedFamily>(
            deps,
            |handle, views| build(RegionBrand(handle), views),
        )
    }

    fn alloc_type(&self, kt: KType<'_>) -> Witnessed<CarriedFamily, CarrierWitness> {
        self.alloc_carried(|b| Carried::Type(b.alloc_ktype(kt)))
    }

    fn alloc_type_with(
        &self,
        deps: &[&DeliveredCarried],
        kt: KType<'_>,
    ) -> Witnessed<CarriedFamily, CarrierWitness> {
        // Scalar gate: a region-free scalar type references none of `deps`, so folding their reach in
        // would only over-retain. Route it to the no-fold path so it seals with an empty reach.
        if kt.is_region_free_scalar() {
            return self.alloc_type(kt);
        }
        self.alloc_carried_with(deps, |b, _views| Carried::Type(b.alloc_ktype(kt)))
    }

    fn alloc_object_with(
        &self,
        deps: &[&DeliveredCarried],
        value: KObject<'_>,
    ) -> Witnessed<CarriedFamily, CarrierWitness> {
        // Scalar gate: a shallow scalar embeds no borrow into any dep, so the dep-witness union is
        // pure over-retention. Route it to the no-fold path so an escaped scalar seals with an empty
        // reach and stops pinning its producer arena. Aggregates keep the fold (their reaches are
        // exact, so the residual is only a borrow the value could have embedded but did not).
        if value.is_shallow_scalar() {
            return self.alloc_carried(|b| Carried::Object(b.alloc_object(value)));
        }
        self.alloc_carried_with(deps, |b, _views| Carried::Object(b.alloc_object(value)))
    }
}

/// Test-only allocation counting over the generic [`Region`] — an extension trait for the same
/// reason as [`KoanRegionExt`].
#[cfg(test)]
pub(crate) trait KoanRegionTestExt {
    /// Total number of values stored across all eight sub-arenas. Each `alloc_*` writes to
    /// exactly one sub-arena, so this is the precise allocation count without double-counting.
    fn alloc_count(&self) -> usize;
}

#[cfg(test)]
impl KoanRegionTestExt for KoanRegion {
    fn alloc_count(&self) -> usize {
        self.family_len::<KObject<'static>>()
            + self.family_len::<KFunction<'static>>()
            + self.family_len::<Scope<'static>>()
            + self.family_len::<Module<'static>>()
            + self.family_len::<ModuleSignature<'static>>()
            + self.family_len::<KType<'static>>()
            + self.family_len::<OperatorGroup>()
            + self.family_len::<FrameSet>()
    }
}

#[cfg(test)]
impl CallFrame {
    /// Test alias for [`CallFrame::new`], kept so the many frame-construction tests share one
    /// construction name distinct from production call sites.
    pub(crate) fn new_test<'a>(
        outer: &'a Scope<'a>,
        outer_frame: Option<Rc<FrameStorage>>,
    ) -> Rc<CallFrame> {
        CallFrame::new(outer, outer_frame)
    }
}

/// Koan's per-call region owner: the library's [`RegionHost`], instantiated for the Koan family
/// set. `RegionHost` lazily mints its region on first allocation — reached by the child `Scope`
/// [`CallFrame::new`] builds immediately, so a constructed frame's region is minted by the time
/// anything reads it — and the `outer` link chains the lexical-ancestor frames' storage alive. An
/// escaping value (a returned closure, a module frame) pins *this* — not the [`CallFrame`] shell —
/// so a tail hop's shell can drop outright while the escapee's captured
/// environment rides the old `FrameStorage` it still holds.
/// The library's raw-region constructor is sealed to `workgraph`, so nothing outside the library
/// can mint a `KoanRegion` directly; the Koan-typed [`RegionBrand`] mint over a `FrameStorage` lives
/// on [`FrameStorageExt`] (an extension trait, since a type alias takes no inherent impls of its own).
pub type FrameStorage = RegionHost<KoanStorageProfile>;

/// The run-root storage: a fresh run region with no `outer` link. Held by `run_program` (and the
/// test harness) so the run-root scope's region has an owning Rc; [`CallFrame::adopting`] reuses
/// it as the run frame's storage and the run-root scope records a `Weak` to it as its
/// `region_owner`. Public: it is the one Koan-side entry point a caller (production or an
/// integration test) uses to obtain run-root storage — it mints nothing itself, only building the
/// library's `RegionHost` shell whose region lazily mints on first allocation.
pub fn run_root_storage() -> Rc<FrameStorage> {
    RegionHost::fresh(None)
}

/// Koan's [`RegionBrand`] mint over a [`FrameStorage`] — an extension trait because `FrameStorage`
/// is a `workgraph` type alias, so Koan cannot add an inherent method to it directly.
pub(crate) trait FrameStorageExt {
    /// Mint this storage's region's [`RegionBrand`] — the **sole** allocation capability for this
    /// storage's region. Minting is the library's [`RegionHandle::from_owner`] rule (it requires the
    /// storage that *owns* the region, via its `RegionOwner` impl); this method pairs it with the
    /// Koan veneer. Allocation is reachable only by riding this brand (it is stored on the [`Scope`]
    /// built at region-open, and threaded from there).
    fn brand(&self) -> RegionBrand<'_>;
}

impl FrameStorageExt for FrameStorage {
    fn brand(&self) -> RegionBrand<'_> {
        RegionBrand(RegionHandle::from_owner(self))
    }
}

/// The reach set backing carrier witnesses: the set of `Rc<FrameStorage>` whose regions a
/// carrier's value reaches. See [`RegionSet`] for the shared mechanism (subsumption, folding,
/// union); Koan's member semantics are the library's [`PinsRegion`](crate::witnessed::PinsRegion)
/// impl for [`RegionHost`].
pub type FrameSet = RegionSet<FrameStorage>;

/// Build a per-call frame's child scope **witnessed**, sealing it to the externally-witnessed
/// [`SealedExtern<ScopeRefFamily>`] the [`CallFrame`] holds — the construction door that re-anchors the
/// longer-lived lexical parent into the fresh region, with no retype outside the witnessed substrate.
///
/// The fresh region's [`RegionHandle`] and the foreign parent (as [`ScopeRefFamily`]) are erased and
/// [`zip`](SealedExtern::zip)ped, then opened at **one** `for<'b>` brand against `storage` — the fresh
/// frame's `Rc`, which pins both the region it owns and, via its `outer` chain, the parent. Inside
/// the brand the real invariant `Scope<'b>` is built coupling the parent at `'b` (its `root`
/// falling out as `outer.root`), allocated through the brand's [`RegionBrand`], and erased witness-less.
/// `Scope`'s invariance is honoured by construction — the only retypes are the substrate's audited brand
/// ([`SealedExtern::open`]) and store ([`RegionHandle::alloc`]) — so the per-call child stops being a
/// re-anchor audited outside Witnessed/Sealed. Branding the two refs at *independent* `'b`s is what
/// invariance rejects; one [`zip`](SealedExtern::zip)ped `open` unifies them at a single `'b`.
pub(crate) fn build_frame_child_witnessed<'p>(
    outer: &'p Scope<'p>,
    storage: &Rc<FrameStorage>,
) -> Sealed<ScopeRefFamily, CarrierWitness> {
    let handle = SealedExtern::<RegionHandleFamily<KoanStorageProfile>>::erase(
        RegionHandle::from_owner(&**storage),
    );
    let parent = SealedExtern::<ScopeRefFamily>::erase(outer);
    let region_owner = Rc::downgrade(storage);
    handle.zip(parent).open(storage, |(handle_b, outer_b)| {
        // `handle_b: RegionHandle<'b, KoanStorageProfile>`, `outer_b: &'b Scope<'b>` — the region
        // handle and parent unified at the one brand. The child stores both by plain coercion (no
        // retype of its own). The child scope lives in `storage`'s own region, so it seals under the
        // empty (`resident`) carrier witness — its liveness is the frame storage, paired with it as the
        // envelope host by the `CallFrame` constructor.
        let child = Scope::child_for_frame_witnessed(outer_b, RegionBrand(handle_b), region_owner);
        handle_b.alloc::<Scope<'static>, _>(child, |live| {
            Sealed::seal(Witnessed::<ScopeRefFamily, CarrierWitness>::resident(live))
        })
    })
}

/// One user-fn call's allocation frame: a thin shell over a refcounted [`FrameStorage`]. `Rc`-pinned
/// so the scheduler manages the frame by `Rc<CallFrame>`; an escaping closure extends only the
/// *storage* (via [`Self::storage_rc`]), not the shell, so a `FreshTail` tail hop can drop this
/// frame's shell outright without foreclosing on the escapee.
///
/// See [per-call-region/README.md](../../../design/per-call-region/README.md) for the
/// carrier set, escaping-value retention, ancestor chain, and TCO
/// frame reuse; [memory-model.md § Region lifetime erasure](../../../design/memory-model.md#region-lifetime-erasure)
/// for the heap-pinning / drop-order invariants.
pub struct CallFrame {
    /// The per-call child scope paired with the frame storage that owns its region, as one delivery
    /// [`Delivered`] envelope: the storage is the envelope's retained host, the scope its
    /// empty-witness ([`resident`](Witnessed::resident)) carrier, read back through
    /// [`Self::with_scope`] / [`Self::scope_sealed`] under that host pin. Co-ownership by one value
    /// replaces the former hand-maintained `(storage, scope_carrier)` field pair: the
    /// storage-pins-the-scope co-location the pair kept by field-order convention is now a
    /// construction invariant of the envelope, and dropping the sealed carrier never dereferences the
    /// child pointer, so no drop-order rule is left to hand-maintain.
    envelope: Delivered<ScopeRefFamily, CarrierWitness, FrameStorage>,
    /// True only for the scheduler-owned run frame, which carries the top-level run scope and
    /// never drops mid-run. Its `region` is empty (top-level values live in the externally-owned
    /// run region, reached via `scope.region`), so there is nothing to lift out of it: the Done
    /// boundary skips the lift for a non-dying frame (lift exists to rescue values from a *dying*
    /// per-call region). Every per-call frame is `false`.
    non_dying: bool,
    /// The slot this frame was installed for — the body that finalizes it. Set at install; checked at
    /// that slot's `Done` / tail-`Continue` to close the frame's scope exactly when its body completes.
    /// A `Yoked` sub-expression slot sharing the frame is not the owner, so its `Done` does not close.
    owner: Cell<Option<NodeId>>,
}

impl CallFrame {
    /// Build a fresh per-call frame whose child `Scope` uses `outer` as its `outer` link.
    /// `outer_frame` must hold the parent frame's `FrameStorage` Rc when the parent is per-call;
    /// `None` when the parent is run-root — a dispatched frame strong-owns no ancestor, so an
    /// escaping value kept alive by a consumer scope's reach-set forms no back-edge.
    pub fn new<'p>(outer: &'p Scope<'p>, outer_frame: Option<Rc<FrameStorage>>) -> Rc<CallFrame> {
        // The storage is heap-pinned behind its own `Rc` from this point on (its region minted
        // lazily, on the child scope's allocation below), so the erased child-scope pointer stays
        // valid as the storage Rc moves into the shell.
        let storage = RegionHost::fresh(outer_frame);
        // The child scope is born externally-witnessed through the construction door: it brands the
        // fresh region and the longer-lived lexical parent at one `for<'b>`, builds the real invariant
        // `Scope<'b>` coupling them, allocs it through the brand, and erases it straight into a
        // `SealedExtern` — no transient `&'a` minted, no re-anchor outside the substrate. The local
        // borrow of `storage` ends here (the carrier holds a `&'static` reference, not a borrow of
        // `storage`), so `storage` moves into the shell below; the `KoanRegion` stays at a fixed heap
        // address behind the Rc, keeping the erased reference valid.
        let scope_carrier = build_frame_child_witnessed(outer, &storage);
        Rc::new(CallFrame {
            envelope: Delivered::hosted(scope_carrier, storage),
            non_dying: false,
            owner: Cell::new(None),
        })
    }

    /// The scheduler-owned **run frame**: a frame that *carries an already-built run scope*
    /// rather than minting a child. Top-level execution runs against this frame so `active_frame`
    /// is never `None`, which makes a body's re-dispatch-against-its-own-scope uniformly framed
    /// (Yoked) at every depth — top level included. Marked `non_dying` so the Done boundary skips
    /// the (pointless) self-lift of top-level results.
    ///
    /// `run_storage` is the `Rc<FrameStorage>` that owns the run region — the same storage `scope`
    /// (the run root) lives in. Adopting it (rather than minting an empty region) makes this frame's
    /// `region()` equal the run-root region, so a top-level-defined FN's captured-region owner
    /// resolves to this frame's storage. The adopted run scope's borrow is erased into
    /// `scope_carrier` exactly as every per-call child scope is — the fabrication hazard is deferred
    /// to the witness-bounded re-attach.
    pub fn adopting<'a>(scope: &'a Scope<'a>, run_storage: Rc<FrameStorage>) -> Rc<CallFrame> {
        debug_assert!(
            std::ptr::eq(run_storage.region(), scope.region() as *const KoanRegion),
            "adopting run_storage must own the run-root scope's region"
        );
        let scope_carrier =
            Sealed::seal(Witnessed::<ScopeRefFamily, CarrierWitness>::resident(scope));
        Rc::new(CallFrame {
            envelope: Delivered::hosted(scope_carrier, run_storage),
            non_dying: true,
            owner: Cell::new(None),
        })
    }

    /// True only for the scheduler-owned run frame (see [`Self::adopting`]). The Done boundary
    /// reads this to skip the self-lift that a never-dying frame would otherwise perform.
    pub fn non_dying(&self) -> bool {
        self.non_dying
    }

    /// Record the slot that finalizes this frame's scope (the body installed into it). Read by the
    /// finalize-time close so it seals exactly the scope whose body just completed.
    pub fn set_owner(&self, slot: NodeId) {
        self.owner.set(Some(slot));
    }

    /// The slot that finalizes this frame's scope, if installed.
    pub fn owner(&self) -> Option<NodeId> {
        self.owner.get()
    }

    /// This frame's own `FrameStorage` — the envelope's retained host, which every constructor
    /// pairs with the child scope.
    fn storage(&self) -> &Rc<FrameStorage> {
        self.envelope.host()
    }

    /// The child scope's externally-witnessed carrier by value (`SealedExtern<ScopeRefFamily>` is
    /// `Copy`) — the run-loop step's source for a `Yoked` slot, opened at the step brand alongside the
    /// continuation / contract / deps instead of re-anchored through the borrow-bounded `attach`.
    /// Reconstructed from the envelope's sealed carrier: the same erased `&Scope`, exposed witness-less
    /// so it [`zip`](SealedExtern::zip)s with the step's other externally-witnessed carriers under one
    /// brand (the envelope host is folded into that step witness separately).
    pub(crate) fn scope_sealed(&self) -> SealedExtern<ScopeRefFamily> {
        SealedExtern::seal(*self.envelope.cell().erased())
    }

    /// Run `f` with this frame's child scope opened at a `for<'b>` brand — the sole scope read, folded
    /// onto `open` like the decide channel. Both the frame-side reads (scope id, the arg reach-set
    /// fold) and the seed-side binds (the MATCH / TRY arm `it`-bind, the user-fn param-bind, the
    /// deferred-return-type elaboration) take this read: a seed relocates its caller-`'a` value into
    /// the opened scope's own region through the substrate (a witnessed shortening) before binding it,
    /// so nothing fabricates a free `&'a`. The carrier opens against this frame's own storage `Rc`
    /// (the pin), and the rank-2 brand keeps the `&Scope<'b>` from escaping the call, so no scope
    /// borrow rides up a `&mut self` path. Carries **no `unsafe`** — [`Delivered::open`] routes the
    /// substrate's single audited reattach, pinned by the envelope's own retained host.
    pub fn with_scope<R>(&self, f: impl for<'b> FnOnce(&'b Scope<'b>) -> R) -> R {
        self.envelope.open(f)
    }

    /// This frame's child scope id, copied out through [`Self::with_scope`] — the scalar read for the
    /// sites that need only the id, with no `&Scope` escaping the open.
    pub fn scope_id(&self) -> ScopeId {
        self.with_scope(|s| s.id)
    }

    pub fn region(&self) -> &KoanRegion {
        self.storage().region()
    }

    /// This frame's region [`RegionBrand`] allocation capability, minted from its owning storage.
    /// Test-only: production allocates through the scope (`scope.brand()`); the frame-level handle is
    /// a convenience for the arena / lift Miri tests that alloc against a bare frame.
    #[cfg(test)]
    pub(crate) fn brand(&self) -> RegionBrand<'_> {
        self.storage().brand()
    }

    /// Clone this frame's `FrameStorage` Rc — the handle an escaping value (a returned closure, a
    /// module frame) pins to keep its captured environment alive independently of the shell: a
    /// `FreshTail` tail hop drops this frame's shell outright, and the escaped storage clone keeps
    /// the region it names alive regardless.
    pub fn storage_rc(&self) -> Rc<FrameStorage> {
        Rc::clone(self.storage())
    }
}

#[cfg(test)]
mod tests;
