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

use crate::machine::CarrierWitness;
use std::rc::Rc;

use crate::machine::execute::StepCarried;

use super::scope::Scope;
use crate::machine::core::kfunction::KFunction;
use crate::machine::model::KType;
use crate::machine::model::{
    Carried, CarriedFamily, ContainerSubstrate, Held, KExpression, KObject, Module, Scalar,
};
use crate::witnessed::reattachable;
use crate::witnessed::{
    BumpMap, Erased, FamilyArena, FoldedPlacement, Reattachable, Region, RegionHandle, StepContext,
    StorageOf, StorageProfile, Stored, Witnessed,
};

mod frame;
mod residence;
mod step_allocator;

pub(crate) use frame::FrameStorageExt;
pub use frame::{
    program_storage, run_root_storage, CallFrame, FrameCoverage, FrameReach, FrameStorage,
    ProgramBrand, ProgramStorage,
};
pub use step_allocator::StepAllocator;

/// The Koan workload: the family set whose library-derived bundle a [`Region`] owns — one library
/// [`FamilyArena`] cell per family.
///
/// **Exactly the three families designed to own things.** A `KFunction` owns its captured binding
/// table, a `Scope` its bindings, a `Module` its member map — each runs a real `Drop` at region death
/// and so needs a typed cell that will run it. Every other value family is `Drop`-free by
/// construction (`Copy`, checked at the bump doors) and lives in the region's bump instead, where
/// death is chunk deallocation and no per-slot glue runs at all. See
/// [value-substrates.md § Untyped arenas](../../../design/value-substrates.md#untyped-arenas-the-drop-free-end-state).
///
/// A [`TypeIdentifier`](crate::machine::model::TypeIdentifier) and a [`KType`] need no cell either:
/// both are `Copy` handles — a borrow of a name already resident where it was parsed, and an interned
/// registry index — so the type channel's carriers hold them by value.
pub struct KoanStorageProfile;

impl StorageProfile for KoanStorageProfile {
    type Families = (KFunction<'static>, (Scope<'static>, (Module<'static>, ())));

    /// Reach descriptions live in the region's side table, typed at the per-call frame owner.
    type FrameOwner = FrameStorage;
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
/// (the witnessed surface, where a construction door builds under a `for<'b>` brand and returns a
/// `Witnessed` carrier); this handle is for the co-located plumbing.
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

    /// Store an owned, region-free leaf into the region (no value holds an owning `Rc` back to a
    /// region, so the store forms no back-edge). Yields a co-located `&'a` resident.
    ///
    /// [`Scalar`] is the gate, and it is the *signature*: a value that borrows any region has no way
    /// to spell itself as one, so it cannot reach this door and takes a fold or merge whose
    /// composition names what it borrows instead. The cell lands in the bump, so region death frees it
    /// with the chunks.
    pub fn alloc_scalar(self, scalar: Scalar) -> &'a KObject<'a> {
        self.alloc_value(scalar.into_object())
    }

    /// [`Self::alloc_scalar`] for a string: copy the bytes into this region ([`Self::alloc_text`]) and
    /// store the cell around the co-located borrow. A separate door because a string is exactly the
    /// leaf whose *representation* is region-hosted even though its meaning is owned — so re-homing
    /// the bytes is the store, and there is nothing for a caller to get wrong between the two.
    pub fn alloc_string(self, text: &str) -> &'a KObject<'a> {
        self.alloc_value(KObject::KString(self.alloc_text(text)))
    }

    /// **Copy `text` into this brand's region** and hand back the co-located `&'a str` — the door
    /// every string a value family slot holds is born through ([`KObject::KString`], a
    /// `Tagged` discriminant, a [`KKey::String`](crate::machine::model::KKey) dict key).
    ///
    /// The bytes land in the region's bump, so they cost region teardown nothing: the bump releases
    /// its chunks whole and runs no destructor, which is what keeps a string slot `Copy` and the
    /// value family free of `Drop` glue. The borrow the caller gets back is checked against this
    /// brand's own `'a`, so no audit and no reach vocabulary enter here — bare bytes reach nothing.
    ///
    /// Storing the *value* built around those bytes is gated at its own door. [`Self::alloc_string`]
    /// re-homes them itself, so its product is resident by construction; a string already living in
    /// another region takes [`FoldingBrand::alloc_object_folded`], where the rank-2 brand proves it was
    /// bumped at the destination. No address probe could stand in for either, because the bump keeps no
    /// address table and so cannot say which region a `&str` points into.
    pub fn alloc_text(self, text: &str) -> &'a str {
        self.0.bump_text(text)
    }

    /// The storage door for a run of `Drop`-free items — an expression's parts, a literal's
    /// elements, a node's stored bucket key. The bump hands back the whole run as one borrow, so
    /// region death runs no per-item `Drop` and copying the run to another region is a memcpy.
    ///
    /// `T: Copy` is the whole gate: a `Copy` item owns no allocation, so anything it points at is
    /// already resident wherever it was bumped. What the caller must get right is *which* region —
    /// the same rule [`Self::alloc_text`] states for a string's bytes.
    pub fn alloc_slice<T: Copy>(self, items: &[T]) -> &'a [T] {
        self.0.bump_slice(items)
    }

