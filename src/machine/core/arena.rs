//! The Koan instantiation of the generic [`Region`](crate::witnessed::Region)
//! storage substrate: `KoanRegion = Region<KoanStorageProfile>`, the per-family
//! [`Stored`](crate::witnessed::Stored) impls (which sub-arena a family lands in), and the
//! Koan-typed `alloc_*` wrappers. `CallFrame`
//! — the per-call frame shell over a refcounted `FrameStorage` (the `KoanRegion` plus the ancestor
//! chain), holding the child `Scope` and resetting in place for TCO — also lives here.
//!
//! The generic erase-store engine lives in [`crate::witnessed::region`]; this file supplies the
//! Koan policy it runs.
//!
//! See [per-call-region/README.md](../../../design/per-call-region/README.md) for the carrier
//! set, escaping-value retention, ancestor chain, and TCO frame reuse;
//! [memory-model.md § Arena lifetime erasure](../../../design/memory-model.md#region-lifetime-erasure)
//! for the heap-pinning / drop-order invariants.

use std::cell::Cell;
use std::rc::Rc;

use typed_arena::Arena;

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
    PinsRegion, Reattachable, Region, RegionOwner, RegionSet, StepContext, StorageProfile, Stored,
    WitnessRegion, Witnessed,
};

/// The Koan storage bundle: one typed sub-arena per stored family. Each sub-arena stores the
/// family's `'static` form (phantom); the [`Region`] engine re-anchors to the caller's `'a`
/// on the way out. The `KType` region backs per-type identity binding storage (`Bindings::types`);
/// the `OperatorGroup` region backs the per-scope operator registry (`Bindings::operators`).
#[derive(Default)]
pub struct KoanStorage {
    objects: Arena<KObject<'static>>,
    functions: Arena<KFunction<'static>>,
    scopes: Arena<Scope<'static>>,
    modules: Arena<Module<'static>>,
    signatures: Arena<ModuleSignature<'static>>,
    ktypes: Arena<KType<'static>>,
    operator_groups: Arena<OperatorGroup>,
}

/// The Koan workload: binds the generic [`Region`] to the Koan family set.
pub struct KoanStorageProfile;

impl StorageProfile for KoanStorageProfile {
    type Storage = KoanStorage;
}

/// Run-lifetime allocator. A [`Region`] carrying the Koan family set; lives for one program
/// run. The `KoanRegion` references across the tree and the `Rc<CallFrame>` back-edge ride this
/// alias unchanged.
pub type KoanRegion = Region<KoanStorageProfile>;

/// The frame-lifetime **allocation capability** for a [`KoanRegion`] — a `Copy` newtype over a
/// `&'a KoanRegion` that carries every `alloc_*` method. It is the *only* way to allocate into a
/// region: a bare `&KoanRegion` (the references that "scope around" — `scope.region()`,
/// `frame.region()`) exposes identity queries (`owns_object`) but **no**
/// `alloc_*`, so nothing can mint a region resident from an ambient region reference and "an
/// allocated value is always born inside the Witnessed/Sealed abstraction" is a type rule, not an
/// audited convention.
///
/// **Un-forgeable from a bare region.** The wrapped field is private to this module and the sole
/// public minter is [`FrameStorage::brand`], which requires `&FrameStorage` — the storage that owns
/// the region. The ambient `&KoanRegion` holders never hold the `FrameStorage`, so they cannot mint a
/// brand; the capability reaches an allocation site only by riding the [`Scope`] (which stores the
/// brand it was built under) or being threaded as a parameter.
///
/// **Frame-lifetime, not a per-alloc `for<'b>` brand.** A structural resident (a binding entry, a
/// `Module`'s child `&Scope`) must outlive any one brand window, so it needs a real `&'a` — which only
/// a frame-lifetime handle hands back. The per-alloc `for<'b>` brand is the right tool for *terminals*
/// (the witnessed surface, where [`Region::alloc`] hands a `for<'b>` brand and returns a `Witnessed`
/// carrier); this handle is for the co-located plumbing.
///
/// A bare `&KoanRegion` exposes **no** `alloc_*` — allocation is reachable only through this brand, so
/// nothing can mint a region resident from an ambient region reference:
///
/// ```compile_fail
/// // `alloc_object` lives on `RegionBrand`, not the bare region: no such method on `&KoanRegion`.
/// let region = koan::machine::KoanRegion::new();
/// let _ = region.alloc_object(todo!());
/// ```
#[derive(Clone, Copy)]
pub struct RegionBrand<'a>(&'a KoanRegion);

