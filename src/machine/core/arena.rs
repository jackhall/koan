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

use crate::machine::{CarrierWitness, KError, KErrorKind};
use std::rc::Rc;

use crate::machine::execute::StepCarried;

use super::scope::Scope;
use crate::machine::core::kfunction::KFunction;
use crate::machine::model::OperatorGroup;
use crate::machine::model::{
    Carried, CarriedFamily, ContainerSubstrate, Held, KObject, ListSubstrate, Module, Record,
    RecordSubstrate,
};
use crate::machine::model::{KType, TypeIdentifier, TypeRegistry};
use crate::witnessed::reattachable;
use crate::witnessed::{
    Erased, FamilyArena, FoldedPlacement, Reattachable, Region, RegionHandle, RegionSet, StorageOf,
    StorageProfile, Stored, Witnessed,
};

mod frame;
mod residence;
mod step_allocator;

pub(crate) use frame::FrameStorageExt;
pub use frame::{run_root_storage, CallFrame, FrameSet, FrameStorage};
pub(crate) use residence::Residence;
use residence::ResidenceEvidence;
pub use step_allocator::StepAllocator;

/// The Koan workload: the family set whose library-derived bundle a [`Region`] owns — one library
/// [`FamilyArena`] cell per family. The `KType` cell backs per-type identity binding storage
/// (`Bindings::types`); the `OperatorGroup` cell backs the per-scope operator registry
/// (`Bindings::operators`); the `TypeIdentifier` cell backs the type channel's unlowered-name
/// carrier ([`Carried::UnresolvedType`]).
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
                        KType,
                        (
                            OperatorGroup,
                            (
                                FrameSet,
                                (
                                    TypeIdentifier,
                                    (RecordSubstrate<'static>, (ListSubstrate<'static>, ())),
                                ),
                            ),
                        ),
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

    /// The storage door for a [`TypeIdentifier`] the bind seam left unlowered. Owned surface data —
    /// no variant borrows region content — so the store is safe and unchecked.
    pub fn alloc_type_identifier(self, ti: TypeIdentifier) -> &'a TypeIdentifier {
        self.0.alloc_resident::<TypeIdentifier>(ti)
    }

    /// Runtime-checked twin of [`Self::alloc_object`] for an `o` that cannot rebuild owned at
    /// `'static` (`KObject` has no general `'static` rebuild):
    /// [`KObject::resident_in`] audits every region borrow `o` carries against this brand's own
    /// region. A `Wrapped { type_id }` tag needs no walk: the `type_id` is a `Copy` `KType` handle
    /// that reaches nothing the audit could reject.
    pub fn alloc_object_checked(
        self,
        o: KObject<'_>,
        types: &TypeRegistry,
    ) -> Result<&'a KObject<'a>, KError> {
        // The audit consumes `o`, so its type is read before the call — but rendering that type is
        // the diagnostic's cost alone, so it stays inside the failure closure.
        let kt = o.ktype();
        self.0
            .alloc_resident_checked::<KObject<'static>>(o, ResidenceEvidence::dest_only())
            .ok_or_else(|| {
                KError::new(KErrorKind::ShapeError(format!(
                    "{}: borrows a region other than its seal's destination",
                    kt.name(types)
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
    /// [`is_splice_free`](crate::machine::model::KExpression::is_splice_free) flag, so a spliced
    /// expression (a resolved value carrying a producer reach the empty seal cannot name) is
    /// rejected. Storing nothing on a failed audit; a foreign-region dangle errors loudly instead of
    /// landing unvetted.
    pub(crate) fn alloc_object_witnessed_checked(
        self,
        value: KObject<'_>,
        types: &TypeRegistry,
    ) -> Result<StepCarried<'a>, KError> {
        let name = value.ktype().name(types);
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
    /// `KObject<'b>` are values derived from the fold's declared operand views, the brand's own
    /// allocations, and owned/`'static` data — all named by the witness the enclosing combinator
    /// composes. An ambient-lifetime capture is a compile error at this signature (a
    /// `KObject<'ambient>` cannot coerce to `KObject<'b>`, since `'b` has no outlives relation to any
    /// enclosing lifetime), so the store is discharged at compile time by the placement capability,
    /// with no runtime audit at all.
    pub(crate) fn alloc_object_folded(self, o: KObject<'a>) -> &'a KObject<'a> {
        self.placement.alloc_resident_folded::<KObject<'static>>(o)
    }

    /// Store a container substrate built at this fold's own brand — the container door, generic over
    /// the substrate payload family `K` (its `'static` [`Stored`] form). Sound by the same rank-2
    /// fold-brand argument as [`Self::alloc_object_folded`]: `substrate` is typed at the brand
    /// lifetime, so an ambient-lifetime capture is a compile error at this signature, discharging the
    /// store's residence obligation at compile time. Each `ContainerSubstrate<C>` family lands in its
    /// own sub-arena slot (its [`Stored`] impl) — the record and list substrates hand-add their
    /// entries; a macro lifts the per-family boilerplate at the third instantiation (the dict
    /// conversion).
    pub(crate) fn alloc_substrate_folded<K: Stored<KoanStorageProfile>>(
        self,
        substrate: K::At<'a>,
    ) -> &'a K::At<'a> {
        self.placement.alloc_resident_folded::<K>(substrate)
    }
}

// The lifetime family of each stored type, keyed on its `'static` form — the GAT the
// `Region` engine erases to `'static` for storage and re-anchors to the caller's `'a` on read.
// Each family is one type generic only in a single lifetime, so its layout is identical for every
// choice of that lifetime; `KType`, `OperatorGroup` and `TypeIdentifier` are lifetime-free,
// trivially invariant. The shared
// `reattachable!` macro discharges the layout-invariance `unsafe` obligation once (see its docs).
reattachable! {
    KObject<'static> => KObject<'r>,
    KType => KType,
    KFunction<'static> => KFunction<'r>,
    Scope<'static> => Scope<'r>,
    Module<'static> => Module<'r>,
    OperatorGroup => OperatorGroup,
    TypeIdentifier => TypeIdentifier,
    ContainerSubstrate<Record<Held<'static>>> => ContainerSubstrate<Record<Held<'r>>>,
    ContainerSubstrate<Vec<Held<'static>>> => ContainerSubstrate<Vec<Held<'r>>>,
}

/// A witnessed-construction operand bundling a destination region's [`RegionHandle`] with a
/// type-channel identity (a `SetMember` / declared type) that must cross the build brand. A
/// value-embedding construction `transfer_into`/`merge`s its object carrier with this operand so the
/// wrapped value lands — allocated through the handle — tagged by the identity, both re-anchored to
/// the build brand under the same witness; the dest frame's `outer` chain pins the identity's
/// (ancestor) region. Used by the newtype / tagged-union constructors and the `CATCH` `Result`
/// build. Layout-invariant: a thin pointer and a `Copy` `KType` handle, representation independent
/// of `'r`.
pub struct RegionTypeFamily;
reattachable!(RegionTypeFamily => (RegionHandle<'r, KoanStorageProfile>, KType));

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

impl Stored<KoanStorageProfile> for KType {
    fn cell(s: &StorageOf<KoanStorageProfile>) -> &FamilyArena<Self> {
        &s.1 .1 .1 .1 .0
    }
}

impl Stored<KoanStorageProfile> for OperatorGroup {
    fn cell(s: &StorageOf<KoanStorageProfile>) -> &FamilyArena<Self> {
        &s.1 .1 .1 .1 .1 .0
    }
}

impl Stored<KoanStorageProfile> for FrameSet {
    fn cell(s: &StorageOf<KoanStorageProfile>) -> &FamilyArena<Self> {
        &s.1 .1 .1 .1 .1 .1 .0
    }
}

impl Stored<KoanStorageProfile> for TypeIdentifier {
    fn cell(s: &StorageOf<KoanStorageProfile>) -> &FamilyArena<Self> {
        &s.1 .1 .1 .1 .1 .1 .1 .0
    }
}

impl Stored<KoanStorageProfile> for ContainerSubstrate<Record<Held<'static>>> {
    fn cell(s: &StorageOf<KoanStorageProfile>) -> &FamilyArena<Self> {
        &s.1 .1 .1 .1 .1 .1 .1 .1 .0
    }
    fn record_local(frame: &KoanRegion, stored: &ContainerSubstrate<Record<Held<'static>>>) {
        frame.record_addr(stored as *const _ as usize);
    }
}

impl Stored<KoanStorageProfile> for ContainerSubstrate<Vec<Held<'static>>> {
    fn cell(s: &StorageOf<KoanStorageProfile>) -> &FamilyArena<Self> {
        &s.1 .1 .1 .1 .1 .1 .1 .1 .1 .0
    }
    fn record_local(frame: &KoanRegion, stored: &ContainerSubstrate<Vec<Held<'static>>>) {
        frame.record_addr(stored as *const _ as usize);
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
    /// [`alloc_object`](RegionBrand::alloc_object)) or a `Carried::Type` (a `Copy` `KType` handle,
    /// needing no storage door). A value that *references* another region's resident
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

    /// Whether `ptr` was returned by a prior `alloc_module` on this region — the residence audit's
    /// check for a `KObject::Module` payload.
    fn owns_module<'a>(&self, ptr: *const Module<'a>) -> bool;

    /// Whether `ptr` was returned by a prior `alloc_function` on this region — the residence
    /// audit's check for a `KObject::KFunction` payload.
    fn owns_function<'a>(&self, ptr: *const KFunction<'a>) -> bool;

    /// Whether `ptr` was returned by a prior `alloc_substrate_folded` on this region —
    /// [`Residence::owns_substrate`](super::Residence::owns_substrate)'s single-region address
    /// check, the same shape as [`Self::owns_function`] but with no scope-region shortcut: a
    /// `ContainerSubstrate<C>` carries no borrow naming its own home region, so the residence walk
    /// widens this with a per-reach-member check rather than a single `covers_region` call.
    fn owns_substrate<C>(&self, ptr: *const ContainerSubstrate<C>) -> bool;

    /// Total bytes allocated across this region's ten Koan families — each family's live count
    /// weighted by the flat size of its stored `'static` form. Prices the host region only, not the
    /// `outer` chain its `Rc<FrameStorage>` also retains (a documented approximation): the cost-copy
    /// seam reads this as the denominator of the payoff ratio, where the host's own footprint is the
    /// relevant scale. `#[allow(dead_code)]` for the same reason as [`Self::owns_object`]: the plain
    /// `--lib` build (no `cfg(test)`) can't see its consumer.
    #[allow(dead_code)]
    fn allocated_total(&self) -> u64;
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
        // ([`WitnessRegion`](crate::witnessed::WitnessRegion)) — the brand proves the built value
        // is region-derived — then
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

    fn owns_function<'a>(&self, ptr: *const KFunction<'a>) -> bool {
        // `KFunction` is invariant in `'a`, so the through-`'static` cast is required despite
        // clippy's complaint.
        #[allow(clippy::unnecessary_cast)]
        let target = ptr as *const KFunction<'static> as usize;
        self.owns_addr(target)
    }

    fn owns_substrate<C>(&self, ptr: *const ContainerSubstrate<C>) -> bool {
        let target = ptr as usize;
        self.owns_addr(target)
    }

    fn allocated_total(&self) -> u64 {
        fn weigh<K: Stored<KoanStorageProfile>>(region: &KoanRegion) -> u64 {
            region.family_len::<K>() as u64 * std::mem::size_of::<K>() as u64
        }
        weigh::<KObject<'static>>(self)
            + weigh::<KFunction<'static>>(self)
            + weigh::<Scope<'static>>(self)
            + weigh::<Module<'static>>(self)
            + weigh::<KType>(self)
            + weigh::<OperatorGroup>(self)
            + weigh::<FrameSet>(self)
            + weigh::<TypeIdentifier>(self)
            + weigh::<RecordSubstrate<'static>>(self)
            + weigh::<ListSubstrate<'static>>(self)
    }
}

/// Test-only allocation counting over the generic [`Region`] — an extension trait for the same
/// reason as [`KoanRegionExt`].
#[cfg(test)]
pub(crate) trait KoanRegionTestExt {
    /// Total number of values stored across the counted sub-arenas. Each `alloc_*` writes to
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
            + self.family_len::<KType>()
            + self.family_len::<OperatorGroup>()
            + self.family_len::<FrameSet>()
            + self.family_len::<RecordSubstrate<'static>>()
            + self.family_len::<ListSubstrate<'static>>()
    }
}

#[cfg(test)]
mod tests;