    /// The single-value peer of [`Self::alloc_slice`], for a node a part arm points at.
    pub fn alloc_value<T: Copy>(self, value: T) -> &'a T {
        self.0.bump_value(value)
    }

    /// The keyed peer of [`Self::alloc_slice`], for an index whose lookup wants a hash table rather
    /// than a sorted run and a binary search — a [`DictSubstrate`]'s key→index table is the one such
    /// index. Buckets and header both land in this brand's region bump, so region death frees the
    /// table without running its `Drop`; the `Copy` bounds on the key and the value are what make
    /// that lossless, exactly as they are for a slice's items.
    pub fn alloc_map<K, V>(self, entries: impl IntoIterator<Item = (K, V)>) -> &'a BumpMap<'a, K, V>
    where
        K: Copy + Eq + std::hash::Hash,
        V: Copy,
    {
        self.0.bump_map(entries)
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
    /// legitimate cross-region caller (transparent-ascribe's re-tagged `Module`) is built at a fold
    /// brand instead ([`Scope::store_transparent_view`]), where the borrow it re-tags is the fold's
    /// own operand view. See [`Self::alloc_function`].
    pub fn alloc_module(self, m: Module<'_>) -> &'a Module<'a> {
        self.0
            .alloc_resident_checked::<Module<'static>>(m, ())
            .expect("alloc_module: a Module must be allocated into its own child scope's region")
    }

    /// The witnessed-allocation surface for an owned leaf built fresh inside the brand — the
    /// arithmetic and comparison builtins' one store. Born under a description hosted in this region
    /// with **no members**: [`Self::alloc_scalar`] stores the value and [`Self::seal_resident`] names
    /// the region-pure obligation, so the active frame is deliberately excluded from the pins. The
    /// producing frame is folded in only at finalize/close (the scope-reach seal), so a
    /// region-resident value never strong-owns its own frame (the `region → object → frame` cycle that
    /// would keep the frame's `Rc` alive forever and defeat the refcount-driven region free).
    ///
    /// The within-step transient invariant is typed: the member-less carrier pins nothing, so it
    /// returns as a [`StepCarried`] branded at this brand's own `'a` — in production a step's
    /// rank-2 open lifetime — and the borrow checker rejects any use past the step. The active
    /// frame pins the region across the step, and the sole exit to node storage is the seal door in
    /// `step_carried.rs`, where finalize's fold names the producer in the carrier's own reach.
    ///
    /// [`Scalar`] is region-purity as a signature: a value that references another region cannot
    /// spell itself as one, and takes the `yoke` / `merge` path or
    /// [`Self::alloc_expression_witnessed`] instead.
    pub(crate) fn alloc_scalar_witnessed(self, scalar: Scalar) -> StepCarried<'a> {
        StepCarried::born(
            self.seal_resident::<CarriedFamily>(Carried::Object(self.alloc_scalar(scalar))),
        )
    }

    /// The store for a `#(...)` quote's body as data — the shape [`Self::alloc_scalar_witnessed`]
    /// cannot take, since `KObject<'a>` is invariant and raw AST has no `'static` rebuild. The
    /// signature is the enforcement: only a [`KExpression`](crate::machine::model::KExpression)
    /// reaches this door, and an AST node names no producer region, so the cell the door bumps here
    /// borrows nothing a seal would have to pin.
    ///
    /// The cell lands in this brand's own region bump, so its residence is where it was placed,
    /// and it costs region death nothing — an expression's parts are already bump-hosted runs.
    pub(crate) fn alloc_expression(self, expression: KExpression<'a>) -> &'a KObject<'a> {
        self.alloc_value(KObject::KExpression(expression))
    }

    /// [`Self::alloc_expression`] bundled as the resident carrier — the quote terminal's one call,
    /// sealed under the same member-less own-region description
    /// [`Self::alloc_scalar_witnessed`] mints.
    pub(crate) fn alloc_expression_witnessed(self, expression: KExpression<'a>) -> StepCarried<'a> {
        StepCarried::born(
            self.seal_resident::<CarriedFamily>(Carried::Object(self.alloc_expression(expression))),
        )
    }

    /// Bundle a value **already resident in this brand's region** whose borrows reach nothing — the
    /// terminal carrier a name / ATTR read hands back and an FN-def / LET define site seals its
    /// object with. Unlike [`alloc_scalar_witnessed`](Self::alloc_scalar_witnessed) the value is not
    /// stored here; it pre-exists in the region. The description is minted **here**, so no caller
    /// pairs a value with a residence it did not derive: its host is this brand's own region owner
    /// and its members are empty, which is the exact claim for a value that reaches nothing beyond
    /// the region it lives in. The reading / defining frame pins that region for the step, and past
    /// the step the scheduler's retention hold (the delivery envelope's host) carries the pin.
    ///
    /// A value that *does* reach somewhere takes [`Self::seal_reaching`] with the description
    /// [`Scope::mint_retained`](crate::machine::core::Scope) derived for it. The brand is the
    /// capability marker: only a handle into the region the value lives in may seal it resident.
    pub(crate) fn seal_resident<T: Reattachable>(
        self,
        value: T::At<'_>,
    ) -> Witnessed<T, CarrierWitness> {
        // A mint with no sources composes nothing, so the retained bundle is empty and the frozen
        // description names this region's owner as host and no member at all.
        self.seal_reaching(value, self.0.mint_retained(&[]))
    }

    /// [`Self::seal_resident`] for a value whose reach is already minted: bundle it under `reach`,
    /// the description a caller derived for this same value into this same region
    /// ([`Scope::mint_retained`](crate::machine::core::Scope)). The description carries the value's
    /// residence as its host, so the pairing this takes is one record, not two — there is no
    /// separate residence for a caller to get wrong.
    pub(crate) fn seal_reaching<T: Reattachable>(
        self,
        value: T::At<'_>,
        reach: &'a FrameReach,
    ) -> Witnessed<T, CarrierWitness> {
        Witnessed::from_erased(Erased::erase(value), CarrierWitness::new(reach))
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
    ///
    /// The cell lands in the destination's bump, through the fold placement's own
    /// [`bump`](FoldedPlacement::bump) door: a `KObject` is `Copy`, so region death frees the cell as
    /// a bump chunk and runs no per-slot glue. The placement is what makes this a *residence* door
    /// rather than the untargeted [`RegionBrand::alloc_value`] — the brand's `'a` is the fold's own,
    /// so a value resident somewhere else cannot be written here.
    pub(crate) fn alloc_object_folded(self, o: KObject<'a>) -> &'a KObject<'a> {
        self.placement.bump().value(o)
    }

    /// Store a [`Module`] built at this fold's own brand — the door the module store folds
    /// ([`Scope::store_module_object`](crate::machine::core::Scope)) re-tag a view through. Sound by
    /// the same rank-2 fold-brand argument as [`Self::alloc_object_folded`]: the module is typed at
    /// the brand lifetime, so its child-scope borrow is the fold's own operand view and an
    /// ambient-lifetime capture is a compile error at this signature. That discharges the residence
    /// obligation `alloc_module`'s `ptr::eq` guard states at runtime — a folded module's child scope
    /// is reached through the fold's operands, whose regions the enclosing composition names.
    pub(crate) fn alloc_module_folded(self, m: Module<'a>) -> &'a Module<'a> {
        self.placement.alloc_resident_folded::<Module<'static>>(m)
    }

    /// Store a container substrate built at this fold's own brand — the container door, generic over
    /// the substrate payload family `K` (its `'static` [`Stored`] form). Sound by the same rank-2
    /// fold-brand argument as [`Self::alloc_object_folded`]: `substrate` is typed at the brand
    /// lifetime, so an ambient-lifetime capture is a compile error at this signature, discharging the
    /// store's residence obligation at compile time. A substrate is `Copy` in every arm — its index
    /// is a bump-hosted name slice or a [`BumpMap`], its cells a [`Sectioned`] run — so it takes the
    /// same bump door as [`Self::alloc_object_folded`] and costs region death nothing.
    pub(crate) fn alloc_substrate_folded<C: Copy>(
        self,
        substrate: ContainerSubstrate<'a, C>,
    ) -> &'a ContainerSubstrate<'a, C> {
        self.placement.bump().value(substrate)
    }

    /// Store one container cell at this fold's own brand, handing back the resident `&'a Held<'a>`
    /// borrow the sectioned alloc door takes as its payload
    /// ([`Sectioned::build`](crate::witnessed::Sectioned::build)). Sound by the same rank-2
    /// fold-brand argument as [`Self::alloc_object_folded`]: the cell is typed at the brand lifetime,
    /// so an ambient-lifetime capture is a compile error at this signature. Residing the cell before
    /// the door runs is what ties it to the same `'a` the container's run descriptions are interned
    /// at, so one pin covers a projected cell and its reach together. A `Held` is `Copy`, so the cell
    /// takes the same bump door as [`Self::alloc_object_folded`].
    pub(crate) fn alloc_cell_folded(self, cell: Held<'a>) -> &'a Held<'a> {
        self.placement.bump().value(cell)
    }

    /// This brand as a [`SubstrateDoor`] over `holder` — the coverage the enclosing fold's operand
    /// envelopes hold, which is the proof the door reads its cells' stored reach under.
    pub(crate) fn with_holder<'h>(self, holder: &'h FrameCoverage) -> SubstrateDoor<'a, 'h> {
        SubstrateDoor {
            brand: self,
            holder,
        }
    }
}