impl<'a> RegionBrand<'a> {
    /// The bare region this brand authorizes — for identity compares (`ptr::eq`, `pins_region`). A
    /// bare `&KoanRegion` cannot be turned *back* into a brand (the field is private and the only
    /// minter is [`FrameStorage::brand`]), so handing out the identity reference opens no hole.
    pub fn region(self) -> &'a KoanRegion {
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
                self.0 as *const KoanRegion,
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

    /// The witnessed-allocation surface for an **owned, region-pure** object — a value referencing no
    /// other region: born witnessed by the **empty** (foreign-reach-only) set. The brand-confined
    /// [`alloc`](Region::alloc) stores `value` and hands the freshly-stored `&'b KObject<'b>` to the
    /// closure at the brand, which bundles it through [`Witnessed::resident`] — the empty-witness
    /// constructor that names the region-pure obligation, so the active frame is deliberately excluded.
    /// The producing frame is folded in only at finalize/close (the scope-reach seal), so a
    /// region-resident value never strong-owns its own frame (the `region → object → frame` cycle that
    /// would defeat the `Rc::get_mut` TCO gate).
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
    ) -> Witnessed<CarriedFamily, FrameSet> {
        self.0
            .alloc::<KObject<'static>, _>(value, |live| Witnessed::resident(Carried::Object(live)))
    }

    /// The witnessed-allocation surface for an **owned, region-pure** type — the type-channel twin of
    /// [`alloc_object_witnessed`](Self::alloc_object_witnessed). Born witnessed by the **empty**
    /// (foreign-reach-only) set: the brand-confined [`alloc`](Region::alloc) stores `value` and hands
    /// the freshly-stored `&'b KType<'b>` to the closure, which bundles it through
    /// [`Witnessed::resident`]. The producing frame is folded in only at the seal/close (the
    /// scope-reach seal), so a region-resident type never strong-owns its own frame.
    ///
    /// Region-pure is the precondition: a `KType` built fresh inside the brand referencing no other
    /// region — owned data, or a borrow this region already pins. A `KType::Module` reaches its child
    /// scope's region, so its carrier is not born on this surface: it is sealed by
    /// [`Scope::resident_type_carrier`](crate::machine::core::Scope) under the child-scope reach folded
    /// at construction.
    pub(crate) fn alloc_ktype_witnessed(
        self,
        value: KType<'_>,
    ) -> Witnessed<CarriedFamily, FrameSet> {
        self.0
            .alloc::<KType<'static>, _>(value, |live| Witnessed::resident(Carried::Type(live)))
    }

    /// Bundle a value **already resident in this brand's region** under `witness` — the terminal
    /// carrier a name / ATTR read hands back and an FN-def / LET define site seals its object with.
    /// Unlike [`alloc_object_witnessed`](Self::alloc_object_witnessed) the value is not stored here;
    /// it pre-exists in the region, so it is bundled through [`Witnessed::resident`] — a within-step
    /// transient (the reading / defining frame pins the region for the step) — and immediately
    /// [`reseal_under`](Witnessed::reseal_under) its own reach, fixing the carrier's witness before it
    /// is stored on a node. Confines [`Witnessed::resident`] to this arena surface, so no read / define
    /// builtin reaches for it. `witness` must name the value's full reach (its home frame ∪ its
    /// home-omitted foreign reach); the caller
    /// ([`Scope::resident_value_carrier`](crate::machine::core::Scope)) folds it. The brand is the
    /// capability marker: only a handle into the region the value lives in may re-seal it resident.
    pub(crate) fn seal_resident(
        self,
        carried: Carried<'_>,
        witness: FrameSet,
    ) -> Witnessed<CarriedFamily, FrameSet> {
        let _ = self.0;
        Witnessed::resident(carried).reseal_under(witness)
    }
}

