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
//! surface at all.
//!
//! Beside the typed cells a region holds a **bump** ([`bump`](Region::bump)), the storage home for
//! any `Drop`-free value family that names the region's own lifetime. It routes no erasure at all —
//! the allocator is lifetime-free, so `'a` enters only at the allocating call — which is what lets
//! a bumped value hold an `&'a` back into its own region with no `AuditedStored` residence audit.
//! The library bumps its own container metadata there ([`bump_slice`](Region::bump_slice) and its
//! siblings, all crate-private); an embedder reaches it only through the public bump door
//! ([`FoldedPlacement::fold_and_bump`](super::FoldedPlacement::fold_and_bump)), which composes the
//! stored value's reach in the same call.
//!
//! No cycle gate: a stored value holds no
//! owning `Rc` back to a region (a closure / future / module is a bare borrow into its defining
//! region, kept alive by its carrier's witness set), so storing it where requested can never form an
//! allocation back-edge. [`Region::storage`] is private and `store` is the only path that reaches it
//! — no `&Arena` ever escapes.
//!
//! The Koan instantiation (`KoanRegion = Region<KoanStorageProfile>`, the family `Stored` impls)
//! lives in the embedder's arena module (Koan's `machine::core::arena`). See
//! [memory-model.md § Arena lifetime erasure](../../../design/memory-model.md#region-lifetime-erasure)
//! for the lifetime-erasure soundness argument and
//! [design/reach.md § Retention model](../../design/reach.md#retention-model)
//! for how an escaped value's region stays alive.

use std::cell::{Cell, RefCell};
use std::marker::PhantomData;
use std::rc::Weak;

use bumpalo::Bump;
use elsa::FrozenMap;
use typed_arena::Arena;

