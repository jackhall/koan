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
use std::marker::PhantomData;
use std::rc::Rc;

use crate::machine::core::kfunction::action::scope_frame;
use crate::machine::execute::StepCarried;

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
    AuditedStored, Delivered, Erased, FamilyArena, FoldedPlacement, Reattachable, Region,
    RegionHandle, RegionHandleFamily, RegionHost, RegionSet, Sealed, StepContext, StorageOf,
    StorageProfile, Stored, WitnessRegion, Witnessed,
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
    /// (a module-family region pointer, a signature pointer, or an `Rc`-shared set — see
    /// [`KType::to_static`]): [`KType::resident_in`] audits every region borrow `t` carries against
    /// this brand's own region before anything is stored, so a foreign-region dangle errors loudly
    /// instead of landing unvetted. Storing nothing on a failed audit.
    ///
    /// Confined to identity-preserving stores: a caller reaches here to store a value that cannot
    /// rebuild at `'static` (a module-family pointer, a signature pointer, an `Rc`-shared set). A
    /// site assembling a new composite [`KType`] from ambiently-read parts takes a brand door
    /// instead ([`StepAllocator::alloc_type_composed`], [`StepAllocator::alloc_carried_with`], or
    /// the field-list fold), so no from-scratch composite rides the runtime audit.
    pub fn alloc_ktype_checked(self, t: KType<'_>) -> Result<&'a KType<'a>, KError> {
        let name = t.name();
        self.0
            .alloc_resident_checked::<KType<'static>>(t, ResidenceEvidence::dest_only())
            .ok_or_else(|| {
                KError::new(KErrorKind::ShapeError(format!(
                    "{name}: borrows a region other than its seal's destination"
                )))
            })
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
            .alloc_resident_checked::<KObject<'static>>(o, ResidenceEvidence::dest_only())
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
            .alloc_resident_checked::<KFunction<'static>>(f, ())
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
            .alloc_resident_checked::<Scope<'static>>(s, ())
            .expect("alloc_scope: a Scope must be allocated into its own region")
    }

    /// INVARIANT: a `Module` must be allocated into its own child scope's region — every `Module`
    /// borrows the child scope `MODULE` opened for its body, so it can never be `'static`. The one
    /// legitimate cross-region caller (transparent-ascribe's re-tagged `Module`) takes
    /// [`Scope::alloc_module_reaching`] instead. See [`Self::alloc_function`].
    pub fn alloc_module(self, m: Module<'_>) -> &'a Module<'a> {
        self.0
            .alloc_resident_checked::<Module<'static>>(m, ResidenceEvidence::dest_only())
            .expect("alloc_module: a Module must be allocated into its own child scope's region")
    }

    /// INVARIANT: a `ModuleSignature` must be allocated into its own decl scope's region — every
    /// `ModuleSignature` borrows the decl scope `SIG` opened for its body, so it can never be
    /// `'static`. See [`Self::alloc_function`].
    pub fn alloc_signature(self, s: ModuleSignature<'_>) -> &'a ModuleSignature<'a> {
        self.0
            .alloc_resident_checked::<ModuleSignature<'static>>(s, ())
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
    /// [`RegionSet::mint_with_dest_bit`]. `omit` is the scope's home/lexical-ancestor policy
    /// predicate; home-omission (self-cycle) is handled by the library. Returns the minted set
    /// (`None` when the composed reach is empty — a region-pure value pins nothing) paired with the
    /// pre-omission destination-coverage bit (`true` iff a source set or materialized host reaches
    /// this brand's own region before home-omission drops it).
    pub(crate) fn mint(
        self,
        sources: &[&FrameSet],
        materialize_hosts: &[Rc<FrameStorage>],
        omit: impl Fn(&KoanRegion) -> bool,
    ) -> (Option<&'a FrameSet>, bool) {
        RegionSet::mint_with_dest_bit(self.0, sources, materialize_hosts, omit)
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
    /// The within-step transient invariant is typed: the empty-witness carrier pins nothing, so it
    /// returns as a [`StepCarried`] branded at this brand's own `'a` — in production a step's
    /// rank-2 open lifetime — and the borrow checker rejects any use past the step. The active
    /// frame pins the region across the step, and the sole exit to node storage is the seal door in
    /// `step_carried.rs`, where finalize's fold names the producer in the carrier's own reach.
    /// `value`'s `'static` bound is region-purity, compile-enforced: a value that references
    /// another region cannot satisfy it — it takes the `yoke` / `merge` path, or
    /// [`Self::alloc_object_witnessed_checked`] for a value whose region borrow is only
    /// runtime-auditable (e.g. raw AST that is splice-free).
    pub(crate) fn alloc_object_witnessed(self, value: KObject<'static>) -> StepCarried<'a> {
        StepCarried::born(
            self.0.alloc::<KObject<'static>, _>(value, |live| {
                Witnessed::resident(Carried::Object(live))
            }),
        )
    }

    /// Runtime-checked twin of [`Self::alloc_object_witnessed`] for a `value` that cannot rebuild at
    /// `'static` (e.g. a `KObject::KExpression` — `KExpression<'a>` is invariant and raw AST has no
    /// `'static` rebuild): the `KObject` family audit vets `value` against this brand's own region
    /// before anything is stored, and the value is stored — sealed under the same empty
    /// (own-region-only) witness [`Self::alloc_object_witnessed`] uses — only if it passes. The
    /// standard `KObject` residence walk gates a `KObject::KExpression` by its
    /// [`is_splice_free`](crate::machine::model::ast::KExpression::is_splice_free) flag, so a spliced
    /// expression (a resolved value carrying a producer reach the empty seal cannot name) is
    /// rejected. Storing nothing on a failed audit; a foreign-region dangle errors loudly instead of
    /// landing unvetted.
    pub(crate) fn alloc_object_witnessed_checked(
        self,
        value: KObject<'_>,
    ) -> Result<StepCarried<'a>, KError> {
        let name = value.ktype().name();
        self.0
            .alloc_resident_checked::<KObject<'static>>(value, ResidenceEvidence::dest_only())
            .map(|live| StepCarried::born(Witnessed::resident(Carried::Object(live))))
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
        let ambient = |r: &KoanRegion| self.covers_region_ambiently(r);
        self.brand()
            .0
            .alloc_resident_checked::<KType<'static>>(
                t,
                ResidenceEvidence::reaching_ambient(sets, &ambient),
            )
            .ok_or_else(|| {
                KError::new(KErrorKind::ShapeError(format!(
                    "{name}: borrows a region other than its seal's destination, evidence reach, \
                     or the destination scope's ambient coverage"
                )))
            })
    }

    /// The object twin of [`Self::alloc_ktype_reaching`]: for an `o` whose region borrows may reach
    /// a *foreign* region this scope has already minted reach evidence for (a read-site's
    /// materialized `StoredReach`). Widens [`RegionBrand::alloc_object_checked`]'s dest-only audit to
    /// "this scope's region, `evidence`'s reach members, or a region
    /// [`Self::covers_region_ambiently`] covers" — the same coverage predicate, honest-partial in the
    /// one place the `KObject` walk is (`Wrapped { type_id }`). Surfacing a resolved `KType::Module`
    /// hit as the Object-arm module value takes this door: the module child scope's region is named by
    /// the hit's stored reach.
    pub(crate) fn alloc_object_reaching(
        &self,
        o: KObject<'_>,
        evidence: &StoredReach<'_>,
    ) -> Result<&'a KObject<'a>, KError> {
        let name = o.ktype().name();
        let sets: &[&FrameSet] = match &evidence.foreign {
            Some(fs) => std::slice::from_ref(fs),
            None => &[],
        };
        let ambient = |r: &KoanRegion| self.covers_region_ambiently(r);
        self.brand()
            .0
            .alloc_resident_checked::<KObject<'static>>(
                o,
                ResidenceEvidence::reaching_ambient(sets, &ambient),
            )
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
        let ambient = |r: &KoanRegion| self.covers_region_ambiently(r);
        self.brand()
            .0
            .alloc_resident_checked::<KObject<'static>>(
                o,
                ResidenceEvidence::reaching_ambient(&sets, &ambient),
            )
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
    /// scope's region *before* this call ([`Scope::child_module_reach`]), so the audit widens
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
            .alloc_resident_checked::<Module<'static>>(
                m,
                ResidenceEvidence::reaching_ambient(sets, &ambient),
            )
            .expect(
                "alloc_module_reaching: a Module's child scope must be covered by dest, the \
                 supplied evidence reach, or the destination scope's ambient coverage",
            )
    }

    /// Checked move-in of a fresh object into this scope's own region ([`RegionBrand::alloc_object_checked`]'s
    /// dest-only audit), paired with its derived [`StoredReach`]: `foreign` is `None` — a value that
    /// passes the dest-only audit borrows no foreign region — and `borrows_into_home` is the audit
    /// walk's saw-a-region-pointer flag ([`Residence::dest_only_seen`]), so the home-borrow bit is
    /// derived from the value's own borrows, never asserted.
    pub(crate) fn alloc_object_checked_stored(
        &self,
        value: KObject<'_>,
    ) -> Result<(&'a KObject<'a>, StoredReach<'a>), KError> {
        let name = value.ktype().name();
        let seen = Cell::new(false);
        let obj = self
            .brand()
            .0
            .alloc_resident_checked::<KObject<'static>>(
                value,
                ResidenceEvidence::dest_only_seen(&seen),
            )
            .ok_or_else(|| {
                KError::new(KErrorKind::ShapeError(format!(
                    "{name}: borrows a region other than its seal's destination"
                )))
            })?;
        Ok((
            obj,
            StoredReach {
                foreign: None,
                borrows_into_home: seen.get(),
            },
        ))
    }

    /// The [`KType`] twin of [`Self::alloc_object_checked_stored`]: checked move-in of a fresh type
    /// into this scope's own region ([`RegionBrand::alloc_ktype_checked`]'s dest-only audit), paired
    /// with its derived [`StoredReach`] (empty foreign reach; the home-borrow bit is the walk's
    /// saw-a-region-pointer flag).
    pub(crate) fn alloc_ktype_checked_stored(
        &self,
        t: KType<'_>,
    ) -> Result<(&'a KType<'a>, StoredReach<'a>), KError> {
        let name = t.name();
        let seen = Cell::new(false);
        let kt = self
            .brand()
            .0
            .alloc_resident_checked::<KType<'static>>(t, ResidenceEvidence::dest_only_seen(&seen))
            .ok_or_else(|| {
                KError::new(KErrorKind::ShapeError(format!(
                    "{name}: borrows a region other than its seal's destination"
                )))
            })?;
        Ok((
            kt,
            StoredReach {
                foreign: None,
                borrows_into_home: seen.get(),
            },
        ))
    }

    /// Derive a resident type's [`StoredReach`] by auditing the value in place — the read-side twin of
    /// [`Self::alloc_ktype_checked_stored`] for a `&KType` already living in this scope's region. The
    /// audit walk targets this region (a resident value borrows only it), so `foreign` is `None` and
    /// `borrows_into_home` is the walk's saw-a-region-pointer flag. No allocation, no assertion.
    pub(crate) fn checked_reach_of_type(&self, kt: &'a KType<'a>) -> StoredReach<'a> {
        let region = self.brand().region();
        let seen = Cell::new(false);
        kt.resident_in_visiting(&Residence::dest_only_seen(region, &seen), &mut Vec::new());
        StoredReach {
            foreign: None,
            borrows_into_home: seen.get(),
        }
    }

    /// Checked alloc of a fresh object into this scope's region, derive its `(None, bit)` witness,
    /// and seal it as the resident carrier — one call for a value born carrier-less. The home-borrow
    /// bit is the checked audit's own saw-a-region-pointer flag, never a caller assertion.
    pub(crate) fn seal_fresh_object(
        &self,
        value: KObject<'_>,
    ) -> Result<Witnessed<CarriedFamily, CarrierWitness>, KError> {
        let (obj, stored) = self.alloc_object_checked_stored(value)?;
        Ok(self.resident_value_carrier(obj, stored))
    }

    /// The [`KType`] twin of [`Self::seal_fresh_object`].
    pub(crate) fn seal_fresh_ktype(
        &self,
        t: KType<'_>,
    ) -> Result<Witnessed<CarriedFamily, CarrierWitness>, KError> {
        let (kt, stored) = self.alloc_ktype_checked_stored(t)?;
        Ok(self.resident_type_carrier(kt, stored))
    }
}

