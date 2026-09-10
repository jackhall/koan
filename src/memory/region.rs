//! The Koan instantiation of the generic [`Region`] storage substrate:
//! `KoanRegion = Region<KoanStorageProfile>`, the [`FrameStorage`] owner a per-call region hangs
//! off, and the Koan-typed `alloc_*` brands over the region's bump — [`RegionBrand`] at a frame
//! lifetime, [`FoldingBrand`] and [`SubstrateDoor`] inside a fold closure.
//!
//! The generic region engine lives in `workgraph`, reached through
//! [`substrate`](super::substrate); this file supplies the Koan policy it runs. The per-call frame
//! shell over a `FrameStorage` is [`frame`](super::frame); the program-text tier above the run root
//! is [`program`](super::program).
//!
//! See [per-call-region/README.md](../../design/per-call-region/README.md) for the carrier
//! set, escaping-value retention, ancestor chain, and TCO frame reuse;
//! [memory-model.md § Region lifetime erasure](../../design/memory-model.md#region-lifetime-erasure)
//! for the heap-pinning / drop-order invariants.

use std::hash::BuildHasher;
use std::rc::{Rc, Weak};

use super::frame::{FrameCoverage, FrameReach};
use super::substrate::{
    BumpAllocator, BumpBackedMap, Delivered, DropFree, FoldedPlacement, Reattachable, Region,
    RegionHandle, RegionHost, Retained, Sealed, StepContext, StorageProfile, Witnessed,
};

/// The Koan workload's storage declaration — the frame-owner type its reach descriptions name, and
/// nothing else.
///
/// **Every Koan value family is `Drop`-free, so every one lives in the region's bump**, where death
/// is chunk deallocation and no per-slot glue runs at all: a `KFunction` with its signature elements
/// a bumped run of `&str`, a `Module` with its path and member tables bump-hosted, and a
/// [`Scope`](crate::machine::core::Scope) with its binding tables built over the same allocator and its own destructor
/// structurally absent. See
/// [value-substrates.md § Untyped arenas](../../design/value-substrates.md#untyped-arenas-the-drop-free-end-state).
///
/// A [`TypeSymbol`](crate::machine::model::TypeSymbol) and a
/// [`KType`](crate::machine::model::KType) need no storage at
/// all: both are lifetime-free `Copy` handles — a name's hash digest and an interned registry
/// index — so the type channel's carriers hold them by value.
pub struct KoanStorageProfile;

impl StorageProfile for KoanStorageProfile {
    /// Reach descriptions live in the region's side table, typed at the per-call frame owner.
    type FrameOwner = FrameStorage;
}

/// Run-lifetime allocator. A [`Region`] carrying the Koan family set; lives for one program
/// run. The `KoanRegion` references across the tree and the `Rc<Frame<_>>` back-edge ride this
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
pub struct RegionBrand<'a>(pub(crate) RegionHandle<'a>);

