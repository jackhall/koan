//! Generic run-lifetime storage substrate. Holds an address membership side-table and routes its
//! store-side lifetime-erasure through its module's single audited
//! [`erase_to_static`](super::erase_to_static) primitive — it names no workload type. A
//! [`StorageProfile`] injects its storage families via [`Stored`]; the single private
//! [`store`](Region::store) path erases each value to `'static`, writes it to the family's sub-arena,
//! and records its address. Two surfaces re-anchor that store: the brand-confined
//! [`alloc`](Region::alloc) hands the freshly-stored value to a `for<'b>` closure (so it enters
//! circulation only wrapped by the Witnessed/Sealed abstraction, never as a bare region reference),
//! and [`alloc_resident`](Region::alloc_resident) re-anchors it to the caller's `'a` as a co-located
//! `&'a` (content == borrow == `'a`, the tight no-free-lifetime shape). Both are `pub(crate)` — the
//! only public allocation surface is [`RegionHandle`], minted from a region owner or handed out at a
//! `for<'b>` brand by the library's construction combinators — so a bare `&Region` has no allocation
//! surface at all. No cycle gate: a stored value holds no
//! owning `Rc` back to a region (a closure / future / module is a bare borrow into its defining
//! region, kept alive by its carrier's witness set), so storing it where requested can never form an
//! allocation back-edge. [`Region::storage`] is private and `store` is the only path that reaches it
//! — no `&Arena` ever escapes.
//!
//! The Koan instantiation (`KoanRegion = Region<KoanStorageProfile>`, the family `Stored` impls)
//! lives in the embedder's arena module (Koan's `machine::core::arena`). See
//! [memory-model.md § Arena lifetime erasure](../../../design/memory-model.md#region-lifetime-erasure)
//! for the lifetime-erasure soundness argument and
//! [per-call-region/lifecycle.md § Escaping-value retention](../../../design/per-call-region/lifecycle.md#escaping-value-retention)
//! for how an escaped value's region stays alive.

use std::cell::RefCell;
use std::marker::PhantomData;

use typed_arena::Arena;

use super::{erase_to_static, with_branded_ref, Reattachable, RegionOwner};

/// One family's typed sub-arena — the library-owned storage cell a `FamilyList` bundle is built
/// from. The inner arena is private to the crate: holding a `&FamilyArena` grants no allocation
/// surface of its own; the only path in is the engine's single [`Region::store`] path.
pub struct FamilyArena<K: Reattachable + 'static> {
    arena: Arena<K::At<'static>>,
}

impl<K: Reattachable + 'static> Default for FamilyArena<K> {
    fn default() -> Self {
        FamilyArena {
            arena: Arena::new(),
        }
    }
}

impl<K: Reattachable + 'static> FamilyArena<K> {
    /// Number of values stored in this cell. Read-only.
    pub fn len(&self) -> usize {
        self.arena.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(crate) fn arena(&self) -> &Arena<K::At<'static>> {
        &self.arena
    }
}

/// A cons-list of storage families — `(A, (B, (C, ())))` — from which the library derives the
/// arena bundle a [`Region`] owns: one [`FamilyArena`] cell per family, in list order. Sealed: the
/// trait itself must be `pub` to appear in [`StorageProfile::Families`]'s bound and [`StorageOf`]'s
/// projection, but the module boundary keeps it unnameable outside the crate.
mod family_list {
    use super::{FamilyArena, Reattachable};

    pub trait FamilyList {
        type Arenas: Default;
    }

    impl FamilyList for () {
        type Arenas = ();
    }

    impl<K: Reattachable + 'static, Rest: FamilyList> FamilyList for (K, Rest) {
        type Arenas = (FamilyArena<K>, Rest::Arenas);
    }
}
use family_list::FamilyList;

/// The arena bundle a profile's family list derives.
pub type StorageOf<W> = <<W as StorageProfile>::Families as FamilyList>::Arenas;

/// A workload's declaration of what a [`Region`] stores for it: a `FamilyList` cons-list of the
/// families it stores. The library derives the bundle of library-owned [`FamilyArena`] cells from
/// it; the workload's [`Stored`] impls project each family's cell out by tuple path.
pub trait StorageProfile: Sized {
    type Families: FamilyList;
}