use super::{
    erase_to_static, with_branded_ref, PinBundle, PinsRegion, ReachDescription, Reattachable,
    RegionOwner, StepCoverage,
};

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
    /// The workload's frame-owner type — the `PinsRegion` member a region's reach descriptions
    /// name. A [`Region`] interns its reach descriptions in a side table typed at this owner
    /// ([`Region::intern_reach_retained`]), separate from the [`Families`](Self::Families) arena bundle, so
    /// the value pages carry no `Drop`-bearing reach state.
    type FrameOwner: PinsRegion + 'static;
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
    /// The region's **reach intern table**: the non-owning reach descriptions minted for values
    /// living in this region ([`Region::intern_reach_retained`]), keyed on the canonical member set
    /// so one description exists per distinct reach per region. Separate from the family
    /// [`storage`](Self::storage) bundle so a description is never arena-page data (see
    /// [design/reach.md § The reach description](../../design/reach.md#the-reach-description)).
    /// A [`ReachDescription`] owns nothing — its members are `Weak`, so hosting it here pins no
    /// region; every value described here is resident here, so what keeps its members alive is
    /// [`retained_reach`](Self::retained_reach) below, folded by the same act that interned the
    /// entry. A holder in transit (the delivery envelope) carries pins of its own on top.
    ///
    /// `elsa::FrozenMap` is what makes get-or-mint expressible through `&self`: it inserts through a
    /// shared borrow and hands back a `&` to the boxed entry valid for the region's whole life — the
    /// same append-stable-address guarantee [`alloc_resident`](Self::alloc_resident) rests on, plus a
    /// read path an append-only arena has none of. The map owns the guarantee, so a carrier can share
    /// a thin reference into it with no `Drop`-order, dangling, or hand-audited-pointer hazard.
    reach_table: FrozenMap<Box<[usize]>, Box<ReachDescription<W::FrameOwner>>>,
    /// The region's **bump**: the storage home for every `Drop`-free value that names the region's
    /// own lifetime. Two kinds of writer reach it — the library's own container metadata (a
    /// [`Sectioned`](super::Sectioned) container's run partition and cell index block, through the
    /// crate-private [`bump_slice`](Self::bump_slice) family) and an embedder converting a
    /// `Drop`-free value family, which reaches it through one of the two public bump doors and never
    /// directly: [`FoldedPlacement::fold_and_bump`](super::FoldedPlacement::fold_and_bump) when the
    /// bytes belong to a value whose operands' reach the door must compose,
    /// [`RegionHandle::bump_text`] when they are bare bytes wanted at the handle's own frame
    /// lifetime. Bumped rather than arena'd because the allocator itself is lifetime-free, so `'a`
    /// enters only at the allocating call — which is what lets a bumped value hold an `&'a` back
    /// into this same region with no erasure and no residence audit. A `typed_arena` cell cannot:
    /// its type would have to name `'a`, and [`Region`] has no lifetime parameter, which is why a
    /// [`ReachDescription`] is lifetime-free.
    ///
    /// A `Bump` runs **no destructor** for what it holds — it releases its chunks whole. That is the
    /// point: everything allocated here costs nothing at region teardown, which is what keeps a
    /// sectioned container `Copy` and `Drop`-free so a frame drop need not walk one. The `T: Copy`
    /// bound every bump primitive carries is what statically holds callers to it — a `Copy` type has
    /// no `Drop` to skip. ("`Drop`-free" itself has no expressible bound; `Copy` is the static
    /// proxy.)
    ///
    /// A cycle among bumped entries is harmless: everything here dies with the region, at once.
    /// Occupancy is one figure for the whole bump ([`bump_bytes`](Self::bump_bytes)) — there is no
    /// per-writer breakdown, because the copy-versus-pin decision reads a region's total against a
    /// candidate value's own copy size and never needs one.
    bump: Bump,
    /// Live bytes bumped into [`bump`](Self::bump) so far, summed over every allocating call — the
    /// figure [`bump_bytes`](Self::bump_bytes) reports.
    ///
    /// Counted here rather than read off the allocator because `bumpalo` reports *chunk capacity*
    /// (`Bump::allocated_bytes` returns the total size of the chunks it has reserved, padding and
    /// the newest chunk's unused tail included). That would put a whole chunk's floor under the
    /// copy-versus-pin ratio this figure exists to serve, so a region holding twenty live bytes
    /// would price like one holding a full chunk.
    ///
    /// `Cell`, not `RefCell`: a `usize` counter needs no borrow tracking, and `Cell<usize>` is
    /// `Copy` and `Drop`-free, so it costs a region teardown nothing.
    bumped_bytes: Cell<usize>,
    /// The region's **union bundle**: one deduped [`PinBundle`] owning a pin for every region
    /// anything resident here reaches, retained for the region's whole life. It is the liveness home
    /// for a value **adopted** copy-free into this region ([`Region::retain_reach`], routed by
    /// [`Delivered::adopt_into`](super::Delivered::adopt_into)), whose re-anchored reference lives as
    /// long as the region and whose reach a non-owning description cannot pin.
    ///
    /// Every retention folds in through [`PinBundle::absorb`] rather than appending a bundle of its
    /// own, so the field stays an antichain of the deepest owners: one owning `Rc` per distinct
    /// region across everything resident here, dropped whole at region death. Region death and the
    /// scope's death are the same schedule — bindings are bind-once and a resident value never dies
    /// before its region — so a per-holder bundle would pin no shorter than this one does.
    ///
    /// A member declaring [`PinsRegion::needs_no_pin`] never enters (the **eternal rule**,
    /// [`PinBundle::without_eternal`]): its storage already outlives this region, so a pin on it is
    /// dead weight — and a live edge into a region that can retain this one back, which is a cycle
    /// neither side ever frees.
    retained_reach: RefCell<PinBundle<W::FrameOwner>>,
    /// The region's **owner** — the frame-owner value this region is a part of, named weakly. It is
    /// what [`ReachDescription::mint_resident`] stamps as a description's host, so a value's residence is
    /// read off the description that already lives in its home region's side table rather than
    /// carried a second time on the envelope.
    ///
    /// `Weak` because the owner owns this region: a strong link would be a self-cycle. The upgrade
    /// is infallible wherever it is reached — a live region *is* live storage inside its owner, so
    /// anything holding a pin that keeps this region alive holds the owner alive too.
    ///
    /// Supplied at construction (there is no un-owned region), so the back-link is established by
    /// the same act that creates the region: an owner builds itself through `Rc::new_cyclic` and
    /// hands its own `Weak` down.
    host: Weak<W::FrameOwner>,
}