/// The workload's value-relocation hook: structurally copy a [`Carried`] into `dest`'s region so the
/// copy outlives the producer's dying frame. Only the top-level node is re-allocated into `dest`; the
/// composite spine shares its `Rc` payloads ([`KObject::deep_clone`]), and a `KFunction` / first-class
/// `Module` rides a *bare* borrow into its defining region — preserved verbatim, never deep-copied (a
/// closure may reference anything reachable from its captured scope). Those surviving borrows are kept
/// alive by the carrier's witness set ([`FrameSet`]), which
/// [`Sealed::transfer_into`](crate::witnessed::Sealed::transfer_into) assembles as the set union of the
/// producer's reached regions and `dest` — so this hook owns only the copy, never a region anchor.
///
/// Runs at the destination brand `'b`, so the copy allocs into `dest` natively: no fabricated
/// lifetime, no caller `unsafe`. Lives in core alongside [`RegionBrand`] so the
/// [`DepTerminal`](super::kfunction::action::DepTerminal) relocation (named in the builtin-`Action`
/// currency) reaches it without depending on the execute layer.
pub(crate) fn relocate_carried<'b>(value: Carried<'b>, dest: RegionBrand<'b>) -> Carried<'b> {
    match value {
        Carried::Object(v) => Carried::Object(dest.alloc_object(v.deep_clone())),
        Carried::Type(t) => Carried::Type(dest.alloc_ktype(t.clone())),
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

/// `Reattachable` family for a **reference** to a [`KoanRegion`] — `&'r KoanRegion`, a thin pointer
/// (`KoanRegion` is lifetime-free, so the reference is layout-invariant in `'r`). It lets the fresh
/// per-call region erase and re-brand at the construction-door brand alongside the foreign parent
/// scope, so [`build_frame_child_witnessed`] can present the region as a `RegionBrand<'b>` at the very
/// `'b` the parent re-anchors to — the unification the per-call frame child needs.
pub struct BareRegionFamily;
reattachable!(BareRegionFamily => &'r KoanRegion);

// Per-family `Stored` policy: which sub-arena each family lands in, plus `KObject`'s allocation
// address side-table hook. No stored family carries a self-targeting `Rc<FrameStorage>` — a stored
// closure / future / module is a bare borrow into its defining region, kept alive by its carrier's
// witness set rather than an owned anchor — so no allocation can self-cycle and the engine needs no
// cycle gate.

impl Stored<KoanStorageProfile> for KObject<'static> {
    fn sub_arena(s: &KoanStorage) -> &Arena<KObject<'static>> {
        &s.objects
    }
    fn record_local(frame: &KoanRegion, stored: &KObject<'static>) {
        frame.record_addr(stored as *const _ as usize);
    }
}

impl Stored<KoanStorageProfile> for KType<'static> {
    fn sub_arena(s: &KoanStorage) -> &Arena<KType<'static>> {
        &s.ktypes
    }
}

impl Stored<KoanStorageProfile> for KFunction<'static> {
    fn sub_arena(s: &KoanStorage) -> &Arena<KFunction<'static>> {
        &s.functions
    }
}

impl Stored<KoanStorageProfile> for Scope<'static> {
    fn sub_arena(s: &KoanStorage) -> &Arena<Scope<'static>> {
        &s.scopes
    }
}

impl Stored<KoanStorageProfile> for Module<'static> {
    fn sub_arena(s: &KoanStorage) -> &Arena<Module<'static>> {
        &s.modules
    }
}

impl Stored<KoanStorageProfile> for ModuleSignature<'static> {
    fn sub_arena(s: &KoanStorage) -> &Arena<ModuleSignature<'static>> {
        &s.signatures
    }
}

impl Stored<KoanStorageProfile> for OperatorGroup {
    fn sub_arena(s: &KoanStorage) -> &Arena<OperatorGroup> {
        &s.operator_groups
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
    /// value folds that in with [`Witnessed::merge`] instead, unioning its reach; this primitive covers
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
    ) -> Witnessed<CarriedFamily, FrameSet>;

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
    ) -> Witnessed<T, FrameSet>
    where
        F: for<'b> FnOnce(RegionBrand<'b>) -> T::At<'b>;

    /// Whether `ptr` was returned by a prior `alloc_object` on this region. Currently only called
    /// from test code (verifying the relocate-into-`dest` invariant); `#[allow(dead_code)]` because
    /// trait methods, unlike inherent ones, are checked per compilation target, and the plain `--lib`
    /// build (no `cfg(test)`) can't see that caller.
    #[allow(dead_code)]
    fn owns_object<'a>(&self, ptr: *const KObject<'a>) -> bool;
}

