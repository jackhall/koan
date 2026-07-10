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

use crate::machine::{CarrierWitness, DeliveredCarried, KError, KErrorKind};
use std::cell::Cell;
use std::rc::Rc;

use super::bindings::StoredReach;
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
    Delivered, Erased, FamilyArena, Reattachable, Region, RegionHandle, RegionHandleFamily,
    RegionHost, RegionSet, Sealed, StepContext, StorageOf, StorageProfile, Stored, WitnessRegion,
    Witnessed,
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
pub struct RegionBrand<'a>(pub(crate) RegionHandle<'a, KoanStorageProfile>);

impl<'a> RegionBrand<'a> {
    /// The bare region this brand authorizes — for identity compares (`ptr::eq`, `pins_region`). A
    /// bare `&KoanRegion` cannot be turned *back* into a brand — the library's [`RegionHandle`] enforces
    /// that — so handing out the identity reference opens no hole.
    pub fn region(self) -> &'a KoanRegion {
        self.0.region()
    }

    /// The bare library allocation capability this brand wraps — the handle-headed construction
    /// operand families (`RegionTypeFamily`, the aggregate accumulators, `execute::run_loop`'s
    /// `DestHandleFamily`) cross the brand as this raw handle rather than the koan veneer, so the
    /// library's own `HasRegionHandle` impls for `RegionHandle`/`(RegionHandle, T)` discharge their
    /// obligation with no koan-side impl. A closure that needs the koan-typed `alloc_*` veneer back
    /// rewraps locally: `RegionBrand(handle)`.
    pub(crate) fn handle(self) -> RegionHandle<'a, KoanStorageProfile> {
        self.0
    }

    /// Store an owned, region-pure [`KObject`] into the region (no value holds an owning `Rc` back
    /// to a region, so the store forms no back-edge). Yields a co-located `&'a` resident. A value
    /// that borrows another region takes [`Self::alloc_object_witnessed_checked`] instead.
    pub fn alloc_object(self, o: KObject<'static>) -> &'a KObject<'a> {
        self.0.alloc_resident::<KObject<'static>>(o)
    }

    /// Store an owned, region-pure [`KType`] into the region. A `t` that borrows another region
    /// (a module-family pointer, or an `Rc`-shared set — see [`KType::to_static`]) cannot satisfy
    /// this bound; it takes [`Self::alloc_ktype_checked`] instead.
    pub fn alloc_ktype(self, t: KType<'static>) -> &'a KType<'a> {
        self.0.alloc_resident::<KType<'static>>(t)
    }

    /// Runtime-checked twin of [`Self::alloc_ktype`] for a `t` that cannot rebuild at `'static`
    /// (a module-family region pointer, or an `Rc`-shared set — see [`KType::to_static`]):
    /// [`KType::resident_in`] audits every region borrow `t` carries against this brand's own
    /// region before anything is stored, so a foreign-region dangle errors loudly instead of
    /// landing unvetted. Storing nothing on a failed audit.
    pub fn alloc_ktype_checked(self, t: KType<'_>) -> Result<&'a KType<'a>, KError> {
        let name = t.name();
        self.0
            .alloc_resident_audited::<KType<'static>>(t, |region, value| value.resident_in(region))
            .ok_or_else(|| {
                KError::new(KErrorKind::ShapeError(format!(
                    "{name}: borrows a region other than its seal's destination"
                )))
            })
    }

    /// Composite entry for a `t` this call site doesn't already know the tier of: the
    /// compile-enforced `'static` tier when [`KType::to_static`] succeeds, else
    /// [`Self::alloc_ktype_checked`]. The brand-level twin of
    /// [`KoanStepContextExt::alloc_type_pure`].
    pub fn alloc_ktype_pure(self, t: KType<'_>) -> Result<&'a KType<'a>, KError> {
        match t.to_static() {
            Some(owned) => Ok(self.alloc_ktype(owned)),
            None => self.alloc_ktype_checked(t),
        }
    }

    /// Runtime-checked twin of [`Self::alloc_object`] for an `o` that cannot rebuild owned at
    /// `'static` (`KObject` has no general `'static` rebuild — see [`KType::to_static`]'s doc):
    /// [`KObject::resident_in`] audits every answerable region borrow `o` carries against this
    /// brand's own region. Honest-partial — see [`KObject::resident_in`]'s doc for the walk's one
    /// blind spot (`Wrapped { type_id }`, un-answerable because `KType` opts out of the residence
    /// side-table).
    pub fn alloc_object_checked(self, o: KObject<'_>) -> Result<&'a KObject<'a>, KError> {
        let name = o.ktype().name();
        self.0
            .alloc_resident_audited::<KObject<'static>>(o, |region, value| {
                value.resident_in(region)
            })
            .ok_or_else(|| {
                KError::new(KErrorKind::ShapeError(format!(
                    "{name}: borrows a region other than its seal's destination"
                )))
            })
    }

    /// INVARIANT: a `KFunction` must be allocated into the same `KoanRegion` that owns its
    /// captured scope — otherwise a `KFunction` could reference a region other than the one
    /// that allocated it, undermining region-based reasoning about `&KFunction` liveness. Every
    /// `KFunction` constructor captures a borrow (its defining scope), so it can never be
    /// `'static`; the `ptr::eq` audit is release-enforced (not `debug_assert!`) — today's UB on
    /// a mis-homed value becomes a loud panic instead.
    pub fn alloc_function(self, f: KFunction<'_>) -> &'a KFunction<'a> {
        self.0
            .alloc_resident_audited::<KFunction<'static>>(f, |region, value| {
                std::ptr::eq(region, value.captured_scope().region())
            })
            .expect(
                "alloc_function: a KFunction must be allocated into the same KoanRegion \
                 that owns its captured scope",
            )
    }

    /// INVARIANT: a `Scope` must be allocated into the region it names as its own — every `Scope`
    /// constructor returns a value borrowing its parent, so it can never be `'static`. See
    /// [`Self::alloc_function`].
    pub fn alloc_scope(self, s: Scope<'_>) -> &'a Scope<'a> {
        self.0
            .alloc_resident_audited::<Scope<'static>>(s, |region, value| {
                std::ptr::eq(region, value.region())
            })
            .expect("alloc_scope: a Scope must be allocated into its own region")
    }

    /// INVARIANT: a `Module` must be allocated into its own child scope's region — every `Module`
    /// borrows the child scope `MODULE` opened for its body, so it can never be `'static`. The one
    /// legitimate cross-region caller (transparent-ascribe's re-tagged `Module`) takes
    /// [`Scope::alloc_module_reaching`] instead. See [`Self::alloc_function`].
    pub fn alloc_module(self, m: Module<'_>) -> &'a Module<'a> {
        self.0
            .alloc_resident_audited::<Module<'static>>(m, |region, value| {
                std::ptr::eq(region, value.child_scope().region())
            })
            .expect("alloc_module: a Module must be allocated into its own child scope's region")
    }

    /// INVARIANT: a `ModuleSignature` must be allocated into its own decl scope's region — every
    /// `ModuleSignature` borrows the decl scope `SIG` opened for its body, so it can never be
    /// `'static`. See [`Self::alloc_function`].
    pub fn alloc_signature(self, s: ModuleSignature<'_>) -> &'a ModuleSignature<'a> {
        self.0
            .alloc_resident_audited::<ModuleSignature<'static>>(s, |region, value| {
                std::ptr::eq(region, value.decl_scope().region())
            })
            .expect(
                "alloc_signature: a ModuleSignature must be allocated into its own decl \
                 scope's region",
            )
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

    /// The witnessed-allocation surface for an owned object built fresh inside the brand: born
    /// witnessed by the **empty** (foreign-reach-only) set. The brand-confined
    /// [`alloc`](Region::alloc) stores `value` and hands the freshly-stored `&'b KObject<'b>` to the
    /// closure at the brand, which bundles it through [`Witnessed::resident`] — the empty-witness
    /// constructor that names the region-pure obligation, so the active frame is deliberately excluded.
    /// The producing frame is folded in only at finalize/close (the scope-reach seal), so a
    /// region-resident value never strong-owns its own frame (the `region → object → frame` cycle that
    /// would keep the frame's `Rc` alive forever and defeat the refcount-driven region free).
    ///
    /// Soundness is the within-step transient invariant: the empty-witness carrier pins nothing,
    /// sound only because the active frame pins the region externally for the construction step and
    /// `finalize` folds the producer **before** the carrier is stored on a node. `value`'s `'static`
    /// bound is region-purity, compile-enforced: a value that references another region cannot
    /// satisfy it — it takes the `yoke` / `merge` path, or
    /// [`Self::alloc_object_witnessed_checked`] for a value whose region borrow is only
    /// runtime-auditable (e.g. raw AST that is splice-free).
    pub(crate) fn alloc_object_witnessed(
        self,
        value: KObject<'static>,
    ) -> Witnessed<CarriedFamily, CarrierWitness> {
        self.0
            .alloc::<KObject<'static>, _>(value, |live| Witnessed::resident(Carried::Object(live)))
    }

    /// Runtime-checked twin of [`Self::alloc_object_witnessed`] for a `value` that cannot rebuild at
    /// `'static` (e.g. a `KObject::KExpression` — `KExpression<'a>` is invariant and raw AST has no
    /// `'static` rebuild): `audit` receives this brand's own region and the value before anything is
    /// stored, and the value is stored — sealed under the same empty (own-region-only) witness
    /// [`Self::alloc_object_witnessed`] uses — only if `audit` returns true. Storing nothing on a
    /// failed audit; a foreign-region dangle errors loudly instead of landing unvetted.
    pub(crate) fn alloc_object_witnessed_checked(
        self,
        value: KObject<'_>,
        audit: impl FnOnce(&KoanRegion, &KObject<'_>) -> bool,
    ) -> Result<Witnessed<CarriedFamily, CarrierWitness>, KError> {
        let name = value.ktype().name();
        self.0
            .alloc_resident_audited::<KObject<'static>>(value, audit)
            .map(|live| Witnessed::resident(Carried::Object(live)))
            .ok_or_else(|| {
                KError::new(KErrorKind::ShapeError(format!(
                    "{name}: borrows a region other than its seal's destination"
                )))
            })
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

/// The evidence-tier move-ins live on [`Scope`], not [`RegionBrand`]: a [`StoredReach`] is
/// meaningful only relative to the scope that minted it — the mint materializes no member for a
/// region [`Scope::covers_region_ambiently`] already covers — so the audit that consumes one must
/// run against that same scope's region and ambient coverage. Taking the destination from `self`
/// makes it the minting scope's own region by construction; there is no scope parameter for a
/// caller to mismatch. (The block lives here, beside the other move-in tiers and [`Residence`],
/// rather than in `scope.rs`.)
impl<'a> Scope<'a> {
    /// The evidence tier for a `t` whose region borrows may reach a *foreign* region this scope
    /// has already minted reach evidence for (a bind-time `register_type`, a read-site's
    /// materialized `StoredReach`), not just its own region. Widens
    /// [`RegionBrand::alloc_ktype_checked`]'s dest-only audit to "this scope's region,
    /// `evidence`'s reach members, or a region [`Self::covers_region_ambiently`] covers" — the
    /// last disjunct is the exact complement of the mint's omission policy, which materializes no
    /// member for an ambiently covered region, so a dest/evidence-only audit would under-cover a
    /// value legitimately reaching one (a module bound at an outer/root scope, read by a nested
    /// per-call functor body). Exact for `KType`, since its only region pointers (`&Module` /
    /// `&ModuleSignature` / `&KFunction`) are all side-table-recorded.
    pub(crate) fn alloc_ktype_reaching(
        &self,
        t: KType<'_>,
        evidence: &StoredReach<'_>,
    ) -> Result<&'a KType<'a>, KError> {
        let name = t.name();
        let sets: &[&FrameSet] = match &evidence.foreign {
            Some(fs) => std::slice::from_ref(fs),
            None => &[],
        };
        self.brand()
            .0
            .alloc_resident_audited::<KType<'static>>(t, |region, value| {
                // The plain evidence-only check first (cheap, no closure alloc, and directly
                // unit-testable in isolation); only fall back to the ambient-widened walk when it
                // declines.
                value.resident_in_reach(region, evidence) || {
                    let ambient = |r: &KoanRegion| self.covers_region_ambiently(r);
                    let residence = Residence::with_reach_and_ambient(region, sets, &ambient);
                    value.resident_in_visiting(&residence, &mut Vec::new())
                }
            })
            .ok_or_else(|| {
                KError::new(KErrorKind::ShapeError(format!(
                    "{name}: borrows a region other than its seal's destination, evidence reach, \
                     or the destination scope's ambient coverage"
                )))
            })
    }

    /// The object evidence tier: for an `o` built from (or embedding a projection of) values
    /// whose reach this scope has already minted as `evidence` — a delivered arg carrier's
    /// `adopted_reach_of`/`host_reach_of`, or several for a multi-carrier fold (an args record).
    /// Widens the coverage predicate over every evidence member's hosting arena, same partiality
    /// as [`RegionBrand::alloc_object_checked`] — plus a region [`Self::covers_region_ambiently`]
    /// covers (see [`Self::alloc_ktype_reaching`]'s doc for why the evidence alone under-covers
    /// that case). Returns a structured `KError` on rejection — the item's decided non-panicking
    /// conversion-failure policy — so a bug in the caller's evidence computation surfaces as a
    /// catchable error rather than crashing the interpreter; a caller with no `KError` channel in
    /// hand (e.g. a seed closure with no `Result` return) calls `.expect(...)` naming the site
    /// invariant instead.
    pub(crate) fn alloc_object_delivered(
        &self,
        o: KObject<'_>,
        evidence: &[StoredReach<'_>],
    ) -> Result<&'a KObject<'a>, KError> {
        let name = o.ktype().name();
        let sets: Vec<&FrameSet> = evidence.iter().filter_map(|r| r.foreign).collect();
        self.brand()
            .0
            .alloc_resident_audited::<KObject<'static>>(o, |region, value| {
                // The plain evidence-only check first (cheap, directly unit-testable); only fall
                // back to the ambient-widened walk when it declines.
                value.resident_in_delivered(region, evidence) || {
                    let ambient = |r: &KoanRegion| self.covers_region_ambiently(r);
                    let residence = Residence::with_reach_and_ambient(region, &sets, &ambient);
                    value.resident_in_visiting(&residence)
                }
            })
            .ok_or_else(|| {
                KError::new(KErrorKind::ShapeError(format!(
                    "{name}: borrows a region not covered by dest, the supplied evidence, or \
                     the destination scope's ambient coverage"
                )))
            })
    }

    /// Placement for a `Module` whose child scope legitimately lives in a region other than this
    /// scope's own — transparent-ascribe's re-tagged `Module`, which reuses the foreign source
    /// module's child scope. `evidence` is the `StoredReach` the caller minted for that child
    /// scope's region *before* this call ([`Scope::reach_of_child`]), so the audit widens
    /// [`RegionBrand::alloc_module`]'s dest-only check to "this scope's region, `evidence`'s
    /// reach, or a region [`Self::covers_region_ambiently`] covers" (see
    /// [`Self::alloc_ktype_reaching`]'s doc for why the last disjunct is needed).
    pub(crate) fn alloc_module_reaching(
        &self,
        m: Module<'_>,
        evidence: &StoredReach<'_>,
    ) -> &'a Module<'a> {
        let sets: &[&FrameSet] = match &evidence.foreign {
            Some(fs) => std::slice::from_ref(fs),
            None => &[],
        };
        let ambient = |region: &KoanRegion| self.covers_region_ambiently(region);
        self.brand()
            .0
            .alloc_resident_audited::<Module<'static>>(m, |region, value| {
                Residence::with_reach_and_ambient(region, sets, &ambient)
                    .covers_region(value.child_scope().region())
            })
            .expect(
                "alloc_module_reaching: a Module's child scope must be covered by dest, the \
                 supplied evidence reach, or the destination scope's ambient coverage",
            )
    }
}