/// Per-family storage policy, implemented by the workload. The lifetime family itself comes from the
/// [`Reattachable`] supertrait — the same single-lifetime GAT (`At<'static> == Self`) the scheduler's
/// erase/reattach discipline routes — so the store-side erasure here and the read-side re-anchor in
/// the scheduler share one audited primitive instead of each carrying its own transmute. A live value
/// enters the engine as `At<'a>`. One trait carries every storage-safety answer for a family — which
/// cell it lands in, whether it would self-cycle, and any post-store side effect — so
/// [`store`](Region::store) reasons about the gate-erase-store sequence once instead of forking it
/// per type.
///
/// Not sealed: this is the workload's extension point. Unbypassability comes from elsewhere — the
/// engine is the only path to the private [`Region::storage`], so an impl can supply policy
/// but cannot route a value past the single store engine.
pub trait Stored<W: StorageProfile>: Reattachable + Sized + 'static {
    /// Project this family's cell out of the library-owned storage bundle. This return type is the
    /// binding chokepoint: every cell has a distinct type, so only the matching tuple path
    /// type-checks — a wrong path is a compile error, not a runtime bug.
    fn cell(storage: &StorageOf<W>) -> &FamilyArena<Self>;
    /// Post-store hook, run inside the engine on the storing frame. Default no-op; a family overrides
    /// it to record the stored address for [`Region::owns_addr`] membership queries.
    fn record_local(_frame: &Region<W>, _stored: &Self::At<'static>) {}
}

/// Run-lifetime allocation frame. Lives for one program run (or one per-call frame). Sub-arenas
/// store `K::At<'static>` (phantom); a surface re-anchors the store on the way out — to a `for<'b>`
/// brand ([`alloc`](Self::alloc)) or the caller's `'a` ([`alloc_resident`](Self::alloc_resident)).
pub struct Region<W: StorageProfile> {
    /// The library-owned typed cell bundle, derived from the workload's family list. PRIVATE and
    /// never exposed by reference: the only path in is [`store`](Self::store), the sole store
    /// engine, so storage is never reachable by reference.
    storage: StorageOf<W>,
    /// Stable addresses of values a family opts to record (via [`Stored::record_local`]), backing
    /// [`owns_addr`](Self::owns_addr). `usize` rather than `*const _` keeps the field
    /// lifetime-erased and `Send`/`Sync`-neutral.
    membership: RefCell<Vec<usize>>,
}

impl<W: StorageProfile> Region<W> {
    /// The library's sole raw-region constructor — `pub(crate)` so an embedder can never mint a
    /// bare `Region` directly. The only mint point reachable from outside `workgraph` is
    /// [`RegionHost::region`](super::RegionHost::region), which calls this lazily on first access.
    pub(crate) fn new() -> Self {
        Self {
            storage: StorageOf::<W>::default(),
            membership: RefCell::new(Vec::new()),
        }
    }

    /// Number of values stored in family `K`'s cell. Read-only; exposes no `&Arena`, so it
    /// cannot be used to bypass the gate.
    pub fn family_len<K: Stored<W>>(&self) -> usize {
        K::cell(&self.storage).len()
    }

    /// Whether `addr` was recorded by a prior [`Stored::record_local`] on this frame.
    pub fn owns_addr(&self, addr: usize) -> bool {
        self.membership.borrow().contains(&addr)
    }

    /// Record `addr` into the membership side-table. Called by a family's
    /// [`Stored::record_local`]; the only writer.
    pub fn record_addr(&self, addr: usize) {
        self.membership.borrow_mut().push(addr);
    }

