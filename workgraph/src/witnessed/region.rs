//! Generic run-lifetime storage substrate. Routes its store-side lifetime-erasure through its
//! module's single audited [`erase_to_static`](super::erase_to_static) primitive — it names no
//! workload type. A [`StorageProfile`] injects its storage families via [`Stored`]; the single
//! private [`store`](Region::store) path erases each value to `'static` and writes it to the
//! family's sub-arena. One surface re-anchors that store:
//! [`alloc_resident`](Region::alloc_resident) hands it back at the caller's `'a` as a co-located
//! `&'a` (content == borrow == `'a`, the tight no-free-lifetime shape). It is `pub(crate)` — the
//! only public allocation surface is [`RegionHandle`], minted from a region owner or handed out at a
//! `for<'b>` brand by the library's construction combinators — so a bare `&Region` has no allocation
//! surface at all.
//!
//! Beside the typed cells a region holds a **bump** ([`bump`](Region::bump)), the storage home for
//! any `Drop`-free value family that names the region's own lifetime. It routes no erasure at all —
//! the allocator is lifetime-free, so `'a` enters only at the allocating call — which is what lets
//! a bumped value hold an `&'a` back into its own region with no residence check at all.
//! Every writer reaches it as a [`BumpAllocator`](super::BumpAllocator), which is where the bump
//! verbs are defined once: the library's own container metadata through the crate-private
//! [`allocator`](Region::allocator), an embedder through [`RegionHandle::allocator`] for bare bytes
//! or through the public bump door
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

use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Weak;

use bumpalo::Bump;
use elsa::FrozenMap;
use typed_arena::Arena;

use super::{
    erase_to_static, BumpAllocator, DropFree, Erased, FoldedPlacement, PinBundle, PinsRegion,
    ReachDescription, Reattachable, ReferenceFamily, RegionOwner, SealedExtern, StepCoverage,
    Witness,
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
/// enters the engine as `At<'a>`. The trait carries the family's one storage answer — which cell it
/// lands in — so [`store`](Region::store) reasons about the erase-store sequence once instead of
/// forking it per type.
///
/// Not sealed: this is the workload's extension point. Unbypassability comes from elsewhere — the
/// engine is the only path to the private [`Region::storage`], so an impl can supply policy
/// but cannot route a value past the single store engine.
pub trait Stored<W: StorageProfile>: Reattachable + Sized + 'static {
    /// Project this family's cell out of the library-owned storage bundle. This return type is the
    /// binding chokepoint: every cell has a distinct type, so only the matching tuple path
    /// type-checks — a wrong path is a compile error, not a runtime bug.
    fn cell(storage: &StorageOf<W>) -> &FamilyArena<Self>;
}

/// Run-lifetime allocation frame. Lives for one program run (or one per-call frame). Sub-arenas
/// store `K::At<'static>` (phantom); a surface re-anchors the store on the way out, to the caller's
/// `'a` ([`alloc_resident`](Self::alloc_resident)).
pub struct Region<W: StorageProfile> {
    /// The library-owned typed cell bundle, derived from the workload's family list. PRIVATE and
    /// never exposed by reference: the only path in is [`store`](Self::store), the sole store
    /// engine, so storage is never reachable by reference.
    storage: StorageOf<W>,
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
    /// own lifetime. Every writer reaches it as a [`BumpAllocator`] over this field — the library's
    /// own container metadata (a [`Sectioned`](super::Sectioned) container's run partition and cell
    /// index block) through the crate-private [`allocator`](Self::allocator), an embedder through
    /// whichever door hands it one: [`RegionHandle::allocator`] for bare bytes at the handle's own
    /// frame lifetime, [`FoldedPlacement::fold_and_bump`](super::FoldedPlacement::fold_and_bump)
    /// when the bytes belong to a value whose operands' reach the door must compose. Bumped rather
    /// than arena'd because the allocator itself is lifetime-free, so `'a` enters only at the
    /// allocating call — which is what lets a bumped value hold an `&'a` back into this same region
    /// with no erasure and no residence audit. A `typed_arena` cell cannot: its type would have to
    /// name `'a`, and [`Region`] has no lifetime parameter, which is why a [`ReachDescription`] is
    /// lifetime-free.
    ///
    /// A `Bump` runs **no destructor** for what it holds — it releases its chunks whole. That is the
    /// point: everything allocated here costs nothing at region teardown, which is what keeps a
    /// sectioned container `Copy` and `Drop`-free so a frame drop need not walk one. The `T: Copy`
    /// bound the allocator's value, slice and text verbs carry is what statically holds callers to
    /// it — a `Copy` type has no `Drop` to skip. ("`Drop`-free" itself has no expressible bound;
    /// `Copy` is the static proxy.) A **collection** built over the same allocator's raw
    /// [`Allocator`](allocator_api2::alloc::Allocator) seam is where that bound stops travelling
    /// with the bytes: a frozen key→value table, a churning binding table. Such a writer proves its
    /// elements glue-free with a `const { assert!(!needs_drop::<_>()) }` at the declaration site
    /// that names their type, which is the same proof the bound stood for.
    ///
    /// A cycle among bumped entries is harmless: everything here dies with the region, at once.
    /// Occupancy is one figure for the whole bump ([`bump_capacity`](Self::bump_capacity)) — there
    /// is no per-writer breakdown, because the copy-versus-pin decision reads a region's total
    /// against a candidate value's own copy size and never needs one.
    bump: Bump,
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
            reach_table: FrozenMap::new(),
            bump: Bump::new(),
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