/// The allocation capability inside a reach-folding closure: the enclosing combinator
/// (`transfer_into` / `merge_pinned` / `map_pinned` / [`StepAllocator::alloc_carried_with`])
/// composes a witness naming every source operand's reach, so a value built *from the closure's
/// operands* is covered by the fold without a per-value audit. Carries the folded-placement
/// methods [`RegionBrand`] deliberately lacks; everything else derefs. A [`FoldedPlacement`] is the
/// sole key to its one constructor ([`Self::in_fold_closure`]): a fold engine mints the placement
/// over the destination region and hands it in, and the placement's `'a` brand keeps it confined to
/// the closure, so this capability is reachable only at a fresh fold brand — enforced by the type,
/// not by a prose audit list.
#[derive(Clone, Copy)]
pub struct FoldingBrand<'a> {
    brand: RegionBrand<'a>,
    placement: FoldedPlacement<'a, KoanStorageProfile>,
}

impl<'a> std::ops::Deref for FoldingBrand<'a> {
    type Target = RegionBrand<'a>;
    fn deref(&self) -> &RegionBrand<'a> {
        &self.brand
    }
}

impl<'a> FoldingBrand<'a> {
    /// Mint the folded-placement capability inside a fold closure. The [`FoldedPlacement`] is the
    /// fold-brand proof: a fold engine mints it over the destination region and hands it to the
    /// closure alongside the operands, and its `'a` brand keeps it confined there — so this
    /// constructor is callable only where the enclosing combinator already folds the operands' reach
    /// into the result.
    pub(crate) fn in_fold_closure(placement: FoldedPlacement<'a, KoanStorageProfile>) -> Self {
        FoldingBrand {
            brand: RegionBrand(placement.handle()),
            placement,
        }
    }