impl<'a> RegionBrand<'a> {
    /// The bare region this brand authorizes — for identity compares (`ptr::eq`, `pins_region`). A
    /// bare `&KoanRegion` cannot be turned *back* into a brand — the library's [`RegionHandle`] enforces
    /// that — so handing out the identity reference opens no hole.
    pub fn region(self) -> &'a KoanRegion {
        self.0.region()
    }

    /// The bare library allocation capability this brand wraps — the handle-headed construction
    /// operand families (`RegionTypeFamily`, the aggregate build products, the destination-operand
    /// `RegionHandleFamily`) cross the brand as this raw handle rather than the koan veneer, so the
    /// library's own `HasRegionHandle` impls for `RegionHandle`/`(RegionHandle, T)` discharge their
    /// obligation with no koan-side impl. A closure that needs the koan-typed `alloc_*` veneer back
    /// rewraps locally: `RegionBrand(handle)`.
    pub(crate) fn handle(self) -> RegionHandle<'a> {
        self.0
    }

    /// The [`FrameStorage`] (a cloned `Weak`) that **owns this brand's region** — read off the
    /// region's own host back-link rather than a copy carried here. A region is born naming its
    /// owner (`Rc::new_cyclic`), so the derivation is total and no constructor can wire it wrong;
    /// the link stays `Weak` because the storage owns the region owns its residents, and an `Rc`
    /// back-edge would leak. Upgrades whenever the region is live.
    ///
    /// The residence facts a resident derives — its owner, its live frame, the pin a child frame
    /// chains — are all this brand's, which is why they live here and not on the lexical record
    /// that happens to hold one: a wrapper over the brand would carry no invariant the brand lacks.
    pub(crate) fn region_owner(self) -> Weak<FrameStorage> {
        self.0.host()
    }

    /// The `Rc<FrameStorage>` that owns this brand's region — the witness a value built into it is
    /// `yoke`d under (the object-family construction inversion: a region-resident object is born
    /// bundled with its frame as its reach). The link is [`Weak`](Self::region_owner) — an
    /// in-region value holds no owning `Rc` back to its frame — and upgrades for as long as a
    /// holder of the brand can run: a **producing** scope during its own step (the producing node
    /// holds the frame); a **consumer/current** scope during a step (the slot's cart — or a cart
    /// ancestor via the `FrameStorage.outer` chain, for a `YokedChild` overlay scope — is held by
    /// the step machinery for the whole step); or the **run root** (the run storage is held by the
    /// interpreter for the whole run). The single owner of this invariant's assertion;
    /// step-scoped callers should route through `DecideCtx::dest_frame` or a finish's
    /// `ctx.frame()` instead of upgrading directly.
    pub(crate) fn frame(self) -> Rc<FrameStorage> {
        self.region_owner()
            .upgrade()
            .expect("a live region brand implies a live region owner")
    }

    /// The storage pin a **child frame** chains when its own resident borrows into this brand's
    /// region ([`Frame::open_under`](super::frame::Frame::open_under)): this region's owning
    /// storage — or no pin when that owner is at the eternal tier
    /// ([`is_eternal`](RegionHost::is_eternal)), whose region outlives everything that could retain
    /// it and must not be strong-chained (a root chain plus an escaping value's reach-set pin is
    /// the region↔value `Rc` cycle the frame design excludes). The owner answers its own tier, so
    /// the two outcomes stay distinct: [`Self::frame`]'s `expect` reports a **dead owner**, which
    /// is a bug, while `None` reports the eternal-tier **policy**.
    pub(crate) fn parent_frame_pin(self) -> Option<Rc<FrameStorage>> {
        let owner = self.frame();
        (!owner.is_eternal()).then_some(owner)
    }

    /// Whether this brand's region sits at the **eternal tier** — the run root, whose region
    /// outlives every per-call frame. The same tier rule [`Self::parent_frame_pin`] declines to
    /// chain on, read from a brand instead of from an owner already in hand, so
    /// [`Frame::hosts`](super::frame::Frame::hosts) can answer `true` without consulting any pin
    /// chain and without upgrading a `Weak` of its own.
    pub(crate) fn is_eternal(self) -> bool {
        self.frame().is_eternal()
    }

    /// **This brand's region bump as a [`BumpAllocator`]** — the door every byte a value family slot
    /// holds is born through: a string's characters ([`KObject::KString`], a
    /// [`KKey::String`](crate::machine::model::KKey) dict key) through `text`, an expression's parts
    /// or a node's stored bucket key through `slice`, a node a part arm points at through `value`.
    /// The verbs and their `Copy` guard live on [`BumpAllocator`] itself, so this brand restates
    /// neither.
    ///
    /// What the brand adds is *which region*, and that is what a caller has to get right: the bytes
    /// land in this brand's region, so a value built around them is resident here and nowhere else.
    /// Storing that value is gated at its own door — [`Self::alloc_string`] re-homes a string's bytes
    /// itself, so its product is resident by construction; a string already living in another region
    /// takes [`FoldingBrand::alloc_object_folded`], where the rank-2 brand proves it was bumped at
    /// the destination. No address probe could stand in for either, because the bump keeps no address
    /// table and so cannot say which region a `&str` points into.
    ///
    /// A value's frozen keyed index takes [`BumpAllocator::frozen_table`], which carries the
    /// entry-glue proof itself. A table that keeps **mutating** — a scope's binding tables — is built
    /// over the same allocator's raw seam, which is where the `Copy` guard stops travelling with the
    /// bytes and the writer restates it with a `const` assert at the declaration naming its entry
    /// types ([`bump_table`]).
    pub(crate) fn allocator(self) -> BumpAllocator<'a> {
        self.0.allocator()
    }

    /// Bundle a value **already resident in this brand's region** whose borrows reach nothing — the
    /// terminal carrier a name / ATTR read hands back and a region-pure `LET` define site seals its
    /// object with. Unlike [`alloc_scalar_witnessed`](Self::alloc_scalar_witnessed) the value is not
    /// stored here; it pre-exists in the region. The description is minted **here**, so no caller
    /// pairs a value with a residence it did not derive: its host is this brand's own region owner
    /// and its members are empty, which is the exact claim for a value that reaches nothing beyond
    /// the region it lives in. The reading / defining frame pins that region for the step, and past
    /// the step the value is an ordinary resident of whichever destination regions the finalize
    /// delivery walk adopted it into — each of those regions' own owners carries the pin.
    ///
    /// A value that *does* reach somewhere takes [`Self::seal_reaching`] with the description
    /// [`Scope::mint_retained`](crate::machine::core::Scope) derived for it. The brand is the
    /// capability marker: only a handle into the region the value lives in may seal it resident.
    pub(crate) fn seal_resident<'v: 'a, T: Reattachable + DropFree>(
        self,
        value: T::At<'v>,
    ) -> Witnessed<T> {
        // A mint with no sources composes nothing, so the retained bundle is empty and the frozen
        // description names this region's owner as host and no member at all.
        self.seal_reaching(value, self.0.mint_retained(&[]))
    }

    /// [`Self::seal_resident`] for a value whose reach is already minted: bundle it under `reach`,
    /// the description a caller derived for this same value into this same region
    /// ([`Scope::mint_retained`](crate::machine::core::Scope)). The description carries the value's
    /// residence as its host, so the pairing this takes is one record, not two — there is no
    /// separate residence for a caller to get wrong.
    ///
    /// Forwards to the library door on the handle this brand wraps, which is the same handle the
    /// description was minted off: the value borrows for the frame lifetime `'a`, so a borrow that
    /// does not outlive the region cannot be sealed under it.
    pub(crate) fn seal_reaching<'v: 'a, T: Reattachable + DropFree>(
        self,
        value: T::At<'v>,
        reach: &'a FrameReach,
    ) -> Witnessed<T> {
        self.0.seal_reaching(value, reach)
    }

    /// [`Self::seal_resident`] handed out as a delivery envelope pinned by this region's own owner —
    /// the delivered twin, forwarded to the library door on the same handle. One door mints the
    /// description, seals the value under it and reads the home pin off the region, so nothing here
    /// pairs a value with a residence or a pin it did not derive.
    pub(crate) fn deliver_resident<'v: 'a, T: Reattachable + DropFree>(
        self,
        value: T::At<'v>,
    ) -> Delivered<T> {
        self.0.deliver_resident(value)
    }

    /// **Lift** a carrier resting in this brand's region into a delivery envelope pinned by the
    /// region's own owner (`Sealed → Delivered`): the library [`Delivered::lift`] upgrades the
    /// sealed description's members `Weak → Rc` under that pin, so the value's whole reach travels
    /// owned and the envelope survives its source frame's death.
    ///
    /// Keyed on the brand rather than on a scope because the region is what has to be right: the
    /// arena hosting the description is this brand's, so a caller holding the brand a seal came off
    /// cannot lift it against the wrong home. [`Scope::lift_resident`](crate::machine::core::Scope)
    /// is the scope-side spelling, and a borrowed binding façade — whose region is the opened
    /// module's, not its window scope's — reaches this door through its own brand.
    pub(crate) fn lift_resident<T: Reattachable + DropFree>(
        self,
        sealed: Sealed<T>,
    ) -> Delivered<T> {
        Delivered::lift(Retained::from_sealed(sealed), self.frame())
    }
}