/// The door every composite substrate is born through: a [`FoldingBrand`] plus the **holder-rule
/// proof** its per-cell reach verdicts are read under.
///
/// A cell that keeps borrowing a foreign source hands the sectioned alloc door that source's stored
/// description ([`CellReach::Pinned`](crate::witnessed::CellReach::Pinned)), and reading a
/// description's members back out is sound only while something pins every region it names. Inside a
/// fold closure the operands' pins are held by the enclosing combinator — but a `for<'b>` closure has
/// no route back to them, so the coverage is captured at the call site and moved in. Pairing it with
/// the brand makes that obligation part of the door's type: a container cannot be built through a
/// brand alone.
///
/// There is no holderless door: a site whose cells are all owned data names
/// [`FrameCoverage::empty`] explicitly, so "nothing to prove here" is a claim written at the call
/// site rather than a shape the door lets a caller fall into.
///
/// Everything else derefs to the brand, so a closure that also allocates objects or type identifiers
/// through the door is unaffected.
#[derive(Clone, Copy)]
pub struct SubstrateDoor<'a, 'h> {
    brand: FoldingBrand<'a>,
    holder: &'h FrameCoverage,
}

impl<'a> std::ops::Deref for SubstrateDoor<'a, '_> {
    type Target = FoldingBrand<'a>;
    fn deref(&self) -> &FoldingBrand<'a> {
        &self.brand
    }
}