    /// The single store path for any family `K`: erase the live form to `'static`, write it to the
    /// family's cell, and fire [`Stored::record_local`] on the storing frame. Hands back the
    /// stored `&K::At<'static>` for a surface to re-anchor. `storage` is private and this is the only
    /// path that reaches it, so every allocation — branded or bare — routes here.
    ///
    /// No cycle gate: a stored value holds no owning `Rc` back to a region (a closure / future /
    /// module is a bare borrow into its defining region, kept alive by its carrier's witness set), so
    /// storing it where requested can never form an allocation back-edge.
    fn store<K: Stored<W>>(&self, value: K::At<'_>) -> &K::At<'static> {
        let stored = K::cell(&self.storage)
            .arena()
            .alloc(erase_to_static::<K>(value));
        // The post-store hook fires on the storing frame (this one — `store` writes where called),
        // so a recorded address tracks its true owner.
        K::record_local(self, stored);
        stored
    }

    /// Brand-confined allocation: store `value`, then hand the freshly-stored carrier to `project`
    /// behind a **rank-2** (`for<'b>`) brand through [`with_branded_ref`]. Nothing region-lifetime
    /// escapes — `project`'s `R` cannot name `'b` — so the value enters circulation only as whatever
    /// carrier `project` builds (a [`Witnessed`](super::Witnessed) bundle, a
    /// [`SealedExtern`](super::SealedExtern)), wrapped by the Witnessed/Sealed abstraction from birth
    /// rather than handed out as a bare region reference. The witnessed-allocation surface, reached
    /// through [`RegionHandle::alloc`] — `pub(crate)` here so a bare `&Region` cannot call it directly.
    ///
    /// Sound by the same `for<'b>` quantifier as [`Witnessed::with`](super::Witnessed::with): the
    /// region pins the pointee for the whole synchronous `project` call and the brand keeps the view
    /// from outliving it, so this surface carries **no `unsafe`** of its own beyond the substrate's
    /// single audited retype.
    pub(crate) fn alloc<K: Stored<W>, R>(
        &self,
        value: K::At<'_>,
        project: impl for<'b> FnOnce(&'b K::At<'b>) -> R,
    ) -> R {
        with_branded_ref::<K, R>(self.store::<K>(value), project)
    }

    /// The co-located resident allocation: store `value` — its input lifetime forgotten by
    /// [`store`](Self::store), so `value` is accepted at **any** lifetime (a caller relocating a
    /// longer-lived value hands it straight in) — then re-anchor the stored reference to the caller's
    /// `'a` through the single audited [`retype`](super::retype). The result is `&'a K::At<'a>`:
    /// **content == borrow == `'a`**, the tightest shape, with no free content lifetime a caller could
    /// widen past the pin. The `&'a self` borrow is what makes it sound — the region pins the pointee
    /// for the whole of `'a`, so the re-anchored reference cannot out-claim its backing.
    ///
    /// Reached through [`RegionHandle::alloc_resident`] — `pub(crate)` here so a bare `&Region`
    /// exposes neither this nor the brand-confined [`alloc`](Self::alloc).
    pub(crate) fn alloc_resident<'a, K: Stored<W>>(&'a self, value: K::At<'_>) -> &'a K::At<'a> {
        let stored: &'a K::At<'static> = self.store::<K>(value);
        // SAFETY: lifetime-only retype of a single-lifetime family (the `Reattachable` contract); a
        // reference is a thin/fat pointer whose layout is identical across the content lifetime. The
        // output is `&'a K::At<'a>` (content == borrow == `'a`), and the `&'a self` borrow keeps the
        // region — hence the pointee — live for all of `'a`, so the re-anchored reference cannot dangle
        // and, having no free content lifetime, cannot be widened past the pin.
        unsafe { super::retype::<&'a K::At<'static>, &'a K::At<'a>>(stored) }
    }
}

// No `Default` impl: `Default` is a public trait, so implementing it here would hand every
// embedder back a public mint route (`Region::<W>::default()`) even with `new` sealed above —
// the raw-region constructor stays reachable only through `RegionHost::region`.

// SAFETY: a `Region`'s values live in a `typed_arena`, whose backing pages never move while the
// region is borrowed, so a held `&Region` keeps any pointee alloc'd in it (or a strict ancestor it
// roots) at a fixed address — the bound the consumer-pull lift's frameless re-anchor relies on to
// witness the destination lifetime.
unsafe impl<W: StorageProfile> super::Witness for Region<W> {}

/// The at-will allocation capability for a [`Region`] — a `Copy` newtype over `&'a Region<W>` carrying
/// the only public allocation surface. A bare `&Region` cannot allocate (the engine's alloc methods
/// are crate-private) and safe embedder code cannot wrap one into a handle (the field and the
/// crate-internal constructor are private): a handle enters circulation only by [`Self::from_owner`]
/// — minting requires the region's *owner*, whose `RegionOwner` impl is an audited, `unsafe`-opt-in
/// declaration — or handed out at a `for<'b>` brand by the library's construction combinators
/// ([`Witnessed::yoke_handle`](super::Witnessed::yoke_handle), [`StepContext::alloc_handle`](super::StepContext::alloc_handle),
/// [`StepContext::alloc_with_handle`](super::StepContext::alloc_with_handle)).
///
/// ```compile_fail
/// // A bare `&Region` has no allocation surface: `alloc_resident` is crate-private.
/// use workgraph::witnessed::doctest_fixture::{fresh_region, RefFamily};
/// let region = fresh_region();
/// let _ = region.alloc_resident::<RefFamily>(&7);
/// ```
///
/// ```compile_fail
/// // Safe embedder code cannot wrap a bare `&Region` into the capability: the field and the raw
/// // constructor are crate-private.
/// use workgraph::witnessed::doctest_fixture::{fresh_region, FixtureProfile};
/// use workgraph::witnessed::RegionHandle;
/// let region = fresh_region();
/// let _: RegionHandle<'_, FixtureProfile> = RegionHandle::new(&region);
/// ```
///
/// ```
/// use std::rc::Rc;
/// use workgraph::witnessed::doctest_fixture::{fresh_region, RefFamily, RegionCart};
/// use workgraph::witnessed::RegionHandle;
/// let cart = Rc::new(RegionCart(fresh_region()));
/// let handle = RegionHandle::from_owner(&*cart);
/// let stored: &u32 = handle.alloc_resident::<RefFamily>(&7);
/// assert_eq!(*stored, 7);
/// ```
///
/// ```compile_fail
/// // The closure-gated move-in is gone: storage of a region-borrowing value is gated by the
/// // family's own declared audit, never by caller code.
/// use std::rc::Rc;
/// use workgraph::witnessed::doctest_fixture::{fresh_region, RegionCart, RefFamily};
/// use workgraph::witnessed::RegionHandle;
/// let cart = Rc::new(RegionCart(fresh_region()));
/// let handle = RegionHandle::from_owner(&*cart);
/// let local = 7u32;
/// let _ = handle.alloc_resident_audited::<RefFamily>(&local, |_, _| true);
/// ```
pub struct RegionHandle<'a, W: StorageProfile> {
    region: &'a Region<W>,
}