impl<W: StorageProfile> Region<W> {
    /// The library's sole raw-region constructor — `pub(crate)` so an embedder can never mint a
    /// bare `Region` directly. The only mint point reachable from outside `workgraph` is
    /// [`RegionHost::region`](super::RegionHost::region), which calls this lazily on first access.
    /// `host` is the owner this region belongs to, named weakly; an owner sources it from
    /// `Rc::new_cyclic`, so a region cannot exist without naming the owner that holds it.
    pub(crate) fn new(host: Weak<W::FrameOwner>) -> Self {
        Self {
            storage: StorageOf::<W>::default(),
            membership: RefCell::new(Vec::new()),
            reach_table: FrozenMap::new(),
            bump: Bump::new(),
            bumped_bytes: Cell::new(0),
            retained_reach: RefCell::new(PinBundle::empty()),
            host,
        }
    }

    /// This region's owner, named weakly — the host [`ReachDescription::mint_resident`] stamps onto every
    /// description it freezes into this region's side table.
    pub(crate) fn host(&self) -> Weak<W::FrameOwner> {
        self.host.clone()
    }

    /// Get-or-mint the description of `composed`'s member set in this region's side table **and
    /// establish its retention here** — the sole reach-allocation path, reached through
    /// [`ReachDescription::mint_resident`]. Keyed on the canonical member set
    /// ([`PinBundle::intern_key`]): a miss builds the description, stores it, and folds `composed`
    /// (self rule then eternal rule) into this region's union bundle; a hit returns the existing
    /// entry and folds nothing, because an identical member set was already folded at that entry's
    /// own miss. One description exists per distinct reach per region, and interning is therefore
    /// itself the proof that this region pins what the entry names.
    ///
    /// Fusing the two is what makes the proof available: a mint is always a resident mint, so the
    /// only reason an entry exists is that some earlier mint retained its members here. Nothing
    /// records retention separately, and no caller can hold the composed bundle to fold by hand.
    ///
    /// Unlike [`alloc_resident`](Self::alloc_resident) this touches no family cell and needs no
    /// lifetime-retype: a [`ReachDescription`] is lifetime-free, and the map already returns a
    /// reference valid for the `&'a self` borrow (the region's life), so the description a carrier
    /// references outlives every read pinned by this region's owner. The description is non-owning
    /// (`Weak` members), so hosting it pins nothing — the members' liveness is the holder's
    /// [`PinBundle`], not this table. No `unsafe`: the append-stable guarantee is the map's, not a
    /// hand-audited pointer extension.
    ///
    /// The probe key is built in the caller's own frame ([`PinBundle::intern_key`], inline for the
    /// member counts that dominate) and boxed **only on the miss**, where the map takes ownership of
    /// it: a hit costs a hash and a compare, not an allocation.
    pub(crate) fn intern_reach_retained(
        &self,
        composed: PinBundle<W::FrameOwner>,
    ) -> &ReachDescription<W::FrameOwner>
    where
        W::FrameOwner: RegionOwner<Region = Region<W>>,
    {
        // The key comes off `composed` *before* the self-rule strip, so it matches the description's
        // membership rather than the retained bundle's — which is what orders the lines below.
        let key = composed.intern_key();
        if let Some(hit) = self.reach_table.get(&key[..]) {
            #[cfg(any(test, feature = "test-hooks"))]
            super::host::note_reach_intern_hit();
            return hit;
        }
        #[cfg(any(test, feature = "test-hooks"))]
        super::host::note_reach_interned();
        let description = self.reach_table.insert(
            key.into_boxed_slice(),
            Box::new(composed.describe(self.host())),
        );
        let mut retained = composed;
        retained.remove_region(self);
        self.retain_reach(retained);
        description
    }