impl KoanRegionExt for KoanRegion {
    fn alloc_witnessed(
        owner: Rc<FrameStorage>,
        build: impl for<'b> FnOnce(RegionBrand<'b>) -> <CarriedFamily as Reattachable>::At<'b>,
    ) -> Witnessed<CarriedFamily, FrameSet> {
        Self::yoke_branded::<CarriedFamily, _>(owner, build)
    }

    fn yoke_branded<T: Reattachable, F>(owner: Rc<FrameStorage>, build: F) -> Witnessed<T, FrameSet>
    where
        F: for<'b> FnOnce(RegionBrand<'b>) -> T::At<'b>,
    {
        // `yoke` into `owner`'s own region under the single-owner `Rc<FrameStorage>` witness
        // ([`WitnessRegion`]), then [`into_set`](Witnessed::into_set) widens that one pin into the
        // `FrameSet` the aggregate world accumulates in — `yoke`'s source == bundle identity preserved,
        // the single→set widening a separate lift. Turbofish `T` at the yoke: inference does not drive
        // `yoke`'s `T` from the return type early enough to check `build`'s `-> T::At<'b>` bound, so it
        // sees `<_ as Reattachable>::At` and fails to match the projection.
        Witnessed::<T, Rc<FrameStorage>>::yoke(owner, |region| build(RegionBrand(region)))
            .into_set::<FrameSet>()
    }

    fn owns_object<'a>(&self, ptr: *const KObject<'a>) -> bool {
        // `KObject` is invariant in `'a`, so the through-`'static` cast is required despite
        // clippy's complaint.
        #[allow(clippy::unnecessary_cast)]
        let target = ptr as *const KObject<'static> as usize;
        self.owns_addr(target)
    }
}

/// Koan-branded wrapper over [`StepContext::alloc`] — the closure receives a [`RegionBrand`] (the koan
/// allocation capability) rather than the bare `&KoanRegion` the library-level context hands out, so a
/// step construction site allocates through the one capability every other site uses. Named with full
/// words (`alloc_carried`, not `alloc`) to avoid colliding with the generic verb it wraps. Lives here —
/// not on `StepContext` itself — because `RegionBrand`'s constructor is private to this module (see
/// [`FrameStorage::brand`]). The dep-folding [`StepContext::alloc_with`] has no koan consumer yet — the
/// value-copy finishes relocate their surviving deps rather than witness-construct from them — so it is
/// reached through the library method directly when a caller needs it.
pub(crate) trait KoanStepContextExt {
    /// [`StepContext::alloc`] with the closure receiving a [`RegionBrand`]: reach = own region only.
    fn alloc_carried(
        &self,
        build: impl for<'b> FnOnce(RegionBrand<'b>) -> <CarriedFamily as Reattachable>::At<'b>,
    ) -> Witnessed<CarriedFamily, FrameSet>;
}

impl KoanStepContextExt for StepContext<FrameStorage> {
    fn alloc_carried(
        &self,
        build: impl for<'b> FnOnce(RegionBrand<'b>) -> <CarriedFamily as Reattachable>::At<'b>,
    ) -> Witnessed<CarriedFamily, FrameSet> {
        self.alloc::<CarriedFamily, FrameSet>(|region| build(RegionBrand(region)))
    }
}

/// Test-only allocation counting over the generic [`Region`] — an extension trait for the same
/// reason as [`KoanRegionExt`].
#[cfg(test)]
pub(crate) trait KoanRegionTestExt {
    /// Total number of values stored across all seven sub-arenas. Each `alloc_*` writes to
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

    /// Test alias for [`CallFrame::try_reset_for_tail`].
    pub(crate) fn try_reset_for_tail_test<'a>(
        self: &mut Rc<Self>,
        new_outer: &'a Scope<'a>,
    ) -> bool {
        self.try_reset_for_tail(new_outer)
    }
}

/// A frame's refcounted storage: the per-call `KoanRegion` plus the `outer` link that keeps
/// the lexical-ancestor frames' storage alive. An escaping value (a returned closure, a module
/// frame) pins *this* — not the [`CallFrame`] shell — so the shell stays uniquely owned and the
/// scheduler can reuse it for the next tail iteration while the escapee's captured environment
/// rides the old `FrameStorage` it still holds. Field order is load-bearing: `region` drops
/// before `outer`, so inner pointers die before the outer storage they may reference.
pub struct FrameStorage {
    region: KoanRegion,
    /// The parent per-call frame's storage: both a liveness pin — held so the ancestor frames'
    /// storage outlives this child's `outer` scope pointer — and the link [`FrameStorage::pins_region`]
    /// walks for [`FrameSet`] subsumption. Drop tears down the chain in order.
    outer: Option<Rc<FrameStorage>>,
}