    /// Store a value built at this fold's own brand. Sound without a per-value audit: the input is
    /// typed at the brand lifetime, and inside a `for<'b>` fold closure the only inhabitants of
    /// `KType<'b>` are values derived from the fold's declared operand views, the brand's own
    /// allocations, and owned/`'static` data — all named by the witness the enclosing combinator
    /// composes. An ambient-lifetime capture is a compile error at this signature (a `KType<'ambient>`
    /// cannot coerce to `KType<'b>`, since `'b` has no outlives relation to any enclosing lifetime),
    /// so the store is discharged at compile time by the placement capability, with no runtime audit
    /// at all.
    pub(crate) fn alloc_ktype_folded(self, t: KType<'a>) -> &'a KType<'a> {
        self.placement.alloc_resident_folded::<KType<'static>>(t)
    }

    /// Object twin of [`Self::alloc_ktype_folded`].
    pub(crate) fn alloc_object_folded(self, o: KObject<'a>) -> &'a KObject<'a> {
        self.placement.alloc_resident_folded::<KObject<'static>>(o)
    }
}

/// One positional operand of a brand-composed type build: the total, embedding-ordered form of
/// the sparse `deps` list [`StepAllocator::alloc_type_composed`] partitions. `Reaching` folds the
/// carrier's reach + host into the result's witness and surfaces as a view; `Pure` is a
/// region-free type rebuilt at the brand through the region's own `'static` door, contributing no
/// reach.
pub(crate) enum TypeOperand<'x> {
    Reaching(&'x DeliveredCarried),
    Pure(KType<'static>),
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

/// [`StepAllocator::alloc_carried_with_scope`]'s dep-fold accumulator: the destination region
/// handle paired with the dep views folded in so far. The handle heads the tuple so the library
/// [`HasRegionHandle`](crate::witnessed::HasRegionHandle) blanket for `(RegionHandle<'b, P>, T)`
/// discharges each fold's composition seam. Layout-invariant in `'r`: a thin handle and a `Vec` of
/// the layout-invariant [`CarriedFamily`] are each layout-invariant, so the pair is too.
struct ScopeFoldViews;
reattachable!(ScopeFoldViews => (RegionHandle<'r, KoanStorageProfile>, Vec<Carried<'r>>));

/// [`StepAllocator::alloc_carried_with_scope`]'s final accumulator: the [`ScopeFoldViews`] pair
/// with the crossed scope envelope re-anchored at the brand nested alongside. Same handle-first
/// nesting so the `(RegionHandle<'b, P>, T)` blanket still applies.
struct ScopeFoldOperands;
reattachable!(ScopeFoldOperands => (RegionHandle<'r, KoanStorageProfile>, (Vec<Carried<'r>>, &'r Scope<'r>)));

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
    /// A saw-a-region-pointer recorder: each `owns_*` leaf (a `KFunction` / `Module` /
    /// `ModuleSignature` pointer — the residence side-table's recorded region pointers) sets it. A
    /// walk that passes the audit and set this reports a value whose borrows reach *some* region; a
    /// value freshly stored in the scope's own region (where every pointer is home by construction)
    /// reads it as its honest home-borrow bit ([`Scope::seal_fresh_object`]). `None` when a caller
    /// wants the plain audit with no recording.
    seen: Option<&'d Cell<bool>>,
}