// Manual impls: a derive would bound `W: Clone` / `W: Copy`, which the reference field does not need.
impl<W: StorageProfile> Clone for RegionHandle<'_, W> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<W: StorageProfile> Copy for RegionHandle<'_, W> {}

impl<'a, W: StorageProfile> RegionHandle<'a, W> {
    pub(crate) fn new(region: &'a Region<W>) -> Self {
        RegionHandle { region }
    }

    /// Mint the allocation capability from a region owner. The one public minter: it requires `&F`
    /// where `F: RegionOwner` — an owner type whose (unsafe-to-implement) contract pins the region —
    /// so an ambient bare `&Region` holder cannot mint.
    pub fn from_owner<F>(owner: &F) -> RegionHandle<'_, W>
    where
        F: RegionOwner<Region = Region<W>>,
    {
        RegionHandle {
            region: owner.region(),
        }
    }

    /// The bare region this handle authorizes — identity queries only.
    pub fn region(self) -> &'a Region<W> {
        self.region
    }

    /// Brand-confined allocation — see [`Region::alloc`]'s (crate-private) docs. Move-in: `value`
    /// must carry no region borrow (`K::At<'static>`) — `project` only views/wraps the
    /// freshly-stored value, it does not construct it, so a borrowing value would reach the arena
    /// unvetted.
    pub fn alloc<K: Stored<W>, R>(
        self,
        value: K::At<'static>,
        project: impl for<'b> FnOnce(&'b K::At<'b>) -> R,
    ) -> R {
        self.region.alloc::<K, R>(value, project)
    }

    /// Co-located resident allocation — see [`Region::alloc_resident`]. Move-in: `value` must carry
    /// no region borrow (`K::At<'static>`), so the store-side lifetime erasure never discards a
    /// borrow only the caller could vet. A value that legitimately borrows a region takes
    /// [`Self::alloc_resident_checked`] instead.
    ///
    /// ```
    /// use std::rc::Rc;
    /// use workgraph::witnessed::doctest_fixture::{fresh_region, RegionCart, RefFamily};
    /// use workgraph::witnessed::RegionHandle;
    /// let cart = Rc::new(RegionCart(fresh_region()));
    /// let handle = RegionHandle::from_owner(&*cart);
    /// // A `'static` value — here a promoted literal reference — is accepted.
    /// let stored: &u32 = handle.alloc_resident::<RefFamily>(&7);
    /// assert_eq!(*stored, 7);
    /// ```
    ///
    /// ```compile_fail
    /// // A region-borrowing value is rejected: `local`'s borrow is not `'static`, so it cannot
    /// // satisfy `alloc_resident`'s `K::At<'static>` bound.
    /// use std::rc::Rc;
    /// use workgraph::witnessed::doctest_fixture::{fresh_region, RegionCart, RefFamily};
    /// use workgraph::witnessed::RegionHandle;
    /// let cart = Rc::new(RegionCart(fresh_region()));
    /// let handle = RegionHandle::from_owner(&*cart);
    /// let local = 7u32;
    /// let _: &u32 = handle.alloc_resident::<RefFamily>(&local);
    /// ```
    pub fn alloc_resident<K: Stored<W>>(self, value: K::At<'static>) -> &'a K::At<'a> {
        self.region.alloc_resident::<K>(value)
    }

    /// Resident move-in vetted by family `K`'s own declared [`AuditedStored`] audit rather than a
    /// call-site closure: `value` is stored only when `K::audit` — the embedder's residence
    /// verifier for the family — accepts it against this handle's region and the typed `context`.
    /// Where [`Self::alloc_resident`] admits only `'static` values, this admits a value that
    /// legitimately borrows a region, with the family (an `unsafe impl`, not forgeable call-site
    /// code) declaring the vetting.
    ///
    /// ```
    /// use std::rc::Rc;
    /// use workgraph::witnessed::doctest_fixture::{fresh_region, RecordedRefFamily, RegionCart};
    /// use workgraph::witnessed::RegionHandle;
    /// static SEED: u32 = 7;
    /// let cart = Rc::new(RegionCart(fresh_region()));
    /// let handle = RegionHandle::from_owner(&*cart);
    /// // Seed the region so it records `SEED`'s address as resident.
    /// let _ = handle.alloc_resident::<RecordedRefFamily>(&SEED);
    /// // A borrow of the now-resident `SEED` passes the family audit.
    /// let stored = handle
    ///     .alloc_resident_checked::<RecordedRefFamily>(&SEED, ())
    ///     .expect("SEED is resident");
    /// assert_eq!(**stored, 7);
    /// ```
    ///
    /// ```
    /// use std::rc::Rc;
    /// use workgraph::witnessed::doctest_fixture::{fresh_region, RecordedRefFamily, RegionCart};
    /// use workgraph::witnessed::RegionHandle;
    /// static OTHER: u32 = 9;
    /// let cart = Rc::new(RegionCart(fresh_region()));
    /// let handle = RegionHandle::from_owner(&*cart);
    /// // `OTHER` was never stored, so the region does not own its address: the audit rejects it.
    /// assert!(handle
    ///     .alloc_resident_checked::<RecordedRefFamily>(&OTHER, ())
    ///     .is_none());
    /// ```
    pub fn alloc_resident_checked<K: AuditedStored<W>>(
        self,
        value: K::At<'_>,
        context: K::AuditContext<'_>,
    ) -> Option<&'a K::At<'a>> {
        K::audit(self.region, &value, context).then(|| self.region.alloc_resident::<K>(value))
    }
}