impl FrameStorage {
    /// The run-root storage: a fresh run region with no `outer` link. Held by `run_program` (and the
    /// test harness) so the run-root scope's region has an owning Rc; [`CallFrame::adopting`] reuses
    /// it as the run frame's storage and the run-root scope records a `Weak` to it as its
    /// `region_owner`.
    pub fn run_root() -> Rc<FrameStorage> {
        Rc::new(FrameStorage {
            region: KoanRegion::new(),
            outer: None,
        })
    }

    /// The backing `KoanRegion`. Used for region-identity comparisons (e.g. [`FrameSet`]
    /// subsumption) by holders that pin storage but never name a `CallFrame`.
    pub(crate) fn region(&self) -> &KoanRegion {
        &self.region
    }

    /// Mint the region's [`RegionBrand`] — the **sole** allocation capability for this storage's
    /// region. The minter requires `&FrameStorage` (the storage that *owns* the region), so a holder
    /// of a bare `&KoanRegion` — the references that scope around — cannot mint one: allocation is
    /// reachable only by riding this brand (it is stored on the [`Scope`] built at region-open, and
    /// threaded from there). This is the one place an allocation capability comes into existence.
    pub(crate) fn brand(&self) -> RegionBrand<'_> {
        RegionBrand(&self.region)
    }

    /// True iff holding `self`'s `Rc` keeps the region at `region_ptr` alive — `self`'s own region or
    /// any of its `outer` ancestors (each pinned by the chain). The subsumption test [`FrameSet`]'s
    /// union uses: a member whose region another member already pins is redundant.
    pub(crate) fn pins_region(&self, region_ptr: *const KoanRegion) -> bool {
        let mut node = self;
        loop {
            // `node.region()` coerces `&KoanRegion → *const` for the address compare (as `rc_targets`).
            if std::ptr::eq(node.region(), region_ptr) {
                return true;
            }
            match &node.outer {
                Some(outer) => node = outer,
                None => return false,
            }
        }
    }
}

/// The reach set backing carrier witnesses: the set of `Rc<FrameStorage>` whose regions a
/// carrier's value reaches. See [`RegionSet`] for the shared mechanism (subsumption, folding,
/// union); Koan's member semantics are the [`PinsRegion`] impl below.
pub type FrameSet = RegionSet<FrameStorage>;

// SAFETY: a held `Rc<FrameStorage>` keeps its owned `FrameStorage` — and the `KoanRegion` field within
// it, along with the arena pages a value lives in — at a fixed heap address for the whole life of the
// `Rc` (`Rc` is `StableDeref`), so `region()` returns a reference into storage the `RegionOwner` blanket
// impl's `Rc<F>: WitnessRegion` pins: a value built solely from that region is pinned by holding the
// `Rc`. A single held `Rc<FrameStorage>` pins exactly one region — its own — which *is* the
// single-region `yoke` precondition, now a type rather than a runtime narrowing of a set that could be
// empty or multi.
unsafe impl RegionOwner for FrameStorage {
    type Region = KoanRegion;
    fn region(&self) -> &KoanRegion {
        FrameStorage::region(self)
    }
}

// SAFETY: `pins_region` walks self's own region and its `outer` ancestor chain; holding self's `Rc`
// holds each ancestor `Rc` in turn, so every region the walk reports pinned stays live and
// fixed-address while self is held.
unsafe impl PinsRegion for FrameStorage {
    fn pins_region(&self, region: &KoanRegion) -> bool {
        FrameStorage::pins_region(self, region)
    }
}