/// The allocation capability inside a reach-folding closure: the enclosing combinator
/// (`transfer_into` / `merge_into` / `project` / [`StepAllocator::alloc_carried_with`](crate::machine::execute::step::StepAllocator::alloc_carried_with))
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
    placement: FoldedPlacement<'a>,
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
    pub(crate) fn in_fold_closure(placement: FoldedPlacement<'a>) -> Self {
        FoldingBrand {
            brand: RegionBrand(placement.handle()),
            placement,
        }
    }

    /// **Store a value built at this fold's own brand** — the one folded-residence door; every
    /// typed spelling in the value layer is a one-line call to it.
    ///
    /// Sound without a per-value audit, for one argument that does not vary with `T`: the input is
    /// typed at the brand lifetime, and inside a `for<'b>` fold closure the only inhabitants of a
    /// `'b`-parameterised type are values derived from the fold's declared operand views, the
    /// brand's own allocations, and owned/`'static` data — all named by the witness the enclosing
    /// combinator composes. An ambient-lifetime capture is a compile error at this signature (a
    /// `T<'ambient>` cannot coerce to `T<'b>`, since `'b` has no outlives relation to any enclosing
    /// lifetime), so the store's residence obligation is discharged at compile time by the placement
    /// capability, with no runtime audit at all. That is what turns "a function borrows only the
    /// scope it captures" from an asserted claim into the composition's own fact: the merge's source
    /// operand names the captured scope's region, and the closure can reach no other.
    ///
    /// The value lands in the destination's bump, through the fold placement's own
    /// [`allocator`](FoldedPlacement::allocator). `T: Copy` is the drop-free gate the bump requires —
    /// every Koan family that reaches this door is `Copy`, so region death frees the value as a bump
    /// chunk and runs no per-slot glue. The placement is what makes this a *residence* door rather
    /// than the untargeted [`RegionBrand::allocator`]: the brand's `'a` is the fold's own, so a value
    /// resident somewhere else cannot be written here.
    pub(crate) fn alloc_folded<T: Copy>(self, value: T) -> &'a T {
        self.placement.allocator().value(value)
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
/// description ([`CellReach::Pinned`](super::substrate::CellReach::Pinned)), and reading a
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