    /// Copy `items` into the region's bump and hand back the co-located slice — the slice primitive
    /// of the bump family below, and the path a [`Sectioned`](super::Sectioned) container's runs and
    /// cell index block take.
    ///
    /// `T: Copy` is load-bearing, not a convenience: the bump never runs a destructor, so admitting
    /// a `Drop`-bearing `T` would silently skip it. `Copy` rules that out statically. No `unsafe`:
    /// the `&'a self` borrow is what keeps the chunk alive for the returned slice, exactly as it does
    /// for [`alloc_resident`](Self::alloc_resident), with no lifetime retype needed at all — the
    /// bump hands back `T` at whatever lifetime `T` already carried.
    pub(crate) fn bump_slice<'a, T: Copy>(&'a self, items: &[T]) -> &'a [T] {
        self.note_bumped(self.bump.alloc_slice_copy(items))
    }

    /// [`bump_slice`](Self::bump_slice) for a single `Copy` value — same bump, same `T: Copy`
    /// rationale, same absence of a lifetime retype. Hands back a **shared** `&'a T`, never the
    /// `&mut` the bump itself returns: a bumped value is region state a holder names, so no writer
    /// gets exclusive access to it after the allocating call.
    pub(crate) fn bump_value<T: Copy>(&self, value: T) -> &T {
        self.note_bumped(self.bump.alloc(value))
    }

    /// [`bump_slice`](Self::bump_slice) for text — a `str` is a `Copy`-element slice with a
    /// UTF-8 invariant, so it carries no destructor either and needs no bound to say so.
    pub(crate) fn bump_text<'a>(&'a self, text: &str) -> &'a str {
        self.note_bumped(self.bump.alloc_str(text))
    }

    /// Add `stored`'s size to the region's live-byte count and hand it straight back — the one
    /// writer of [`bumped_bytes`](Self::bumped_bytes). Every bump primitive returns through it, so
    /// counting is a wrapper on the return value rather than a separate step; nothing in the type
    /// system enforces that, so a new primitive has to be routed the same way. `size_of_val` reads
    /// the size off the value the bump actually produced (a slice's whole span, a `str`'s byte
    /// length), never a requested capacity.
    fn note_bumped<'a, T: ?Sized>(&self, stored: &'a T) -> &'a T {
        self.bumped_bytes
            .set(self.bumped_bytes.get() + size_of_val(stored));
        stored
    }

    /// The region's bump occupancy, in **total live bytes**: the sum of the sizes of the values
    /// bumped into it, which is the figure the copy-versus-pin decision weighs a candidate value's
    /// own copy size against. Not the allocator's reserved chunk capacity — see
    /// [`bumped_bytes`](Self::bumped_bytes) for why that would misprice the ratio.
    ///
    /// It is a whole-region figure: there is no per-family or per-writer breakdown, because that
    /// decision never needs one (see the [`bump`](Self::bump) field). Monotonic, like the bump
    /// itself — nothing is freed before the region dies, so nothing is ever subtracted.
    pub fn bump_bytes(&self) -> usize {
        self.bumped_bytes.get()
    }

    /// Fold an owning [`PinBundle`] into the region's union bundle, retained for the region's whole
    /// life — the liveness home for a value **adopted** copy-free into this region
    /// ([`Delivered::adopt_into`](super::Delivered::adopt_into)), whose re-anchored reference lives as
    /// long as the region. A non-owning description cannot pin the adopted value's reach, so the
    /// bundle it was minted with folds in here and is dropped only at region death.
    ///
    /// The fold is [`PinBundle::absorb`], so retaining the same region twice costs one `Rc` in total
    /// and a retention subsumed by an outer member costs none — the field stays a single antichain
    /// rather than a bundle per retention.
    pub(crate) fn retain_reach(&self, bundle: PinBundle<W::FrameOwner>) {
        #[cfg(any(test, feature = "test-hooks"))]
        super::host::note_reach_retention_fold();
        self.retained_reach
            .borrow_mut()
            .absorb(bundle.without_eternal());
    }

    /// Number of distinct owners in the region's union bundle — white-box retention introspection,
    /// gated behind `test-hooks` like the other reach white-box readers. Exposes a count, not the
    /// bundle, so it cannot be used to narrow a claim.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn retained_reach_len(&self) -> usize {
        self.retained_reach.borrow().members().len()
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
/// use workgraph::witnessed::doctest_fixture::{fresh_cart, RefFamily};
/// let cart = fresh_cart();
/// let _ = cart.0.alloc_resident::<RefFamily>(&7);
/// ```
///
/// ```compile_fail
/// // Safe embedder code cannot wrap a bare `&Region` into the capability: the field and the raw
/// // constructor are crate-private.
/// use workgraph::witnessed::doctest_fixture::{fresh_cart, FixtureProfile};
/// use workgraph::witnessed::RegionHandle;
/// let cart = fresh_cart();
/// let _: RegionHandle<'_, FixtureProfile> = RegionHandle::new(&cart.0);
/// ```
///
/// ```
/// use std::rc::Rc;
/// use workgraph::witnessed::doctest_fixture::{fresh_cart, RefFamily};
/// use workgraph::witnessed::RegionHandle;
/// let cart = fresh_cart();
/// let handle = RegionHandle::from_owner(&*cart);
/// let stored: &u32 = handle.alloc_resident::<RefFamily>(&7);
/// assert_eq!(*stored, 7);
/// ```
///
/// ```compile_fail
/// // The closure-gated move-in is gone: storage of a region-borrowing value is gated by the
/// // family's own declared audit, never by caller code.
/// use std::rc::Rc;
/// use workgraph::witnessed::doctest_fixture::{fresh_cart, RefFamily};
/// use workgraph::witnessed::RegionHandle;
/// let cart = fresh_cart();
/// let handle = RegionHandle::from_owner(&*cart);
/// let local = 7u32;
/// let _ = handle.alloc_resident_audited::<RefFamily>(&local, |_, _| true);
/// ```
///
/// ```compile_fail
/// // Folding reach into a region by hand is off the embedder surface: `retain_reach` is
/// // crate-private, so every retention an embedder can reach rides a mint that performs it.
/// use std::rc::Rc;
/// use workgraph::witnessed::doctest_fixture::fresh_cart;
/// use workgraph::witnessed::{RegionHandle, StepCoverage};
/// let cart = fresh_cart();
/// let handle = RegionHandle::from_owner(&*cart);
/// handle.retain_reach(StepCoverage::of(Rc::clone(&cart)));
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

    /// **Bump `text` into this handle's region** and hand back the co-located `&'a str` — the
    /// frame-lifetime bytes door, for a `Drop`-free byte run an embedder needs at the handle's own
    /// lifetime rather than confined to a fold closure.
    ///
    /// The sibling of [`BumpPlacement::text`](super::BumpPlacement::text), and deliberately not the
    /// same door. [`fold_and_bump`](super::FoldedPlacement::fold_and_bump) exists to compose its
    /// **operands'** reach into the product's carrier, which is why its placement is minted only
    /// inside that call; bare bytes have no operands and no reach to compose, so there is nothing for
    /// that machinery to do and nothing a call site could claim wrongly. What is left is an ordinary
    /// borrow: the returned `&'a str` cannot outlive the `&'a Region` this handle holds, which the
    /// borrow checker enforces with no audit and no `unsafe`. A value built *around* those bytes is
    /// gated where it always was — [`alloc_resident`](Self::alloc_resident)'s `'static` bound, the
    /// family audit on [`alloc_resident_checked`](Self::alloc_resident_checked), or the rank-2 brand
    /// on [`FoldedPlacement::alloc_resident_folded`](super::FoldedPlacement::alloc_resident_folded)
    /// — none of which this door touches.
    ///
    /// ```
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, FixtureProfile};
    /// use workgraph::witnessed::RegionHandle;
    /// let cart = fresh_cart();
    /// let handle: RegionHandle<'_, FixtureProfile> = RegionHandle::from_owner(&*cart);
    /// let stored: &str = handle.bump_text("hello");
    /// assert_eq!(stored, "hello");
    /// ```
    pub fn bump_text(self, text: &str) -> &'a str {
        self.region.bump_text(text)
    }

    /// **Bump a `Copy` slice into this handle's region** and hand back the co-located `&'a [T]` —
    /// the frame-lifetime run door, sibling of [`Self::bump_text`] and of
    /// [`BumpPlacement::slice`](super::BumpPlacement::slice). The shape an embedder's structural
    /// storage takes when a run of elements must live at the handle's own lifetime rather than
    /// confined to a fold closure: an expression's part list, a cached key.
    ///
    /// `T: Copy` carries the same "no destructor to skip" obligation as
    /// [`BumpPlacement::value`](super::BumpPlacement::value): the bump releases its chunks whole and
    /// runs no `Drop`. The reach argument is [`Self::bump_text`]'s verbatim — the elements are bytes
    /// this door merely relocates, and the returned borrow cannot outlive the `&'a Region` the
    /// handle holds, so there is no residence claim for a call site to get wrong. An element that
    /// itself borrows another region is gated where every stored value is: the family audits and the
    /// rank-2 fold brand, none of which this door touches.
    ///
    /// ```
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, FixtureProfile};
    /// use workgraph::witnessed::RegionHandle;
    /// let cart = fresh_cart();
    /// let handle: RegionHandle<'_, FixtureProfile> = RegionHandle::from_owner(&*cart);
    /// let stored: &[u32] = handle.bump_slice(&[1, 2, 3]);
    /// assert_eq!(stored, &[1, 2, 3]);
    /// ```
    pub fn bump_slice<T: Copy>(self, items: &[T]) -> &'a [T] {
        self.region.bump_slice(items)
    }

    /// **Bump one `Copy` value into this handle's region** and hand back the co-located `&'a T` —
    /// the single-value peer of [`Self::bump_slice`], for a stored node an embedder reaches by
    /// reference rather than inline. Same `T: Copy` obligation and same borrow argument.
    ///
    /// ```
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, FixtureProfile};
    /// use workgraph::witnessed::RegionHandle;
    /// let cart = fresh_cart();
    /// let handle: RegionHandle<'_, FixtureProfile> = RegionHandle::from_owner(&*cart);
    /// let stored: &u32 = handle.bump_value(7u32);
    /// assert_eq!(*stored, 7);
    /// ```
    pub fn bump_value<T: Copy>(self, value: T) -> &'a T {
        self.region.bump_value(value)
    }

    /// Fold an owned [`StepCoverage`] into this handle's region's union bundle, retained for the
    /// region's whole life — the library's own resting-cell fold onto [`Region::retain_reach`],
    /// backing [`Delivered::rest_in`](super::Delivered::rest_in): a value dropped to rest here keeps
    /// referencing the description its producer stamped, so nothing is minted and the coverage it
    /// arrives with is what must be pinned for as long as this region lives.
    ///
    /// **Crate-private.** An embedder supplies copy-or-pin verdicts and born-borrowing seeds and has
    /// no vocabulary for folding reach into a region: every embedder-reachable retention rides a
    /// mint ([`Self::mint_retained`]), which performs its own.
    ///
    /// The **self rule** applies as it does at [`Self::mint_retained`]: this region is stripped from
    /// the retained bundle, because a region owning a pin on itself is a cycle nothing ever breaks.
    /// It is what lets a caller hand over a whole coverage — a resting cell's claim, home included —
    /// without first asking whether that home happens to be this very region.
    pub(crate) fn retain_reach(self, coverage: StepCoverage<W::FrameOwner>)
    where
        W::FrameOwner: RegionOwner<Region = Region<W>>,
    {
        // The coverage arrives owned, so the strip is in place — no second buffer and no refcount
        // traffic, where `without_region` would clone the whole bundle to drop one member.
        let mut bundle = coverage.0;
        bundle.remove_region(self.region);
        self.region.retain_reach(bundle)
    }

    /// **Mint and retain in one verb** — the embedder-facing reach-derivation door. Freezes
    /// `sources`' composed reach into this region's side table, which is the same act that folds the
    /// owning bundle into this region's union bundle ([`ReachDescription::mint_resident`]): the
    /// description and the ownership that backs it are established together and there is no
    /// in-between state where an embedder could hold the pins.
    ///
    /// Returns the hosted description — stamped with this region's owner as its host, so the value
    /// it describes carries its own residence. No policy is threaded in: the mint applies
    /// subsumption and the self rule alone, so the description is the value's exact reach and the
    /// retained bundle is that reach minus this region itself (a region owning a pin on itself is a
    /// cycle).
    pub fn mint_retained(
        self,
        sources: &[&StepCoverage<W::FrameOwner>],
    ) -> &'a ReachDescription<W::FrameOwner>
    where
        W::FrameOwner: RegionOwner<Region = Region<W>>,
    {
        let bundles: Vec<&PinBundle<W::FrameOwner>> = sources.iter().map(|s| &s.0).collect();
        ReachDescription::mint_resident(self, &bundles)
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
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, RefFamily};
    /// use workgraph::witnessed::RegionHandle;
    /// let cart = fresh_cart();
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
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, RefFamily};
    /// use workgraph::witnessed::RegionHandle;
    /// let cart = fresh_cart();
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
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, RecordedRefFamily};
    /// use workgraph::witnessed::RegionHandle;
    /// static SEED: u32 = 7;
    /// let cart = fresh_cart();
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
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, RecordedRefFamily};
    /// use workgraph::witnessed::RegionHandle;
    /// static OTHER: u32 = 9;
    /// let cart = fresh_cart();
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
    /// self-contained residence check; a richer context (reach evidence naming what the value is
    /// allowed to borrow) for a family whose audit widens against the destination's coverage.
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