/// The allocation capability inside a reach-folding closure: the enclosing combinator
/// (`transfer_into` / `merge_pinned` / `map_pinned` / [`KoanStepContextExt::alloc_carried_with`])
/// composes a witness naming every source operand's reach, so a value built *from the closure's
/// operands* is covered by the fold without a per-value audit. Carries the folded-placement
/// methods [`RegionBrand`] deliberately lacks; everything else derefs. Minted only two ways (see
/// [`Self::in_fold_closure`] and [`KoanStepContextExt::alloc_carried_with`]'s impl) — `grep
/// FoldingBrand` is the complete list of trust points a value can reach this capability through.
#[derive(Clone, Copy)]
pub(crate) struct FoldingBrand<'a>(RegionBrand<'a>);

impl<'a> std::ops::Deref for FoldingBrand<'a> {
    type Target = RegionBrand<'a>;
    fn deref(&self) -> &RegionBrand<'a> {
        &self.0
    }
}

impl<'a> FoldingBrand<'a> {
    /// Named trust point for a closure under a *library* fold combinator (`transfer_into` /
    /// `merge_pinned` / `map_pinned`), which hands raw handle-headed operands by design and so
    /// cannot itself mint a `FoldingBrand`. `grep in_fold_closure(` is the complete audit list of
    /// every such trust point; the caller's obligation is that the value it builds through the
    /// returned brand is built only from that closure's own operands, whose reach the enclosing
    /// combinator already folds into the result.
    pub(crate) fn in_fold_closure(handle: RegionHandle<'a, KoanStorageProfile>) -> Self {
        FoldingBrand(RegionBrand(handle))
    }