/// Koan's at-will allocation entry and identity queries over the generic [`Region`] — an extension
/// trait because `Region` lives in the `workgraph` crate and a foreign type takes no inherent impls.
/// Every co-located `alloc_*` lives on [`RegionBrand`] (minted via [`FrameStorage::brand`]); a bare
/// `&KoanRegion` keeps only the identity surface here.
pub(crate) trait KoanRegionExt {
    /// The alloc-witnessed construction inversion's region-pure primitive: build a value into
    /// `owner`'s region *inside* a **zero-dep fold**, returning it bundled with the [`FrameReach`]
    /// singleton pinning `owner` so it is co-located by construction rather than paired with an
    /// asserted witness. The closure receives a per-construction [`FoldingBrand`] confined to the
    /// `for<'b>` brand (it cannot escape the closure). A value that *references* another region's
    /// resident value folds that in with the envelope merge instead, unioning its reach; this
    /// primitive covers the case whose references are all region-derived or owned, so the `for<'b>`
    /// brand admits them.
    ///
    /// The fold brand rather than a bare [`RegionBrand`] because a region-pure leaf is no longer
    /// necessarily `'static`: a string literal's bytes are bumped into this same region
    /// ([`RegionBrand::allocator`]), so the value is region-self-referential and only
    /// [`alloc_folded`](FoldingBrand::alloc_folded)'s rank-2 argument admits it. With no deps the
    /// fold composes nothing, so the product's reach is exactly what it was: this region and no
    /// member.
    ///
    /// `build`'s return is spelled `T::At<'b>`, not a concrete payload type: the two are equal by the
    /// family's definition, but under the `for<'b>` binder the compiler does not normalize the
    /// projection lazily, so a `build` typed `-> Carried<'b>` fails to satisfy a `-> T::At<'b>`
    /// bound. Naming the projection makes the bounds syntactically identical. An inline closure
    /// returning the concrete type still unifies fine at the call site.
    // Drives the object-family construction inversion
    // (design/per-node-memory.md): a region-pure leaf builds its value inside this closure.
    fn fold_witnessed<T: Reattachable + DropFree>(
        owner: Rc<FrameStorage>,
        build: impl for<'b> FnOnce(FoldingBrand<'b>) -> T::At<'b>,
    ) -> Delivered<T>
    where
        for<'b> T::At<'b>: Copy;

    /// `yoke` a value of **any** carrier family into `owner`'s region, handing the build closure a
    /// per-construction [`RegionBrand`] confined to the `for<'b>` brand. Generalizes
    /// [`fold_witnessed`](Self::fold_witnessed) (the `CarriedFamily` case) for the operand yokes
    /// over a non-carried family (the construction operands' `RegionTypeFamily`,
    /// `OperatorGroupFamily`) whose closures alloc into the dest region. The yoke hands a
    /// `&'b KoanRegion`; wrapping it as the brand is sound for the same reason
    /// the yoke is — the `for<'b>` quantifier admits only region-derived/owned references, so
    /// co-location holds by construction and nothing branded escapes the closure.
    fn yoke_branded<T: Reattachable + DropFree, F>(
        owner: Rc<FrameStorage>,
        build: F,
    ) -> Delivered<T>
    where
        F: for<'b> FnOnce(RegionBrand<'b>) -> T::At<'b>;

    /// Total bytes allocated in this region: its **reserved bump capacity**
    /// ([`Region::bump_capacity`]), which is the whole of it — every Koan value lives in the bump,
    /// alongside the string bytes a value slot holds and the library's own sectioned-container
    /// metadata, and a pin retains those chunks whole. Prices the host region only, not the
    /// `outer` chain its `Rc<FrameStorage>` also retains (a documented approximation): the cost-copy
    /// seam reads this as the denominator of the payoff ratio, where the host's own footprint is the
    /// relevant scale. `#[allow(dead_code)]` because trait methods, unlike inherent ones, are checked
    /// per compilation target, and the plain `--lib` build (no `cfg(test)`) can't see its consumer.
    #[allow(dead_code)]
    fn allocated_total(&self) -> u64;
}

