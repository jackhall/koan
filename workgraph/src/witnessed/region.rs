//! Generic run-lifetime storage substrate. Holds an address membership side-table and routes its
//! store-side lifetime-erasure through its module's single audited
//! [`erase_to_static`](super::erase_to_static) primitive — it names no workload type. A
//! [`StorageProfile`] injects its storage families via [`Stored`]; the single private
//! [`store`](Region::store) path erases each value to `'static`, writes it to the family's sub-arena,
//! and records its address. Two surfaces re-anchor that store: the brand-confined
//! [`alloc`](Region::alloc) hands the freshly-stored value to a `for<'b>` closure (so it enters
//! circulation only wrapped by the Witnessed/Sealed abstraction, never as a bare region reference),
//! and [`alloc_resident`](Region::alloc_resident) re-anchors it to the caller's `'a` as a co-located
//! `&'a` (content == borrow == `'a`, the tight no-free-lifetime shape). Both are reachable only
//! through a [`RegionBrand`](crate::machine::core::RegionBrand) — a bare `&Region` has no allocation
//! surface at all. No cycle gate: a stored value holds no
//! owning `Rc` back to a region (a closure / future / module is a bare borrow into its defining
//! region, kept alive by its carrier's witness set), so storing it where requested can never form an
//! allocation back-edge. [`Region::storage`] is private and `store` is the only path that reaches it
//! — no `&Arena` ever escapes.
//!
//! The Koan instantiation (`KoanRegion = Region<KoanStorageProfile>`, the family `Stored` impls)
//! lives in [`crate::machine::core::arena`]. See
//! [memory-model.md § Arena lifetime erasure](../../design/memory-model.md#region-lifetime-erasure)
//! for the lifetime-erasure soundness argument and
//! [per-call-region/lifecycle.md § Escaping-value retention](../../design/per-call-region/lifecycle.md#escaping-value-retention)
//! for how an escaped value's region stays alive.

use std::cell::RefCell;

use typed_arena::Arena;

use super::{erase_to_static, with_branded_ref, Reattachable};

/// A workload's declaration of what a [`Region`] stores for it. `Storage` is the bundle of
/// typed sub-arenas the frame owns; the workload's [`Stored`] impls project each family out of it.
pub trait StorageProfile: Sized {
    type Storage: Default;
}

/// Per-family storage policy, implemented by the workload. The lifetime family itself comes from the
/// [`Reattachable`] supertrait — the same single-lifetime GAT (`At<'static> == Self`) the scheduler's
/// erase/reattach discipline routes — so the store-side erasure here and the read-side re-anchor in
/// the scheduler share one audited primitive instead of each carrying its own transmute. A live value
/// enters the engine as `At<'a>`. One trait carries every storage-safety answer for a family — which
/// sub-arena it lands in, whether it would self-cycle, and any post-store side effect — so
/// [`store`](Region::store) reasons about the gate-erase-store sequence once instead of forking it
/// per type.
///
/// Not sealed: this is the workload's extension point. Unbypassability comes from elsewhere — the
/// engine is the only path to the private [`Region::storage`], so an impl can supply policy
/// but cannot route a value past the single store engine.
pub trait Stored<W: StorageProfile>: Reattachable + Sized + 'static {
    /// Project this family's sub-arena out of the workload storage bundle. This return type is the
    /// binding chokepoint: storing `At<'static>` into `Arena<Self::At<'static>>` only type-checks
    /// when the family is wired to the matching sub-arena.
    fn sub_arena(storage: &W::Storage) -> &Arena<Self::At<'static>>;
    /// Post-store hook, run inside the engine on the storing frame. Default no-op; a family overrides
    /// it to record the stored address for [`Region::owns_addr`] membership queries.
    fn record_local(_frame: &Region<W>, _stored: &Self::At<'static>) {}
}