    /// Store a `t` built from the fold's own operands — the always-true audit is sound only
    /// because the capability itself is confined to a fold closure (see the type doc).
    pub(crate) fn alloc_ktype_folded(self, t: KType<'_>) -> &'a KType<'a> {
        (self.0)
            .0
            .alloc_resident_audited::<KType<'static>>(t, |_, _| true)
            .expect("alloc_resident_audited with an always-true audit never returns None")
    }

    /// Object twin of [`Self::alloc_ktype_folded`].
    pub(crate) fn alloc_object_folded(self, o: KObject<'_>) -> &'a KObject<'a> {
        (self.0)
            .0
            .alloc_resident_audited::<KObject<'static>>(o, |_, _| true)
            .expect("alloc_resident_audited with an always-true audit never returns None")
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

/// A witnessed-construction operand bundling a destination region's [`RegionHandle`] with a
/// type-channel identity (a `SetRef` / declared type) that must cross the build brand. A
/// value-embedding construction `transfer_into`/`merge`s its object carrier with this operand so the
/// wrapped value lands — allocated through the handle — tagged by the identity, both re-anchored to
/// the build brand under the same witness; the dest frame's `outer` chain pins the identity's
/// (ancestor) region. Used by the newtype / tagged-union constructors and the `CATCH` `Result`
/// build. Layout-invariant: two thin pointers, representation independent of `'r`.
pub struct RegionTypeFamily;
reattachable!(RegionTypeFamily => (RegionHandle<'r, KoanStorageProfile>, &'r KType<'r>));

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
    fn record_local(frame: &KoanRegion, stored: &KFunction<'static>) {
        frame.record_addr(stored as *const _ as usize);
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
    fn record_local(frame: &KoanRegion, stored: &Module<'static>) {
        frame.record_addr(stored as *const _ as usize);
    }
}

impl Stored<KoanStorageProfile> for ModuleSignature<'static> {
    fn cell(s: &StorageOf<KoanStorageProfile>) -> &FamilyArena<Self> {
        &s.1 .1 .1 .1 .0
    }
    fn record_local(frame: &KoanRegion, stored: &ModuleSignature<'static>) {
        frame.record_addr(stored as *const _ as usize);
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

    /// Whether `ptr` was returned by a prior `alloc_module` on this region — the
    /// [`KType::resident_in`](crate::machine::model::types::KType::resident_in) audit's check for
    /// a `KType::Module` / `AbstractType { source: AbstractSource::Module(_), .. }` payload.
    fn owns_module<'a>(&self, ptr: *const Module<'a>) -> bool;

    /// Whether `ptr` was returned by a prior `alloc_signature` on this region — the residence
    /// audit's check for a `KType::Signature` payload.
    fn owns_signature<'a>(&self, ptr: *const ModuleSignature<'a>) -> bool;

    /// Whether `ptr` was returned by a prior `alloc_function` on this region — the residence
    /// audit's check for a `KType::KFunctor { body: Some(_), .. }` payload.
    fn owns_function<'a>(&self, ptr: *const KFunction<'a>) -> bool;
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

    fn owns_module<'a>(&self, ptr: *const Module<'a>) -> bool {
        // `Module` is invariant in `'a`, so the through-`'static` cast is required despite
        // clippy's complaint.
        #[allow(clippy::unnecessary_cast)]
        let target = ptr as *const Module<'static> as usize;
        self.owns_addr(target)
    }