impl SubstrateDoor<'_, '_> {
    /// The holder-rule proof this door reads stored cell reach under, as a coverage the door hands
    /// on to the alloc door per pinned cell — see the type's own doc.
    pub(crate) fn holder(&self) -> FrameCoverage {
        self.holder.clone()
    }
}

// The lifetime family of each stored type, keyed on its `'static` form — the GAT the
// `Region` engine erases to `'static` for storage and re-anchors to the caller's `'a` on read.
// Each family is one type generic only in a single lifetime, so its layout is identical for every
// choice of that lifetime; `KType` is lifetime-free, trivially invariant. The
// shared `reattachable!` macro discharges the layout-invariance `unsafe` obligation once (see its
// docs).
reattachable! {
    KFunction<'static> => KFunction<'r>,
    Scope<'static> => Scope<'r>,
    Module<'static> => Module<'r>,
    Held<'static> => Held<'r>,
}

/// A witnessed-construction operand bundling a destination region's [`RegionHandle`] with a
/// type-channel identity (a `SetMember` / declared type) that must cross the build brand. A
/// value-embedding construction `transfer_into`s its object carrier into this operand so the wrapped
/// value lands — allocated through the handle — tagged by the identity, both re-anchored to the
/// build brand under the same witness. The identity is a bare interned handle pointing into no
/// region, so the whole operand is born co-located in the dest region by a single yoke. Used by the
/// newtype / tagged-union constructors and the `CATCH` `Result` build. Layout-invariant: a thin
/// pointer and a `Copy` `KType` handle, representation independent of `'r`.
pub struct RegionTypeFamily;
reattachable!(RegionTypeFamily => (RegionHandle<'r, KoanStorageProfile>, KType));

// Per-family `Stored` policy: which sub-arena each of the three droppy families lands in. None
// carries a self-targeting `Rc<FrameStorage>` — a stored closure / module is a bare borrow into its
// defining region, kept alive by its carrier's witness set rather than an owned anchor — so no
// allocation can self-cycle and the engine needs no cycle gate. None records an address either:
// residence is answered by a value's own field (the three `ptr::eq` reattach guards) or by the
// construction door's brand, never by a side table.

impl Stored<KoanStorageProfile> for KFunction<'static> {
    fn cell(s: &StorageOf<KoanStorageProfile>) -> &FamilyArena<Self> {
        &s.0
    }
}

impl Stored<KoanStorageProfile> for Scope<'static> {
    fn cell(s: &StorageOf<KoanStorageProfile>) -> &FamilyArena<Self> {
        &s.1 .0
    }
}