/// Run-lifetime allocation frame. Lives for one program run (or one per-call frame). Sub-arenas
/// store `K::At<'static>` (phantom); a surface re-anchors the store on the way out — to a `for<'b>`
/// brand ([`alloc`](Self::alloc)) or the caller's `'a` ([`alloc_resident`](Self::alloc_resident)).
pub struct Region<W: StorageProfile> {
    /// The workload's typed sub-arena bundle. PRIVATE and never exposed by reference: the only
    /// path in is [`store`](Self::store), the sole store engine, so storage is never reachable by
    /// reference.
    storage: W::Storage,
    /// Stable addresses of values a family opts to record (via [`Stored::record_local`]), backing
    /// [`owns_addr`](Self::owns_addr). `usize` rather than `*const _` keeps the field
    /// lifetime-erased and `Send`/`Sync`-neutral.
    membership: RefCell<Vec<usize>>,
}

impl<W: StorageProfile> Region<W> {
    pub fn new() -> Self {
        Self {
            storage: W::Storage::default(),
            membership: RefCell::new(Vec::new()),
        }
    }

    /// Number of values stored in family `K`'s sub-arena. Read-only; exposes no `&Arena`, so it
    /// cannot be used to bypass the gate.
    pub fn family_len<K: Stored<W>>(&self) -> usize {
        K::sub_arena(&self.storage).len()
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
    /// family's sub-arena, and fire [`Stored::record_local`] on the storing frame. Hands back the
    /// stored `&K::At<'static>` for a surface to re-anchor. `storage` is private and this is the only
    /// path that reaches it, so every allocation — branded or bare — routes here.
    ///
    /// No cycle gate: a stored value holds no owning `Rc` back to a region (a closure / future /
    /// module is a bare borrow into its defining region, kept alive by its carrier's witness set), so
    /// storing it where requested can never form an allocation back-edge.
    fn store<K: Stored<W>>(&self, value: K::At<'_>) -> &K::At<'static> {
        let stored = K::sub_arena(&self.storage).alloc(erase_to_static::<K>(value));
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
    /// rather than handed out as a bare region reference. The witnessed-allocation surface.
    ///
    /// Sound by the same `for<'b>` quantifier as [`Witnessed::with`](super::Witnessed::with): the
    /// region pins the pointee for the whole synchronous `project` call and the brand keeps the view
    /// from outliving it, so this surface carries **no `unsafe`** of its own beyond the substrate's
    /// single audited retype.
    pub fn alloc<K: Stored<W>, R>(
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
    /// Reachable only through an embedder's brand type (Koan's `RegionBrand`)'s `alloc_*` wrappers
    /// (the co-located residents: a registered `&KType`, a child `&Scope`); the witnessed terminals go
    /// through the brand-confined [`alloc`](Self::alloc). A bare `&Region` exposes neither.
    pub fn alloc_resident<'a, K: Stored<W>>(&'a self, value: K::At<'_>) -> &'a K::At<'a> {
        let stored: &'a K::At<'static> = self.store::<K>(value);
        // SAFETY: lifetime-only retype of a single-lifetime family (the `Reattachable` contract); a
        // reference is a thin/fat pointer whose layout is identical across the content lifetime. The
        // output is `&'a K::At<'a>` (content == borrow == `'a`), and the `&'a self` borrow keeps the
        // region — hence the pointee — live for all of `'a`, so the re-anchored reference cannot dangle
        // and, having no free content lifetime, cannot be widened past the pin.
        unsafe { super::retype::<&'a K::At<'static>, &'a K::At<'a>>(stored) }
    }
}

impl<W: StorageProfile> Default for Region<W> {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: a `Region`'s values live in a `typed_arena`, whose backing pages never move while the
// region is borrowed, so a held `&Region` keeps any pointee alloc'd in it (or a strict ancestor it
// roots) at a fixed address — the bound the consumer-pull lift's frameless re-anchor relies on to
// witness the destination lifetime.
unsafe impl<W: StorageProfile> super::Witness for Region<W> {}