    fn owns_signature<'a>(&self, ptr: *const ModuleSignature<'a>) -> bool {
        // `ModuleSignature` is invariant in `'a`, so the through-`'static` cast is required
        // despite clippy's complaint.
        #[allow(clippy::unnecessary_cast)]
        let target = ptr as *const ModuleSignature<'static> as usize;
        self.owns_addr(target)
    }

    fn owns_function<'a>(&self, ptr: *const KFunction<'a>) -> bool {
        // `KFunction` is invariant in `'a`, so the through-`'static` cast is required despite
        // clippy's complaint.
        #[allow(clippy::unnecessary_cast)]
        let target = ptr as *const KFunction<'static> as usize;
        self.owns_addr(target)
    }
}

/// Ownership predicate for the checked/reaching-tier residence audits: "`dest`, or the hosting
/// arena of some member of `reach`, or a region `ambient` reports as already covered" —
/// [`KType::resident_in`](crate::machine::model::types::KType::resident_in) /
/// [`KObject::resident_in`](KObject::resident_in)'s dest-only check is the `reach: &[]`,
/// `ambient: None` case; [`KType::resident_in_reach`](crate::machine::model::types::KType::resident_in_reach)
/// and the object delivered tier widen it. Each `reach` set was minted into `dest`'s own arena by
/// the same scope the audit runs against (`Scope::host_reach_of` / `adopted_reach_of`), so
/// membership here is dest-relative by construction — no separate "is this evidence dest-relative"
/// check is needed. `ambient`, when supplied, is the destination scope's own
/// [`Scope::covers_region_ambiently`](super::scope::Scope::covers_region_ambiently) — the exact
/// predicate every `host_reach_of` / `adopted_reach_of` mint omits by, so a region the mint left
/// out of `reach` is still resident — omitted from the *reach set*, never from *residence*. Only
/// [`Scope`]'s own evidence-tier methods construct the `ambient` form, binding the predicate to
/// the destination scope by construction.
pub(crate) struct Residence<'d> {
    dest: &'d KoanRegion,
    reach: &'d [&'d FrameSet],
    ambient: Option<&'d dyn Fn(&KoanRegion) -> bool>,
}