/// A per-family residence audit an embedder declares once, consumed by
/// [`RegionHandle::alloc_resident_checked`] to gate a region-borrowing move-in. Where
/// [`RegionHandle::alloc_resident`] admits only `'static` values and the crate-private
/// brand-confined doors build in place, this is the door for a value the embedder can vet only at
/// runtime — but the audit is a **family declaration**, not a forgeable call-site closure, so a
/// permissive audit is not writable in safe code. Each call site passes typed `context`
/// (residence evidence), never code.
///
/// # Safety
///
/// An implementor's [`audit`](Self::audit) must return `true` only when every region borrow the
/// stored `value` carries is resident in `region` or covered by `context`'s evidence — the same
/// obligation the caller of [`Region::alloc_resident`] otherwise discharges by construction. A
/// lying audit (one that returns `true` for a value borrowing a region that `region` neither owns
/// nor `context` covers) re-admits an unvetted lifetime-lengthening move-in, exactly the dangle the
/// `'static` bound on [`RegionHandle::alloc_resident`] rules out. `unsafe` to implement for that
/// reason, following the [`RegionOwner`] / [`Reattachable`] precedent — the impl is an audited
/// soundness declaration.
pub unsafe trait AuditedStored<W: StorageProfile>: Stored<W> {
    /// The typed evidence a call site passes — never code. `()` for a family whose audit is a
    /// self-contained residence check; a richer context (reach evidence, an ambient predicate) for
    /// a family whose audit widens against the destination's coverage.
    type AuditContext<'ctx>;
    /// Vet `value` for residence in `region` under `context`. Returns `true` only when the store is
    /// sound per the trait's safety contract.
    fn audit(region: &Region<W>, value: &Self::At<'_>, context: Self::AuditContext<'_>) -> bool;
}

/// [`Reattachable`] family for a [`RegionHandle`] — a thin pointer, layout independent of `'r` — so an
/// embedder can erase/re-anchor the capability through the witnessed substrate (the per-call
/// construction door).
pub struct RegionHandleFamily<W>(PhantomData<W>);

// SAFETY: `RegionHandle<'r, W>` is a newtype over `&'r Region<W>`, a thin pointer whose layout is
// identical for every choice of `'r`.
unsafe impl<W: StorageProfile + 'static> Reattachable for RegionHandleFamily<W> {
    type At<'r> = RegionHandle<'r, W>;
}