impl KoanRegionExt for KoanRegion {
    fn fold_witnessed<T: Reattachable + DropFree>(
        owner: Rc<FrameStorage>,
        build: impl for<'b> FnOnce(FoldingBrand<'b>) -> T::At<'b>,
    ) -> Delivered<T>
    where
        for<'b> T::At<'b>: Copy,
    {
        // A zero-dep fold: the engine composes no operand reach, so the envelope it hands back is
        // homed in `owner`'s own region and covers nothing beyond it — the same claim
        // `yoke_branded` makes, reached through the fold door instead. The dep family is `T` only
        // because the door names one; with an empty dep slice it constrains nothing.
        StepContext::new(owner).alloc_with::<KoanStorageProfile, T, T>(&[], |placement, _views| {
            build(FoldingBrand::in_fold_closure(placement))
        })
    }

    fn yoke_branded<T: Reattachable + DropFree, F>(
        owner: Rc<FrameStorage>,
        build: F,
    ) -> Delivered<T>
    where
        F: for<'b> FnOnce(RegionBrand<'b>) -> T::At<'b>,
    {
        // The library's born-delivered door over `owner`'s own region: the yoke brand proves the
        // built value is region-derived, and the envelope's home pin is that same region's owner —
        // one `Rc`, so the value is born under the pin it travels under. The product's reach is
        // exactly its own region, and its liveness is the envelope's until it rests or finalizes.
        // Turbofish `T`: inference does not drive it from the return type early enough to check
        // `build`'s `-> T::At<'b>` bound, so it sees `<_ as Reattachable>::At` and fails to match
        // the projection.
        RegionHandle::from_owner(&*owner).deliver_yoked::<T>(|handle| build(RegionBrand(handle)))
    }

    fn allocated_total(&self) -> u64 {
        self.bump_capacity() as u64
    }
}