impl<'d> Residence<'d> {
    pub(crate) fn dest_only(dest: &'d KoanRegion) -> Self {
        Residence {
            dest,
            reach: &[],
            ambient: None,
        }
    }

    pub(crate) fn with_reach(dest: &'d KoanRegion, reach: &'d [&'d FrameSet]) -> Self {
        Residence {
            dest,
            reach,
            ambient: None,
        }
    }

    /// [`Self::with_reach`] plus the destination scope's own ambient coverage
    /// ([`Scope::covers_region_ambiently`]) — see the type doc's `ambient` paragraph.
    pub(crate) fn with_reach_and_ambient(
        dest: &'d KoanRegion,
        reach: &'d [&'d FrameSet],
        ambient: &'d dyn Fn(&KoanRegion) -> bool,
    ) -> Self {
        Residence {
            dest,
            reach,
            ambient: Some(ambient),
        }
    }

    /// Whether `region` is `dest` itself, is covered by some `reach` member's own pin chain, or is
    /// reported covered by `ambient` — [`Scope::alloc_module_reaching`]'s coverage check.
    /// [`RegionSet::pins_region`] is the library's public reach-coverage query (unlike
    /// [`RegionSet::members`], which is gated to `test`/`test-hooks` — koan cannot enumerate a
    /// set's members in production, only ask it whether a given region is covered).
    pub(crate) fn covers_region(&self, region: &KoanRegion) -> bool {
        std::ptr::eq(self.dest, region)
            || self.reach.iter().any(|fs| fs.pins_region(region))
            || self.ambient.is_some_and(|f| f(region))
    }