impl Stored<KoanStorageProfile> for Module<'static> {
    fn cell(s: &StorageOf<KoanStorageProfile>) -> &FamilyArena<Self> {
        &s.1 .1 .0
    }
}

/// Koan's at-will allocation entry and identity queries over the generic [`Region`] — an extension
/// trait because `Region` lives in the `workgraph` crate and a foreign type takes no inherent impls.
/// Every co-located `alloc_*` lives on [`RegionBrand`] (minted via [`FrameStorage::brand`]); a bare
/// `&KoanRegion` keeps only the identity surface here.
pub(crate) trait KoanRegionExt {
    /// The alloc-witnessed construction inversion's region-pure primitive: build a value into
    /// `owner`'s region *inside* a **zero-dep fold**, returning it bundled with the [`FrameReach`]
    /// singleton pinning `owner` so it is co-located by construction rather than paired with an
    /// asserted witness. The closure receives a per-construction [`FoldingBrand`] confined to the
    /// `for<'b>` brand (it cannot escape the closure), so it allocates through the same capability as
    /// every other construction site. One primitive for both value families — the closure returns a
    /// `Carried::Object` (an [`alloc_object_folded`](FoldingBrand::alloc_object_folded)) or a
    /// `Carried::Type` (a `Copy` `KType` handle, needing no storage door). A value that *references*
    /// another region's resident value folds that in with [`Witnessed::merge_pinned`] instead,
    /// unioning its reach; this primitive covers the case whose references are all region-derived or
    /// owned, so the `for<'b>` brand admits them.
    ///
    /// The fold brand rather than a bare [`RegionBrand`] because a region-pure leaf is no longer
    /// necessarily `'static`: a string literal's bytes are bumped into this same region
    /// ([`RegionBrand::alloc_text`]), so the value is region-self-referential and only
    /// [`alloc_object_folded`](FoldingBrand::alloc_object_folded)'s rank-2 argument admits it. With no
    /// deps the fold composes nothing, so the product's reach is exactly what it was: this region and
    /// no member.
    ///
    /// `build`'s return is spelled `<CarriedFamily as Reattachable>::At<'b>`, not the concrete
    /// `Carried<'b>`: the two are equal by the family's definition, but under the `for<'b>` binder the
    /// compiler does not normalize the projection lazily, so a `build` typed `-> Carried<'b>` fails to
    /// satisfy the `-> T::At<'b>` bound. Naming the projection makes the bounds syntactically
    /// identical. An inline closure returning a `Carried` still unifies fine at the call site.
    // Drives the object-family construction inversion
    // (design/per-node-memory.md): a region-pure leaf builds its `KObject` inside this closure.
    fn fold_witnessed(
        owner: Rc<FrameStorage>,
        build: impl for<'b> FnOnce(FoldingBrand<'b>) -> <CarriedFamily as Reattachable>::At<'b>,
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