/// Build a per-call frame's child scope **witnessed**, sealing it to the externally-witnessed
/// [`SealedExtern<ScopeRefFamily>`] the [`CallFrame`] holds — the construction door that re-anchors the
/// longer-lived lexical parent into the fresh region, with no retype outside the witnessed substrate.
///
/// The fresh region (as [`BareRegionFamily`]) and the foreign parent (as [`ScopeRefFamily`]) are erased
/// and [`zip`](SealedExtern::zip)ped, then opened at **one** `for<'b>` brand against `storage` — the
/// fresh frame's `Rc`, which pins both the region it owns and, via its `outer` chain, the parent. Inside
/// the brand the real invariant `Scope<'b>` is built coupling the parent at `'b` (its `root`
/// falling out as `outer.root`), allocated through the brand's [`RegionBrand`], and erased witness-less.
/// `Scope`'s invariance is honoured by construction — the only retypes are the substrate's audited brand
/// ([`SealedExtern::open`]) and store ([`Region::alloc`]) — so the per-call child stops being a re-anchor
/// audited outside Witnessed/Sealed. The earlier single-`with_branded_ref`-per-ref attempt branded the
/// two at *independent* `'b`s, which invariance rejects; one [`zip`](SealedExtern::zip)ped `open`
/// unifies them.
pub(crate) fn build_frame_child_witnessed<'p>(
    outer: &'p Scope<'p>,
    storage: &Rc<FrameStorage>,
) -> SealedExtern<ScopeRefFamily> {
    let region = SealedExtern::<BareRegionFamily>::erase(storage.region());
    let parent = SealedExtern::<ScopeRefFamily>::erase(outer);
    let region_owner = Rc::downgrade(storage);
    region.zip(parent).open(storage, |(region_b, outer_b)| {
        // `region_b: &'b KoanRegion`, `outer_b: &'b Scope<'b>` — the region and parent unified at the
        // one brand. The child stores both by plain coercion (no retype of its own).
        let child = Scope::child_for_frame_witnessed(outer_b, RegionBrand(region_b), region_owner);
        region_b
            .alloc::<Scope<'static>, _>(child, |live| SealedExtern::<ScopeRefFamily>::erase(live))
    })
}