    /// Whether `module`'s own storage is `dest`-resident (the address side-table check) or its
    /// child scope's region is covered by `reach` — [`Self::covers_region`] over the module's own
    /// region accessor, since a raw payload pointer's *owning* region cannot be recovered from
    /// `reach` without enumerating members.
    pub(crate) fn owns_module(&self, module: &Module<'_>) -> bool {
        self.dest.owns_module(module as *const Module<'_>)
            || self.covers_region(module.child_scope().region())
    }

    pub(crate) fn owns_signature(&self, sig: &ModuleSignature<'_>) -> bool {
        self.dest.owns_signature(sig as *const ModuleSignature<'_>)
            || self.covers_region(sig.decl_scope().region())
    }

    pub(crate) fn owns_function(&self, f: &KFunction<'_>) -> bool {
        self.dest.owns_function(f as *const KFunction<'_>)
            || self.covers_region(f.captured_scope().region())
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

    /// [`StepContext::alloc_with`] with the closure receiving a [`FoldingBrand`] and the deps'
    /// views: the built carrier names every listed dep's reach **and residence host** (each dep
    /// arrives as its delivery envelope and folds at `Residence::Kept`), by construction — so a
    /// value the closure builds from those deps' operands is covered by the fold, and
    /// [`FoldingBrand`]'s folded-placement methods store it without a per-value audit. Plain
    /// [`RegionBrand`] methods stay reachable through `Deref`, so a closure building an unrelated
    /// `'static` value is unaffected.
    fn alloc_carried_with(
        &self,
        deps: &[&DeliveredCarried],
        build: impl for<'b> FnOnce(FoldingBrand<'b>, Vec<Carried<'b>>) -> Carried<'b>,
    ) -> Witnessed<CarriedFamily, CarrierWitness>;

    /// [`Self::alloc_carried`] specialized to the one-`KType`-carrier shape: reach = own region
    /// only. `kt`'s `'static` bound is region-purity, compile-enforced — the common case for a
    /// bind-time or synchronously-resolved type. A `kt` that borrows another region takes
    /// [`Self::alloc_type_checked`] instead.
    fn alloc_type(&self, kt: KType<'static>) -> Witnessed<CarriedFamily, CarrierWitness>;

    /// The step twin of [`RegionBrand::alloc_ktype_checked`]: runtime-audits `kt`'s region
    /// borrows against this frame's own region and seals the result under the empty (own-region
    /// only) reach — the same [`Carried::Type`] wrap [`Self::alloc_type`] uses — erroring instead
    /// of storing an unvetted foreign-region dangle. For a `kt` [`KType::to_static`] declines (a
    /// module-family pointer or an `Rc`-shared set).
    fn alloc_type_checked(
        &self,
        kt: KType<'_>,
    ) -> Result<Witnessed<CarriedFamily, CarrierWitness>, KError>;

    /// Composite entry a caller reaches for whenever it doesn't already know which tier `kt`
    /// needs: the compile-enforced `'static` tier when [`KType::to_static`] succeeds, else the
    /// runtime-checked tier. Always correct — the two tiers agree on every value `to_static`
    /// accepts (`to_static().is_some()` implies [`KType::resident_in`] for any destination).
    fn alloc_type_pure(
        &self,
        kt: KType<'_>,
    ) -> Result<Witnessed<CarriedFamily, CarrierWitness>, KError> {
        match kt.to_static() {
            Some(owned) => Ok(self.alloc_type(owned)),
            None => self.alloc_type_checked(kt),
        }
    }

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
        build: impl for<'b> FnOnce(FoldingBrand<'b>, Vec<Carried<'b>>) -> Carried<'b>,
    ) -> Witnessed<CarriedFamily, CarrierWitness> {
        self.alloc_with_handle::<KoanStorageProfile, CarriedFamily, CarriedFamily>(
            deps,
            |handle, views| build(FoldingBrand(RegionBrand(handle)), views),
        )
    }

    fn alloc_type(&self, kt: KType<'static>) -> Witnessed<CarriedFamily, CarrierWitness> {
        self.alloc_carried(|b| Carried::Type(b.alloc_ktype(kt)))
    }

    fn alloc_type_checked(
        &self,
        kt: KType<'_>,
    ) -> Result<Witnessed<CarriedFamily, CarrierWitness>, KError> {
        // Unlike `alloc_carried`'s `for<'b>` brand construction, the checked veneer doesn't need
        // to build `kt` from brand-derived references — `kt` already exists and is audited by
        // address, so the resident reference it hands back is erased straight into the empty
        // (own-region-only) witness via `Witnessed::resident`, mirroring
        // `RegionBrand::alloc_object_witnessed`'s erase-on-store without the brand-closure
        // indirection `alloc_carried` needs for a from-scratch construction.
        let frame = self.frame();
        let stored = frame.brand().alloc_ktype_checked(kt)?;
        Ok(Witnessed::resident(Carried::Type(stored)))
    }

    fn alloc_type_with(
        &self,
        deps: &[&DeliveredCarried],
        kt: KType<'_>,
    ) -> Witnessed<CarriedFamily, CarrierWitness> {
        // Scalar gate: a region-free scalar type references none of `deps`, so folding their reach in
        // would only over-retain. Route it to the no-fold path so it seals with an empty reach.
        // `is_region_free_scalar` is exactly `to_static`'s owned-leaf class, so the rebuild always
        // succeeds.
        if kt.is_region_free_scalar() {
            return self.alloc_type(
                kt.to_static()
                    .expect("is_region_free_scalar implies to_static() is Some"),
            );
        }
        self.alloc_carried_with(deps, |b, _views| Carried::Type(b.alloc_ktype_folded(kt)))
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
        // `is_shallow_scalar`'s four variants hold only owned payloads, so rebuilding fresh (rather
        // than coercing the `'_`-tagged `value`) is always valid at `'static` — `KObject` has no
        // general `to_static` (unlike `KType`; see `KType::to_static`'s doc), so this is a
        // by-hand rebuild scoped to exactly the owned variants `is_shallow_scalar` names.
        if value.is_shallow_scalar() {
            let value = match value {
                KObject::Number(n) => KObject::Number(n),
                KObject::KString(s) => KObject::KString(s),
                KObject::Bool(b) => KObject::Bool(b),
                KObject::Null => KObject::Null,
                _ => unreachable!("is_shallow_scalar guarantees one of the four owned variants"),
            };
            return self.alloc_carried(|b| Carried::Object(b.alloc_object(value)));
        }
        self.alloc_carried_with(deps, |b, _views| {
            Carried::Object(b.alloc_object_folded(value))
        })
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
        //
        // `child.outer` is a genuine cross-region borrow into the lexical parent's (possibly foreign)
        // region — unlike every other resident move-in in this file, `child` cannot rebuild at
        // `'static` and its liveness is not the reach-witness system's business to name: it is
        // guaranteed instead by `FrameStorage`'s own `outer` `Rc` chain (see this fn's doc), a
        // structural invariant this construction door alone upholds by always chaining `storage`'s
        // `outer` to the same frame that owns `outer_b`'s region. The audit here is therefore
        // unconditional — there is no address to check against `handle_b`'s region, only the
        // caller-side (this function's sole caller, `CallFrame::new`) obligation that `storage`'s
        // `outer_frame` already pins the ancestor. Storage can't fail here.
        let child = Scope::child_for_frame_witnessed(outer_b, RegionBrand(handle_b), region_owner);
        let live = handle_b
            .alloc_resident_audited::<Scope<'static>>(child, |_region, _value| true)
            .expect("alloc_resident_audited with an always-true audit never returns None");
        Sealed::seal(Witnessed::<ScopeRefFamily, CarrierWitness>::resident(live))
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