/// Koan's per-call region owner: the library's [`RegionHost`], instantiated for the Koan family
/// set. `RegionHost` lazily mints its region on first allocation — reached by the resident
/// [`Frame::open_under`](super::frame::Frame::open_under) births immediately, so a constructed
/// frame's region is minted by the time anything reads it — and the `outer` link chains the
/// lexical-ancestor frames' storage alive. An escaping value (a returned closure, a module frame)
/// pins *this* — not the [`Frame`](super::frame::Frame) shell — so a tail hop's shell can
/// drop outright while the escapee's captured environment rides the old `FrameStorage` it still
/// holds.
/// The library's raw-region constructor is sealed to `workgraph`, so nothing outside the library
/// can mint a `KoanRegion` directly; the Koan-typed [`RegionBrand`] mint over a `FrameStorage` lives
/// on [`FrameStorageExt`] (an extension trait, since a type alias takes no inherent impls of its own).
pub type FrameStorage = RegionHost<KoanStorageProfile>;

/// The run-root storage: a fresh run region with no `outer` link, stamped at the eternal tier
/// ([`RegionHost::is_eternal`]) so anything holding it can tell the run region from a per-call one.
/// Held by `run_program` (and the test harness) so the run-root scope's region has an owning Rc;
/// [`Frame::adopting`](super::frame::Frame::adopting) derives it back off the run-root resident's
/// brand as the run frame's storage, and the run-root scope reads it as its region owner through
/// the region's own host back-link. Public so an integration test can stand one up: it mints nothing itself, only
/// building the library's `RegionHost` shell whose region lazily mints on first allocation.
pub fn run_root_storage() -> Rc<FrameStorage> {
    RegionHost::fresh_eternal()
}

/// Koan's [`RegionBrand`] mint over a [`FrameStorage`] — an extension trait because `FrameStorage`
/// is a `workgraph` type alias, so Koan cannot add an inherent method to it directly.
pub(crate) trait FrameStorageExt {
    /// Mint this storage's region's [`RegionBrand`] allocation capability. Minting is the library's
    /// [`RegionHandle::from_owner`] rule (it requires the storage that *owns* the region, via its
    /// `RegionOwner` impl); this method pairs it with the Koan veneer, and a bare `&KoanRegion`
    /// exposes no `alloc_*` of its own.
    fn brand(&self) -> RegionBrand<'_>;
}

impl FrameStorageExt for FrameStorage {
    fn brand(&self) -> RegionBrand<'_> {
        RegionBrand(RegionHandle::from_owner(self))
    }
}

/// Build one of a scope's tables over its region bump, **proving at compile time** that its entries
/// carry no drop glue. The bump runs no destructor, so a `Drop`-bearing key or value would silently
/// leak whatever it owns; the assert is monomorphization-checked, so a future entry field that
/// brings glue back is a build error at the declaration that admitted it rather than a leak.
///
/// This is where each table's storage choice is stated: all five of a scope's tables route here, so
/// none has an unstated exemption. It lives beside the brand rather than beside the tables because
/// it is the one construction that names the map implementation directly — the substrate rule keeps
/// that spelling inside `memory`.
pub(crate) fn bump_table<'a, K, V, S: BuildHasher + Default>(
    brand: RegionBrand<'a>,
) -> BumpBackedMap<'a, K, V, S> {
    const {
        assert!(
            !std::mem::needs_drop::<K>() && !std::mem::needs_drop::<V>(),
            "a bump-backed table's entries must carry no drop glue: the bump runs no destructor",
        )
    };
    hashbrown::HashMap::with_hasher_in(S::default(), brand.allocator())
}