impl<'d> Residence<'d> {
    pub(crate) fn dest_only(dest: &'d KoanRegion) -> Self {
        Residence {
            dest,
            reach: &[],
            ambient: None,
            seen: None,
        }
    }

    /// [`Self::dest_only`] with a saw-a-region-pointer recorder — the [`Self::seen`] flag is set
    /// while the walk visits any `owns_*` region-pointer leaf.
    pub(crate) fn dest_only_seen(dest: &'d KoanRegion, seen: &'d Cell<bool>) -> Self {
        Residence {
            dest,
            reach: &[],
            ambient: None,
            seen: Some(seen),
        }
    }

    pub(crate) fn with_reach(dest: &'d KoanRegion, reach: &'d [&'d FrameSet]) -> Self {
        Residence {
            dest,
            reach,
            ambient: None,
            seen: None,
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
            seen: None,
        }
    }

    /// Record a visited region-pointer leaf into [`Self::seen`], if a recorder is attached.
    fn note_region_pointer(&self) {
        if let Some(seen) = self.seen {
            seen.set(true);
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
        self.note_region_pointer();
        self.dest.owns_module(module as *const Module<'_>)
            || self.covers_region(module.child_scope().region())
    }

    pub(crate) fn owns_signature(&self, sig: &ModuleSignature<'_>) -> bool {
        self.note_region_pointer();
        self.dest.owns_signature(sig as *const ModuleSignature<'_>)
            || self.covers_region(sig.decl_scope().region())
    }

    pub(crate) fn owns_function(&self, f: &KFunction<'_>) -> bool {
        self.note_region_pointer();
        self.dest.owns_function(f as *const KFunction<'_>)
            || self.covers_region(f.captured_scope().region())
    }
}

/// The typed residence evidence a move-in site hands to an [`AuditedStored`] audit — the
/// call-site half of a [`Residence`], without the destination region (the audit takes that from
/// the handle it runs against). A family's `audit` builds a [`Residence`] from `(region, self)`
/// and runs the family's own residence walk over it. Fields are private and mirror [`Residence`]'s
/// evidence fields: `reach` are the reach sets a foreign borrow may legitimately land in, `ambient`
/// (when present) is the destination scope's own [`Scope::covers_region_ambiently`], and `seen` is
/// the walk's saw-a-region-pointer recorder.
///
/// [`Self::dest_only`] and [`Self::dest_only_seen`] are freely mintable within `machine::core`; the
/// ambient-bearing form ([`Self::reaching_ambient`]) is module-private, minted only by [`Scope`]'s
/// own evidence-tier methods, so the ambient predicate is always the destination scope's own
/// coverage — a builtin cannot mint a permissive (always-true ambient) context.
pub struct ResidenceEvidence<'ctx> {
    reach: &'ctx [&'ctx FrameSet],
    ambient: Option<&'ctx dyn Fn(&KoanRegion) -> bool>,
    seen: Option<&'ctx Cell<bool>>,
}

impl<'ctx> ResidenceEvidence<'ctx> {
    /// Dest-only evidence: the audit vets `value` against the destination region alone.
    pub(crate) fn dest_only() -> Self {
        ResidenceEvidence {
            reach: &[],
            ambient: None,
            seen: None,
        }
    }

    /// [`Self::dest_only`] with a saw-a-region-pointer recorder — the [`Residence::seen`] flag the
    /// checked-stored sites read after the store to derive a value's home-borrow bit.
    pub(crate) fn dest_only_seen(seen: &'ctx Cell<bool>) -> Self {
        ResidenceEvidence {
            reach: &[],
            ambient: None,
            seen: Some(seen),
        }
    }

    /// The reaching evidence tier: `reach`'s foreign sets plus the destination scope's own ambient
    /// coverage. Module-private so only [`Scope`]'s evidence-tier methods mint it — binding
    /// `ambient` to the destination scope by construction.
    fn reaching_ambient(
        reach: &'ctx [&'ctx FrameSet],
        ambient: &'ctx dyn Fn(&KoanRegion) -> bool,
    ) -> Self {
        ResidenceEvidence {
            reach,
            ambient: Some(ambient),
            seen: None,
        }
    }
}

// SAFETY: `audit` returns true only when every region borrow the stored `KType` carries is
// resident in `region`, covered by `context`'s reach evidence, or (when the ambient predicate is
// present) covered by the destination scope's own ambient coverage — the exact residence the
// `KType` walk verifies. Exact for `KType`: its only region pointers (`&Module` / `&ModuleSignature`
// / `&KFunction`) each expose their owning region, so no member enumeration is needed.
unsafe impl AuditedStored<KoanStorageProfile> for KType<'static> {
    type AuditContext<'ctx> = ResidenceEvidence<'ctx>;
    fn audit(region: &KoanRegion, value: &KType<'_>, context: ResidenceEvidence<'_>) -> bool {
        match (context.ambient, context.seen) {
            (Some(ambient), _) => {
                // The plain evidence-only check first (cheap, no closure alloc, directly
                // unit-testable in isolation); only fall back to the ambient-widened walk when it
                // declines.
                value.resident_in_reach(region, context.reach)
                    || value.resident_in_visiting(
                        &Residence::with_reach_and_ambient(region, context.reach, ambient),
                        &mut Vec::new(),
                    )
            }
            (None, Some(seen)) => value
                .resident_in_visiting(&Residence::dest_only_seen(region, seen), &mut Vec::new()),
            (None, None) => value.resident_in(region),
        }
    }
}

// SAFETY: `audit` returns true only when every answerable region borrow the stored `KObject`
// carries is resident in `region`, covered by `context`'s reach evidence, or (when the ambient
// predicate is present) covered by the destination scope's own ambient coverage — the residence the
// `KObject` walk verifies. Honest-partial in the one place the walk is (`Wrapped { type_id }`, whose
// `&KType` opts out of the residence side-table); every other borrow is checked.
unsafe impl AuditedStored<KoanStorageProfile> for KObject<'static> {
    type AuditContext<'ctx> = ResidenceEvidence<'ctx>;
    fn audit(region: &KoanRegion, value: &KObject<'_>, context: ResidenceEvidence<'_>) -> bool {
        match (context.ambient, context.seen) {
            (Some(ambient), _) => {
                // The plain evidence-only check first (cheap, directly unit-testable); only fall
                // back to the ambient-widened walk when it declines.
                value.resident_in_delivered(region, context.reach)
                    || value.resident_in_visiting(&Residence::with_reach_and_ambient(
                        region,
                        context.reach,
                        ambient,
                    ))
            }
            (None, Some(seen)) => {
                value.resident_in_visiting(&Residence::dest_only_seen(region, seen))
            }
            (None, None) => value.resident_in(region),
        }
    }
}

