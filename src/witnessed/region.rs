//! Generic run-lifetime storage substrate. Holds an address membership side-table and routes its
//! store-side lifetime-erasure through its module's single audited
//! [`erase_to_static`](super::erase_to_static) primitive — it names no workload type. A
//! [`StorageProfile`] injects its storage families via [`Stored`]; the single private
//! [`store`](Region::store) path erases each value to `'static`, writes it to the family's sub-arena,
//! and records its address. Two surfaces re-anchor that store: the brand-confined
//! [`alloc`](Region::alloc) hands the freshly-stored value to a `for<'b>` closure (so it enters
//! circulation only wrapped by the Witnessed/Sealed abstraction, never as a bare region reference),
//! and the transitional [`alloc_resident`](Region::alloc_resident) re-anchors it to the caller's
//! lifetime as a bare `&'a` for the callers not yet witnessed. No cycle gate: a stored value holds no
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

use super::{erase_to_static, reattach_ref_with, with_branded_ref, Reattachable};

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
    pub(crate) fn family_len<K: Stored<W>>(&self) -> usize {
        K::sub_arena(&self.storage).len()
    }

    /// Whether `addr` was recorded by a prior [`Stored::record_local`] on this frame.
    pub(crate) fn owns_addr(&self, addr: usize) -> bool {
        self.membership.borrow().contains(&addr)
    }

    /// Record `addr` into the membership side-table. Called by a family's
    /// [`Stored::record_local`]; the only writer.
    pub(crate) fn record_addr(&self, addr: usize) {
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
    pub(crate) fn alloc<K: Stored<W>, R>(
        &self,
        value: K::At<'_>,
        project: impl for<'b> FnOnce(&'b K::At<'b>) -> R,
    ) -> R {
        with_branded_ref::<K, R>(self.store::<K>(value), project)
    }

    /// The transitional bare-`&'a` allocation: store `value` — its input lifetime forgotten by
    /// [`store`](Self::store) — then re-anchor the store to the caller's `'a` through the
    /// witness-bounded [`reattach_ref_with`], with `self` (the region the value now lives in) as the
    /// pin. Because the store erases the input, `value` is accepted at **any** lifetime, so a caller
    /// relocating a longer-lived value into this region hands it straight in rather than pre-shortening
    /// it to the region borrow. The surface the not-yet-witnessed callers route; the brand-confined
    /// [`alloc`](Self::alloc) is its witnessed replacement, and confining this leaf behind a branded
    /// region handle (so a bare `&Region` cannot reach it) is the access-verb item's close. Carries no
    /// `unsafe`: the result borrow is capped at `&'a self`, so no `'static`-claiming reference escapes
    /// the frame's own borrow.
    pub(crate) fn alloc_resident<'a, K: Stored<W>>(&'a self, value: K::At<'_>) -> &'a K::At<'a> {
        reattach_ref_with::<K, _>(self.store::<K>(value), self)
    }
}

impl<W: StorageProfile> Default for Region<W> {
    fn default() -> Self {
        Self::new()
    }
}