    /// Total bytes allocated in this region: each Koan family's live count weighted by the flat size
    /// of its stored `'static` form, plus the region's **bump occupancy**
    /// ([`Region::bump_bytes`]) — the string bytes a value family slot holds and the library's own
    /// sectioned-container metadata, which a pin would retain just as surely as an arena cell.
    /// Prices the host region only, not the
    /// `outer` chain its `Rc<FrameStorage>` also retains (a documented approximation): the cost-copy
    /// seam reads this as the denominator of the payoff ratio, where the host's own footprint is the
    /// relevant scale. `#[allow(dead_code)]` because trait methods, unlike inherent ones, are checked
    /// per compilation target, and the plain `--lib` build (no `cfg(test)`) can't see its consumer.
    #[allow(dead_code)]
    fn allocated_total(&self) -> u64;
}

impl KoanRegionExt for KoanRegion {
    fn fold_witnessed(
        owner: Rc<FrameStorage>,
        build: impl for<'b> FnOnce(FoldingBrand<'b>) -> <CarriedFamily as Reattachable>::At<'b>,
    ) -> Witnessed<CarriedFamily, CarrierWitness> {
        // A zero-dep fold: the engine composes no operand reach, so the envelope it hands back is
        // homed in `owner`'s own region with empty coverage — the same claim `yoke_branded`'s
        // reference-only re-bundle makes, reached through the fold door instead. Unsealing it drops
        // an empty coverage, so nothing a pin was holding is discarded.
        StepContext::new(owner)
            .alloc_with_handle::<KoanStorageProfile, CarriedFamily, CarriedFamily>(
                &[],
                |placement, _views| build(FoldingBrand::in_fold_closure(placement)),
            )
            .into_cell()
            .unseal()
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
        // [`into_reference_only`](Witnessed::into_reference_only) re-bundles under a reference-only
        // carrier hosted in that same region with no members: the value's reach is exactly its own
        // region, and its liveness is external (the active frame during the step, the scheduler's
        // retention hold once finalized). Turbofish `T` at the yoke: inference does not drive
        // `yoke`'s `T` from the return type early enough to check `build`'s `-> T::At<'b>` bound, so
        // it sees `<_ as Reattachable>::At` and fails to match the projection.
        Witnessed::<T, Rc<FrameStorage>>::yoke_handle(owner, |handle| build(RegionBrand(handle)))
            .into_reference_only::<KoanStorageProfile>()
    }

    fn allocated_total(&self) -> u64 {
        fn weigh<K: Stored<KoanStorageProfile>>(region: &KoanRegion) -> u64 {
            region.family_len::<K>() as u64 * std::mem::size_of::<K>() as u64
        }
        weigh::<KFunction<'static>>(self)
            + weigh::<Scope<'static>>(self)
            + weigh::<Module<'static>>(self)
            + self.bump_bytes() as u64
    }
}

/// Test-only allocation counting over the generic [`Region`] — an extension trait for the same
/// reason as [`KoanRegionExt`].
#[cfg(test)]
pub(crate) trait KoanRegionTestExt {
    /// Total number of values stored across the three typed sub-arenas. Each typed `alloc_*` writes
    /// to exactly one of them, so this is the precise count without double-counting. It says nothing
    /// about the `Drop`-free families: those live in the bump, which counts bytes
    /// ([`Region::bump_bytes`]) rather than values.
    fn alloc_count(&self) -> usize;
}

#[cfg(test)]
impl KoanRegionTestExt for KoanRegion {
    fn alloc_count(&self) -> usize {
        self.family_len::<KFunction<'static>>()
            + self.family_len::<Scope<'static>>()
            + self.family_len::<Module<'static>>()
    }
}

#[cfg(test)]
mod tests;