// SAFETY: `audit` returns true only when `region` is the very region that owns the stored
// `KFunction`'s captured scope — the function borrows that scope, so a store elsewhere would
// lengthen the borrow's lifetime past its region.
unsafe impl AuditedStored<KoanStorageProfile> for KFunction<'static> {
    type AuditContext<'ctx> = ();
    fn audit(region: &KoanRegion, value: &KFunction<'_>, _context: ()) -> bool {
        std::ptr::eq(region, value.captured_scope().region())
    }
}

// SAFETY: `audit` returns true only when `region` is the region the stored `Scope` names as its
// own — every `Scope` borrows its parent, so a store into any other region would dangle.
unsafe impl AuditedStored<KoanStorageProfile> for Scope<'static> {
    type AuditContext<'ctx> = ();
    fn audit(region: &KoanRegion, value: &Scope<'_>, _context: ()) -> bool {
        std::ptr::eq(region, value.region())
    }
}

// SAFETY: `audit` returns true only when the stored `Module`'s child scope's region is `region`
// itself, covered by `context`'s reach evidence, or covered by the destination scope's ambient
// coverage — the `Module` borrows that child scope, so its region must be covered. Honest-partial:
// the audit does not walk the `type_members` / `slot_type_tags` maps or the `self_sig` cell,
// which also carry region-borrowing `KType`s — sound because every store site allocates the
// module empty (`Module::new`), with those maps and the cell installed only post-store through
// `RefCell` / `OnceCell` interior mutability, a channel no store-time audit can see (the hotspot
// map's "Module member installs" row).
unsafe impl AuditedStored<KoanStorageProfile> for Module<'static> {
    type AuditContext<'ctx> = ResidenceEvidence<'ctx>;
    fn audit(region: &KoanRegion, value: &Module<'_>, context: ResidenceEvidence<'_>) -> bool {
        let residence = match context.ambient {
            Some(ambient) => Residence::with_reach_and_ambient(region, context.reach, ambient),
            None => Residence::dest_only(region),
        };
        residence.covers_region(value.child_scope().region())
    }
}

