//! Generic run-lifetime storage substrate. A region's value storage is its **bump**
//! ([`bump`](Region::bump)) and nothing else: every family it hosts is `Drop`-free, so death is
//! chunk deallocation and no per-slot destructor runs at all. It names no workload type — a
//! [`StorageProfile`] declares only the frame-owner type its reach descriptions are typed at.
//!
//! The bump routes **no lifetime erasure**: the allocator is lifetime-free, so `'a` enters only at
//! the allocating call, which is what lets a bumped value hold an `&'a` back into its own region
//! with no residence check. Every writer reaches it as a [`BumpAllocator`](super::BumpAllocator),
//! which is where the bump verbs are defined once: the library's own container metadata through the
//! crate-private [`allocator`](Region::allocator), an embedder through [`RegionHandle::allocator`]
//! for bare bytes, through [`RegionHandle::bump_born_with`] for a value embedding a *foreign*
//! operand, or through the reach-composing door
//! ([`FoldedPlacement::fold_and_bump`](super::FoldedPlacement::fold_and_bump)).
//!
//! No cycle gate: a stored value holds no owning `Rc` back to a region (a closure / future / module
//! is a bare borrow into its defining region, kept alive by its carrier's witness set), so storing
//! it where requested can never form an allocation back-edge.
//!
//! The Koan instantiation (`KoanRegion = Region<KoanStorageProfile>`) lives in the embedder's arena
//! module (Koan's `machine::core::arena`). See
//! [memory-model.md § Region lifetime erasure](../../../design/memory-model.md#region-lifetime-erasure)
//! for the reference-side lifetime-erasure soundness argument and
//! [design/reach.md § Retention model](../../design/reach.md#retention-model)
//! for how an escaped value's region stays alive.

use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Weak;

use bumpalo::Bump;
use elsa::FrozenMap;

use super::{
    BumpAllocator, Carrier, DropFree, Erased, FoldedPlacement, PinBundle, PinsRegion,
    ReachDescription, Reattachable, ReferenceFamily, RegionOwner, SealedExtern, StepCoverage,
    Witness, Witnessed,
};

/// A workload's storage declaration: the frame-owner type a [`Region`]'s reach descriptions name.
/// Value storage needs no declaration at all — every family a region hosts is `Drop`-free and lives
/// in the region's bump, which is untyped.
pub trait StorageProfile: Sized {
    /// The workload's frame-owner type — the `PinsRegion` member a region's reach descriptions
    /// name. A [`Region`] interns its reach descriptions in a side table typed at this owner
    /// ([`Region::intern_reach_retained`]), separate from the bump, so the value bytes carry no
    /// `Drop`-bearing reach state.
    type FrameOwner: PinsRegion + 'static;
}

/// Run-lifetime allocation frame. Lives for one program run (or one per-call frame). Its values
/// live in the bump, born at the caller's own lifetime.
pub struct Region<W: StorageProfile> {
    /// The region's **reach intern table**: the non-owning reach descriptions minted for values
    /// living in this region ([`Region::intern_reach_retained`]), keyed on the canonical member set
    /// so one description exists per distinct reach per region. Separate from the family
    /// [`bump`](Self::bump) so a description is never value-page data (see
    /// [design/reach.md § The reach description](../../design/reach.md#the-reach-description)).
    /// A [`ReachDescription`] owns nothing — its members are `Weak`, so hosting it here pins no
    /// region; every value described here is resident here, so what keeps its members alive is
    /// [`retained_reach`](Self::retained_reach) below, folded by the same act that interned the
    /// entry. A holder in transit (the delivery envelope) carries pins of its own on top.
    ///
    /// `elsa::FrozenMap` is what makes get-or-mint expressible through `&self`: it inserts through a
    /// shared borrow and hands back a `&` to the boxed entry valid for the region's whole life — the
    /// same fixed-address guarantee the bump gives its own allocations, plus a
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
    /// with no erasure and no residence audit. A lifetime-*typed* cell could not: its type would
    /// have to name `'a`, and [`Region`] has no lifetime parameter, which is why a
    /// [`ReachDescription`] is lifetime-free.
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
    /// This touches no value storage and needs no
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
}