    /// This region's bump as a [`BumpAllocator`] — the one write surface for its bytes, carrying
    /// both the `Copy`-guarded verbs and the raw allocator seam a **mutable** collection is built
    /// over. Crate-private: an embedder reaches the same allocator through
    /// [`RegionHandle::allocator`], which is the accessor that names a region it is authorized over.
    /// Bytes taken either way are priced by [`bump_capacity`](Self::bump_capacity), which is what
    /// keeps an off-verb allocation honest; the allocator's own doc states what a collection over
    /// the raw seam owes at its declaration site in place of the guard.
    pub(crate) fn allocator(&self) -> BumpAllocator<'_> {
        BumpAllocator::over(&self.bump)
    }

    /// The region's bump footprint, in **reserved chunk capacity** (`Bump::allocated_bytes`):
    /// padding and the newest chunk's unused tail included. Capacity rather than a live-byte tally
    /// because this figure prices the copy-versus-pin decision and a pin retains chunks whole —
    /// capacity *is* what a pin costs. Reading it off the allocator also means an allocation that
    /// reaches the bump without going through a door here (a collection built over the bump through
    /// `allocator-api2`) is priced like any other.
    ///
    /// It is a whole-region figure: there is no per-family or per-writer breakdown, because that
    /// decision never needs one (see the [`bump`](Self::bump) field). Monotonic, like the bump
    /// itself — nothing is freed before the region dies, so nothing is ever subtracted.
    pub fn bump_capacity(&self) -> usize {
        self.bump.allocated_bytes()
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

    /// The single store path for any family `K`: erase the live form to `'static` and write it to
    /// the family's cell. Hands back the stored `&K::At<'static>` for a surface to re-anchor.
    /// `storage` is private and this is the only path that reaches it, so every family allocation
    /// routes here.
    ///
    /// No cycle gate: a stored value holds no owning `Rc` back to a region (a closure / future /
    /// module is a bare borrow into its defining region, kept alive by its carrier's witness set), so
    /// storing it where requested can never form an allocation back-edge.
    fn store<K: Stored<W>>(&self, value: K::At<'_>) -> &K::At<'static> {
        K::cell(&self.storage)
            .arena()
            .alloc(erase_to_static::<K>(value))
    }

    /// The co-located resident allocation: store `value` — its input lifetime forgotten by
    /// [`store`](Self::store), so `value` is accepted at **any** lifetime (a caller relocating a
    /// longer-lived value hands it straight in) — then re-anchor the stored reference to the caller's
    /// `'a` through the single audited [`retype`](super::retype). The result is `&'a K::At<'a>`:
    /// **content == borrow == `'a`**, the tightest shape, with no free content lifetime a caller could
    /// widen past the pin. The `&'a self` borrow is what makes it sound — the region pins the pointee
    /// for the whole of `'a`, so the re-anchored reference cannot out-claim its backing.
    ///
    /// Reached through [`RegionHandle::alloc_resident`] — `pub(crate)` here so a bare `&Region` does
    /// not expose it.
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

    /// **This region's owner, named weakly** — the back-link an owner established at construction
    /// (`Rc::new_cyclic`), so a value resident here reads the frame that owns it off the region
    /// rather than carrying a copy of the same `Weak` in a field of its own.
    ///
    /// On the handle rather than on a bare `&Region`: a handle holder already has the region's full
    /// allocation capability, so handing it the owner grants nothing new — where a bare `&Region`
    /// gaining an upgradeable owner path would reopen the mint route [`Self::from_owner`] closes
    /// (upgrade the `Weak`, mint a handle off the owner).
    pub fn host(self) -> Weak<W::FrameOwner> {
        self.region.host()
    }

    /// **This handle's region's bump as a [`BumpAllocator`](super::BumpAllocator)** — the frame-lifetime
    /// bytes door, and the whole of it: the guarded `text` / `slice` / `value` verbs for what is
    /// written once and read thereafter, and the raw allocator seam for a collection that keeps
    /// mutating after it is built.
    ///
    /// Deliberately not the same door as
    /// [`fold_and_bump`](super::FoldedPlacement::fold_and_bump), which exists to compose its
    /// **operands'** reach into the product's carrier. Bare bytes have no operands and no reach to
    /// compose, so there is nothing for that machinery to do and nothing a call site could claim
    /// wrongly. What is left is an ordinary borrow: nothing written through the returned allocator
    /// can outlive the `&'a Region` this handle holds, which the borrow checker enforces with no
    /// audit and no `unsafe`. A value built *around* those bytes is gated at its own door —
    /// [`alloc_resident`](Self::alloc_resident)'s `'static` bound, or one of the two rank-2 brands
    /// ([`alloc_resident_born`](Self::alloc_resident_born),
    /// [`FoldedPlacement::alloc_resident_folded`](super::FoldedPlacement::alloc_resident_folded)) —
    /// none of which this door touches.
    ///
    /// The destructor obligation travels on the allocator, not here: the `Copy`-bounded verbs carry
    /// it statically, and a collection over the raw seam restates it at the declaration site naming
    /// its element type. [`BumpAllocator`](super::BumpAllocator) carries both arguments.
    ///
    /// ```
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, FixtureProfile};
    /// use workgraph::witnessed::RegionHandle;
    /// let cart = fresh_cart();
    /// let handle: RegionHandle<'_, FixtureProfile> = RegionHandle::from_owner(&*cart);
    /// let stored: &str = handle.allocator().text("hello");
    /// assert_eq!(stored, "hello");
    /// let run: &[u32] = handle.allocator().slice(&[1, 2, 3]);
    /// assert_eq!(run, &[1, 2, 3]);
    ///
    /// let mut table: hashbrown::HashMap<u32, u32, hashbrown::DefaultHashBuilder, _> =
    ///     hashbrown::HashMap::new_in(handle.allocator());
    /// table.insert(7, 11);
    /// assert_eq!(table.get(&7), Some(&11));
    /// ```
    pub fn allocator(self) -> BumpAllocator<'a> {
        self.region.allocator()
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

    /// Co-located resident allocation — see [`Region::alloc_resident`]. Move-in: `value` must carry
    /// no region borrow (`K::At<'static>`), so the store-side lifetime erasure never discards a
    /// borrow only the caller could vet. A value that legitimately borrows a region is built where
    /// it lands instead, through [`Self::alloc_resident_born`] or its crossing-operand sibling.
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

    /// **Build a region-borrowing value at a fresh brand and store it here in one act** — the
    /// fold-free *born* door, for a family whose every constructor borrows the region it lands in and
    /// so can never meet [`alloc_resident`](Self::alloc_resident)'s `'static` bound.
    ///
    /// `build` runs under a `for<'b>` quantifier and receives a [`FoldedPlacement`] over *this*
    /// handle's region re-anchored to `'b`. That is the whole residence proof, and it is a compile
    /// one: `'b` is universally quantified with no outlives relation to any enclosing lifetime, so the
    /// only `&'b Region<W>` (and hence the only `X<'b>` built over one) a closure body can name is the
    /// one derived from the placement handed in. A capture of an ambient `&'a Region` does not coerce
    /// and does not compile. The built value's region pointer is therefore the destination's by
    /// construction — the same no-outlives argument
    /// [`FoldedPlacement::alloc_resident_folded`] rests on, with no operands to compose.
    ///
    /// Returns the stored value co-located at this handle's own `'a` (**content == borrow == `'a`**),
    /// so a caller storing the result in a `'a`-lifetimed field needs no re-anchor of its own.
    ///
    /// A value that must embed an operand borrowed from *outside* the closure takes
    /// [`alloc_resident_born_with`](Self::alloc_resident_born_with), which crosses that operand into
    /// `'b` through the witnessed channel rather than widening this signature.
    ///
    /// ```
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, HomedRef, HomedRefFamily};
    /// use workgraph::witnessed::RegionHandle;
    /// static SEED: u32 = 7;
    /// let cart = fresh_cart();
    /// let handle = RegionHandle::from_owner(&*cart);
    /// // The value names the placement's own region as its home, so it is resident by construction.
    /// let stored: &HomedRef<'_> = handle.alloc_resident_born::<HomedRefFamily>(|placement| {
    ///     HomedRef { home: placement.handle().region(), value: &SEED }
    /// });
    /// assert_eq!(*stored.value, 7);
    /// ```
    ///
    /// ```compile_fail
    /// // The residence proof, negatively: a value whose region pointer derives from an *ambient*
    /// // (non-brand) region cannot be returned from the closure — `elsewhere`'s `&Region` is at an
    /// // enclosing lifetime, which has no outlives relation to the universally quantified `'b`.
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, HomedRef, HomedRefFamily};
    /// use workgraph::witnessed::RegionHandle;
    /// static SEED: u32 = 7;
    /// let cart = fresh_cart();
    /// let elsewhere = fresh_cart();
    /// let handle = RegionHandle::from_owner(&*cart);
    /// let _ = handle.alloc_resident_born::<HomedRefFamily>(|_placement| {
    ///     HomedRef { home: &elsewhere.0, value: &SEED }
    /// });
    /// ```
    pub fn alloc_resident_born<K: Stored<W>>(
        self,
        build: impl for<'b> FnOnce(FoldedPlacement<'b, W>) -> K::At<'b>,
    ) -> &'a K::At<'a>
    where
        W: 'static,
    {
        // The handle is erased and re-anchored rather than captured: inside a `for<'b>` closure a
        // captured `RegionHandle<'a, W>` cannot coerce to `'b`, and the fresh `'b` is precisely what
        // makes an ambient capture a compile error. The witness is the region itself — the pointee the
        // erased handle names — borrowed for the whole of `'a`, so the re-anchor is a shortening.
        let handle = SealedExtern::<RegionHandleFamily<W>>::erase(self);
        handle.open(self.region, |handle_b| {
            let value = build(FoldedPlacement::mint(handle_b));
            // Stored through `self`'s own `&'a Region`, not `handle_b`'s: the result must come out at
            // `'a`, and a `&'b K::At<'b>` could not leave the closure at all.
            self.region.alloc_resident::<K>(value)
        })
    }

    /// [`alloc_resident_born`](Self::alloc_resident_born) **with a crossing operand** — the born door
    /// for a value built *from* a reference the caller already holds, where the surrounding
    /// construction lives at an enclosing lifetime the closure's `'b` cannot see.
    ///
    /// `operand` is re-anchored to the *same* `'b` as the placement, so a value embedding it is well
    /// typed at the brand even for an invariant family (branding the two at independent `'b`s is what
    /// invariance rejects; one [`zip`](SealedExtern::zip)ped open unifies them). Everything the
    /// closure needs at `'b` arrives this way or from the placement — that is what keeps the
    /// destination-residence proof of the operand-free door intact for every region pointer the value
    /// derives from the placement.
    ///
    /// # The operand's own liveness
    ///
    /// What the signature cannot prove is the *operand's*: its pointee may live in another region
    /// entirely, and the stored value keeps naming it for as long as this handle's region lives.
    /// `pin` is where the caller discharges that — it is borrowed for `'a`, the destination region's
    /// own lifetime, so the [`Witness`] contract (holding it keeps the backing live and fixed-address)
    /// covers the stored reference's whole life rather than merely the call. Passing a pin whose
    /// liveness does not in fact cover the operand is the same co-location obligation every
    /// [`Witness`] carries, and the same one [`SealedExtern::open`] states; this door narrows it to a
    /// duration the type system checks.
    ///
    /// ```
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, seal_extern, HomedRef, HomedRefFamily, RefFamily};
    /// use workgraph::witnessed::RegionHandle;
    /// let source = fresh_cart();
    /// let cart = fresh_cart();
    /// // A value resident in another region, which the built value will embed.
    /// let borrowed: &u32 = RegionHandle::from_owner(&*source).alloc_resident::<RefFamily>(&7);
    /// let handle = RegionHandle::from_owner(&*cart);
    /// // `source` is held for the whole of the destination's life, so it pins what the operand names.
    /// let stored = handle.alloc_resident_born_with::<HomedRefFamily, RefFamily, _>(
    ///     seal_extern::<RefFamily>(borrowed),
    ///     &source,
    ///     |placement, operand| HomedRef { home: placement.handle().region(), value: operand },
    /// );
    /// assert_eq!(*stored.value, 7);
    /// ```
    pub fn alloc_resident_born_with<K: Stored<W>, Op: Reattachable + DropFree, P: Witness>(
        self,
        operand: SealedExtern<Op>,
        pin: &'a P,
        build: impl for<'b> FnOnce(FoldedPlacement<'b, W>, Op::At<'b>) -> K::At<'b>,
    ) -> &'a K::At<'a>
    where
        W: 'static,
    {
        let handle = SealedExtern::<RegionHandleFamily<W>>::erase(self);
        handle.zip(operand).open(pin, |(handle_b, operand_b)| {
            let value = build(FoldedPlacement::mint(handle_b), operand_b);
            self.region.alloc_resident::<K>(value)
        })
    }

    /// **Build a region-borrowing value from a crossing operand and bump it here** — the born door
    /// for a `Drop`-free family, and the bump's answer to
    /// [`alloc_resident_born_with`](Self::alloc_resident_born_with). Same brand, same operand
    /// crossing, same pin obligation; what drops out is the store-side erasure, because the bump is
    /// lifetime-free and the value lands at the brand it was built at.
    ///
    /// `build` runs under a `for<'b>` quantifier and receives a [`FoldedPlacement`] over *this*
    /// handle's region plus `operand` re-anchored to that same `'b` — one [`zip`](SealedExtern::zip)ped
    /// open, so an invariant family is well typed at the brand. That quantifier is the residence
    /// proof and it is a compile one: `'b` has no outlives relation to any enclosing lifetime, so
    /// the only `&'b Region<W>` a closure body can name is the placement's. `pin` discharges the
    /// operand's own liveness exactly as it does at the typed door: borrowed for `'a`, the
    /// destination region's lifetime, so it covers the stored reference's whole life rather than
    /// merely the call.
    ///
    /// The `const` assert is the family's side of the bargain, restated at the door the acceptance
    /// runs through: a bump-hosted family runs **no destructor**, so it must have none to run. It
    /// monomorphizes per family, so a field that later grows a `Drop` is a build error here.
    ///
    /// There is no operand-free twin. A value with nothing foreign to embed needs no brand at all:
    /// its fields are already at the caller's `'a`, so it is built there and bumped through
    /// [`allocator().in_place`](super::BumpAllocator::in_place) directly.
    ///
    /// ```
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, seal_extern, HomedRef, HomedRefFamily, RefFamily};
    /// use workgraph::witnessed::RegionHandle;
    /// let source = fresh_cart();
    /// let cart = fresh_cart();
    /// // A value resident in another region, which the built value will embed.
    /// let borrowed: &u32 = RegionHandle::from_owner(&*source).allocator().value(7u32);
    /// let handle = RegionHandle::from_owner(&*cart);
    /// // `source` is held for the whole of the destination's life, so it pins what the operand names.
    /// let stored: &HomedRef<'_> = handle.bump_born_with::<HomedRefFamily, RefFamily, _>(
    ///     seal_extern::<RefFamily>(borrowed),
    ///     &source,
    ///     |placement, operand| HomedRef { home: placement.handle().region(), value: operand },
    /// );
    /// assert_eq!(*stored.value, 7);
    /// ```
    ///
    /// ```compile_fail
    /// // The residence proof, negatively: a value whose region pointer derives from an *ambient*
    /// // region cannot be returned from the closure — `elsewhere`'s `&Region` is at an enclosing
    /// // lifetime, which has no outlives relation to the universally quantified `'b`.
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, seal_extern, HomedRef, HomedRefFamily, RefFamily};
    /// use workgraph::witnessed::RegionHandle;
    /// let source = fresh_cart();
    /// let cart = fresh_cart();
    /// let elsewhere = fresh_cart();
    /// let borrowed: &u32 = RegionHandle::from_owner(&*source).allocator().value(7u32);
    /// let handle = RegionHandle::from_owner(&*cart);
    /// let _ = handle.bump_born_with::<HomedRefFamily, RefFamily, _>(
    ///     seal_extern::<RefFamily>(borrowed),
    ///     &source,
    ///     |_placement, operand| HomedRef { home: &elsewhere.0, value: operand },
    /// );
    /// ```
    ///
    /// ```compile_fail
    /// // Nothing built at the brand leaves it except the door's own product: a branded allocation
    /// // assigned to an enclosing binding is rejected by `for<'b>`.
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, seal_extern, HomedRef, HomedRefFamily, RefFamily};
    /// use workgraph::witnessed::RegionHandle;
    /// let source = fresh_cart();
    /// let cart = fresh_cart();
    /// let borrowed: &u32 = RegionHandle::from_owner(&*source).allocator().value(7u32);
    /// let handle = RegionHandle::from_owner(&*cart);
    /// let mut escaped: Option<&u32> = None;
    /// let _ = handle.bump_born_with::<HomedRefFamily, RefFamily, _>(
    ///     seal_extern::<RefFamily>(borrowed),
    ///     &source,
    ///     |placement, operand| {
    ///         escaped = Some(placement.handle().allocator().value(1u32));
    ///         HomedRef { home: placement.handle().region(), value: operand }
    ///     },
    /// );
    /// println!("{}", escaped.unwrap());
    /// ```
    pub fn bump_born_with<K, Op, P>(
        self,
        operand: SealedExtern<Op>,
        pin: &'a P,
        build: impl for<'b> FnOnce(FoldedPlacement<'b, W>, Op::At<'b>) -> K::At<'b>,
    ) -> &'a K::At<'a>
    where
        K: Reattachable,
        Op: Reattachable + DropFree,
        P: Witness,
        W: 'static,
    {
        const {
            assert!(
                !std::mem::needs_drop::<K::At<'static>>(),
                "a bump-hosted family must carry no drop glue: the bump runs no destructor",
            )
        };
        let handle = SealedExtern::<RegionHandleFamily<W>>::erase(self);
        // The *reference* the bump hands back is what leaves the brand: a `&'b K::At<'b>` cannot be
        // named outside the closure, so it is erased on the way out and re-anchored below. The value
        // itself never is — it is built at `'b` and stored at `'b`, which is the whole point of
        // bumping rather than storing to a `'static`-slotted cell.
        let stored: Erased<ReferenceFamily<K>> =
            handle.zip(operand).open(pin, |(handle_b, operand_b)| {
                let placement = FoldedPlacement::mint(handle_b);
                let value = build(placement, operand_b);
                Erased::erase(placement.allocator().in_place(value))
            });
        // SAFETY: the pointee was bumped into `self.region` inside the closure above — the placement's
        // allocator is over this handle's own region — and the `&'a Region` this handle holds pins the
        // region, hence every chunk it has handed out, for the whole of `'a`. The re-anchor is
        // therefore a shortening onto live, fixed-address backing: `'b` is the region's own storage
        // lifetime and `'a` is bounded by the handle's borrow of it. The result is `&'a K::At<'a>`
        // (content == borrow == `'a`), the same tight no-free-lifetime shape
        // [`Region::alloc_resident`] returns, so no caller can widen it past the pin.
        unsafe { stored.reattach() }
    }
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

/// A [`RegionHandle`] is a `Copy` thin pointer, so the handle family rests in the Copy tier.
impl<W> DropFree for RegionHandleFamily<W> {}