// SAFETY: `audit` returns true only when `region` is the region that owns the stored
// `ModuleSignature`'s decl scope — the signature borrows that scope, so a store elsewhere dangles.
unsafe impl AuditedStored<KoanStorageProfile> for ModuleSignature<'static> {
    type AuditContext<'ctx> = ();
    fn audit(region: &KoanRegion, value: &ModuleSignature<'_>, _context: ()) -> bool {
        std::ptr::eq(region, value.decl_scope().region())
    }
}

/// The step-branded construction context: the library [`StepContext`] over the step's destination
/// frame, confined to the scheduler step that minted it by the brand lifetime `'step`. The koan
/// construction doors live here — not on `StepContext` itself — because [`RegionBrand`]'s
/// constructor is private to this module (see [`FrameStorage::brand`]): each door's closure
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
pub struct StepAllocator<'step> {
    context: StepContext<FrameStorage>,
    step: PhantomData<&'step ()>,
}

impl Clone for StepAllocator<'_> {
    fn clone(&self) -> Self {
        StepAllocator {
            context: self.context.clone(),
            step: PhantomData,
        }
    }
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
        // `outer` to the same frame that owns `outer_b`'s region. That chain is **derived**, not
        // asserted: `CallFrame::new` computes it from the parent scope's own `region_owner`
        // (`Scope::parent_frame_pin`), root-region parents chain nothing, and the one deliberate
        // no-chain frame is the reserved `CallFrame::new_tail`.
        //
        // The store runs the real `Scope` family audit — the same live O(1)
        // `ptr::eq(region, value.region())` as `alloc_scope`. `child` is built over
        // `RegionBrand(handle_b)`, so `child.region()` is `handle_b`'s own region and the check
        // holds by construction; the parent-liveness chain above stays typed by `CallFrame::new`.
        let child = Scope::child_for_frame_witnessed(outer_b, RegionBrand(handle_b), region_owner);
        let live = handle_b
            .alloc_resident_checked::<Scope<'static>>(child, ())
            .expect("frame child is built over this frame's own region");
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
    /// Build a fresh per-call frame whose child `Scope` uses `outer` as its `outer` link. The
    /// storage pin chained for the parent is **derived** from `outer` via
    /// [`Scope::parent_frame_pin`]: the parent scope's own region owner when it is per-call, or no
    /// chain when the parent lives in the run-root region (which outlives the run). No caller can
    /// under-pin — there is no pin parameter to mis-wire; the one deliberate no-chain frame is the
    /// TCO fresh-tail cart, minted by the reserved [`Self::new_tail`].
    pub fn new<'p>(outer: &'p Scope<'p>) -> Rc<CallFrame> {
        Self::with_parent_pin(outer, outer.parent_frame_pin())
    }

    /// The TCO fresh-tail cart: a frame that strong-owns no ancestor (`outer_frame = None`), so tail
    /// recursion stays constant-space and no back-edge forms. The captured scope's liveness rides the
    /// closure value's carrier and the return contract's witness, not the `FrameStorage.outer` chain
    /// (see `design/tail-call-optimization.md`). `pub(in crate::machine)` reserves it to the
    /// fresh-tail placement (`resolve_frame_placement`, in `crate::machine`); builtins live in
    /// `crate::builtins` and cannot name it, so the no-chain shape is unreachable to them.
    pub(in crate::machine) fn new_tail<'p>(outer: &'p Scope<'p>) -> Rc<CallFrame> {
        Self::with_parent_pin(outer, None)
    }

    /// Shared body of [`Self::new`] and [`Self::new_tail`]: build the frame with `outer_frame` as the
    /// parent pin the fresh storage's `outer` chain holds.
    fn with_parent_pin<'p>(
        outer: &'p Scope<'p>,
        outer_frame: Option<Rc<FrameStorage>>,
    ) -> Rc<CallFrame> {
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
    pub(crate) fn storage(&self) -> &Rc<FrameStorage> {
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