// No `Default` impl: `Default` is a public trait, so implementing it here would hand every
// embedder back a public mint route (`Region::<W>::default()`) even with `new` sealed above —
// the raw-region constructor stays reachable only through `RegionHost::region`.

// SAFETY: a `Region`'s values live in its `Bump`, which never moves an allocated chunk — an
// allocation is served from a chunk's free space and a new chunk is linked in rather than the old
// one grown — so a held `&Region` keeps any pointee alloc'd in it (or a strict ancestor it roots) at
// a fixed address, the bound the consumer-pull lift's frameless re-anchor relies on to witness the
// destination lifetime.
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
/// // A bare `&Region` has no allocation surface: `allocator` is crate-private.
/// use workgraph::witnessed::doctest_fixture::fresh_cart;
/// let cart = fresh_cart();
/// let _ = cart.0.allocator().value(7u32);
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
/// use workgraph::witnessed::doctest_fixture::fresh_cart;
/// use workgraph::witnessed::RegionHandle;
/// let cart = fresh_cart();
/// let handle = RegionHandle::from_owner(&*cart);
/// let stored: &u32 = handle.allocator().value(7u32);
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
    /// audit and no `unsafe`. A value built *around* those bytes that embeds a **foreign** operand
    /// is gated at its own rank-2 brand ([`bump_born_with`](Self::bump_born_with),
    /// [`FoldedPlacement::fold_and_bump`](super::FoldedPlacement::fold_and_bump)); one whose fields
    /// are all at this handle's own `'a` needs no gate and takes
    /// [`in_place`](super::BumpAllocator::in_place) directly.
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

    /// **Bundle a value that already lives in this region** under the description minted for it —
    /// the born-witnessed door for a value the region hosts but did not allocate through a
    /// placement: a binding entry read back out, a structural resident an embedder holds an `&'a`
    /// to. The value is not stored here; it pre-exists, and this names its reach.
    ///
    /// It sits on the handle that [`mint_retained`](Self::mint_retained) sits on, so the residence
    /// the description is stamped with and the capability that seals under it are the *same*
    /// handle — there is no second door for a caller to bring a foreign description to. The value
    /// borrows for some `'v` outliving `'a`, this handle's own lifetime, so a borrow that does not
    /// live as long as the region handle cannot be sealed under it at all. `'v` is free rather than
    /// pinned to `'a` so an *invariant* family — one whose `At` cannot shrink — can still seal a
    /// value that outlives the handle, which is the safe direction.
    ///
    /// The witness is the reference-only [`Carrier`]: it names the reach without pinning it, so
    /// every read opens under an external pin — the active frame during the producing step, the
    /// delivery envelope's retention hold afterwards. A value whose reach is genuinely empty takes
    /// `mint_retained(&[])` for its description, the degenerate case of the same door.
    pub fn seal_reaching<'v: 'a, T: Reattachable + DropFree>(
        self,
        value: T::At<'v>,
        reach: &'a ReachDescription<W::FrameOwner>,
    ) -> Witnessed<T, Carrier<W::FrameOwner>> {
        Witnessed::from_erased(Erased::erase(value), Carrier::new(reach))
    }

    /// **Build a region-borrowing value from a crossing operand and bump it here** — the born door,
    /// for a value built *from* a reference the caller already holds where the surrounding
    /// construction lives at an enclosing lifetime the closure's `'b` cannot see.
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
    /// The `const` assert is the family's side of the bargain, restated at the door the value
    /// enters through: a bump-hosted family runs **no destructor**, so it must have none to run. It
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
        // — content == borrow == `'a`, the tight co-located shape with no free content lifetime, so
        // no caller can widen it past the pin.
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