/// One user-fn call's allocation frame: a thin shell over a refcounted [`FrameStorage`]. `Rc`-pinned
/// so the scheduler manages the frame by `Rc<CallFrame>`; an escaping closure extends only the
/// *storage* (via [`Self::storage_rc`]), not the shell, so tail reuse can reset the shell's storage
/// without foreclosing on the escapee. Field order is load-bearing: `storage` drops before
/// `scope_carrier`, so the region tears down before the now-dangling child reference.
///
/// See [per-call-region/README.md](../../../design/per-call-region/README.md) for the
/// carrier set, escaping-value retention, ancestor chain, and TCO
/// frame reuse; [memory-model.md § Arena lifetime erasure](../../../design/memory-model.md#region-lifetime-erasure)
/// for the heap-pinning / drop-order invariants.
pub struct CallFrame {
    storage: Rc<FrameStorage>,
    /// The per-call child scope on the substrate's externally-witnessed [`SealedExtern`] carrier
    /// (a `&'static Scope`); read back through [`Self::with_scope`] / [`Self::scope_sealed`]
    /// ([`SealedExtern::open`]) against `storage` as the pin.
    scope_carrier: Option<SealedExtern<ScopeRefFamily>>,
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
        // The region is born inside its own `Rc<FrameStorage>`, heap-pinned from this point on, so
        // the erased child-scope pointer below stays valid as the storage Rc moves into the shell.
        let storage = Rc::new(FrameStorage {
            region: KoanRegion::new(),
            outer: outer_frame,
        });
        // The child scope is born externally-witnessed through the construction door: it brands the
        // fresh region and the longer-lived lexical parent at one `for<'b>`, builds the real invariant
        // `Scope<'b>` coupling them, allocs it through the brand, and erases it straight into a
        // `SealedExtern` — no transient `&'a` minted, no re-anchor outside the substrate. The local
        // borrow of `storage` ends here (the carrier holds a `&'static` reference, not a borrow of
        // `storage`), so `storage` moves into the shell below; the `KoanRegion` stays at a fixed heap
        // address behind the Rc, keeping the erased reference valid.
        let scope_carrier = build_frame_child_witnessed(outer, &storage);
        Rc::new(CallFrame {
            storage,
            scope_carrier: Some(scope_carrier),
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
        Rc::new(CallFrame {
            storage: run_storage,
            scope_carrier: Some(SealedExtern::erase(scope)),
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

    /// The child scope's externally-witnessed [`SealedExtern`] carrier, which is `Some` for the whole
    /// life of a constructed frame (`None` only transiently inside `new` / `try_reset_for_tail`
    /// before the child scope is allocated).
    fn scope_carrier_set(&self) -> &SealedExtern<ScopeRefFamily> {
        self.scope_carrier
            .as_ref()
            .expect("scope_carrier is set after construction")
    }

    /// The child scope's externally-witnessed carrier by value (`SealedExtern<ScopeRefFamily>` is
    /// `Copy`) — the run-loop step's source for a `Yoked` slot, opened at the step brand alongside the
    /// continuation / contract / deps instead of re-anchored through the borrow-bounded `attach`.
    pub(crate) fn scope_sealed(&self) -> SealedExtern<ScopeRefFamily> {
        *self.scope_carrier_set()
    }

    /// Run `f` with this frame's child scope opened at a `for<'b>` brand — the sole scope read, folded
    /// onto `open` like the decide channel. Both the frame-side reads (scope id, the arg reach-set
    /// fold) and the seed-side binds (the MATCH / TRY arm `it`-bind, the user-fn param-bind, the
    /// deferred-return-type elaboration) take this read: a seed relocates its caller-`'a` value into
    /// the opened scope's own region through the substrate (a witnessed shortening) before binding it,
    /// so nothing fabricates a free `&'a`. The carrier opens against this frame's own storage `Rc`
    /// (the pin), and the rank-2 brand keeps the `&Scope<'b>` from escaping the call, so no scope
    /// borrow rides up a `&mut self` path. Carries **no `unsafe`** — `SealedExtern::open` routes the
    /// substrate's single audited reattach.
    pub fn with_scope<R>(&self, f: impl for<'b> FnOnce(&'b Scope<'b>) -> R) -> R {
        self.scope_sealed().open(&self.storage, f)
    }

    /// This frame's child scope id, copied out through [`Self::with_scope`] — the scalar read for the
    /// sites that need only the id, with no `&Scope` escaping the open.
    pub fn scope_id(&self) -> ScopeId {
        self.with_scope(|s| s.id)
    }

    pub fn region(&self) -> &KoanRegion {
        &self.storage.region
    }

    /// This frame's region [`RegionBrand`] allocation capability, minted from its owning storage.
    /// Test-only: production allocates through the scope (`scope.brand()`); the frame-level handle is
    /// a convenience for the arena / lift Miri tests that alloc against a bare frame.
    #[cfg(test)]
    pub(crate) fn brand(&self) -> RegionBrand<'_> {
        self.storage.brand()
    }

    /// Clone this frame's `FrameStorage` Rc — the handle an escaping value (a returned closure, a
    /// module frame) pins to keep its captured environment alive *without* pinning the shell, so
    /// tail reuse stays free to reset the shell.
    pub fn storage_rc(&self) -> Rc<FrameStorage> {
        Rc::clone(&self.storage)
    }

    /// Reset this frame for a tail-call iteration: install a fresh `FrameStorage` (a new
    /// `KoanRegion` escaping into `new_outer.region`, no `outer` link) and re-allocate the child
    /// `Scope` under `new_outer`. The old `FrameStorage` is dropped here — and its region with it —
    /// *unless* an escaped value still holds it, in which case that snapshot lives on independently
    /// while the shell reuses. Returns `false` (untouched) only when `Rc::get_mut` fails — another
    /// live `Rc<CallFrame>` (a shell clone, never an escape) foreclosing in-place reuse. See
    /// [per-call-region/frames.md § TCO frame reuse](../../../design/per-call-region/frames.md#tco-frame-reuse).
    pub fn try_reset_for_tail<'p>(self: &mut Rc<Self>, new_outer: &'p Scope<'p>) -> bool {
        if Rc::get_mut(self).is_none() {
            return false;
        }
        // Build the fresh storage and its child scope before touching the shell, so the region is
        // heap-pinned by the new storage Rc when it lands in the shell.
        let storage = Rc::new(FrameStorage {
            region: KoanRegion::new(),
            outer: None,
        });
        // Born externally-witnessed through the construction door: it brands the fresh region and
        // `new_outer` at one `for<'b>`, builds the invariant `Scope<'b>` coupling them, and erases the
        // freshly-stored child scope into a `SealedExtern` with no transient `&'a` minted.
        let scope_carrier = build_frame_child_witnessed(new_outer, &storage);
        // The local borrow of `storage` ends above, so it can move into the shell.
        let this = Rc::get_mut(self).expect("just-verified unique above");
        // Drops the old storage (and its region) unless an escapee still holds it.
        this.storage = storage;
        this.scope_carrier = Some(scope_carrier);
        true
    }
}

#[cfg(test)]
mod tests;
