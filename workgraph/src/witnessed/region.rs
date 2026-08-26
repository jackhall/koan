//! Generic run-lifetime storage substrate. A region's value storage is its **bump**
//! ([`bump`](Region::bump)) and nothing else: every family it hosts is `Drop`-free, so death is
//! chunk deallocation and no per-slot destructor runs at all. It names no workload type — a
//! [`StorageProfile`] declares only the frame-owner type its reach descriptions are typed at.
//!
//! The bump routes **no lifetime erasure**: the allocator is lifetime-free, so `'a` enters only at
//! the allocating call, which is what lets a bumped value hold an `&'a` back into its own region
//! with no residence check. Every writer reaches it as a [`BumpAllocator`](super::BumpAllocator) —
//! the crate-private [`allocator`](Region::allocator) for the library's own container metadata,
//! [`RegionHandle::allocator`] for an embedder's bare bytes, [`RegionHandle::bump_born_with`] for a
//! value embedding a *foreign* operand, or the reach-composing door
//! ([`FoldedPlacement::fold_and_bump`](super::FoldedPlacement::fold_and_bump)).
//!
//! No cycle gate on storage: a stored value holds no owning `Rc` back to a region, so storing it
//! where requested can never form an allocation back-edge. The *retention* graph is where a ring is
//! expressible — two regions' union bundles holding each other's owners — and the self and eternal
//! rules cut the two shapes that arise by construction. A debug-build detector reports whatever is
//! left, online at the fold that would close the ring ([`Region::retain_reach`]); it is diagnostic
//! and compiles out of a release build entirely
//! ([design/reach.md § Debug audits](../../design/reach.md#debug-audits)).
//!
//! The Koan instantiation (`KoanRegion = Region<KoanStorageProfile>`) lives in the embedder's arena
//! module (Koan's `machine::core::arena`). See
//! [memory-model.md § Region lifetime erasure](../../../design/memory-model.md#region-lifetime-erasure)
//! for the reference-side lifetime-erasure soundness argument and
//! [design/reach.md § Retention model](../../design/reach.md#retention-model)
//! for how an escaped value's region stays alive.

use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::{Rc, Weak};

use bumpalo::Bump;
use elsa::FrozenMap;

use super::{
    BumpAllocator, Carrier, Delivered, DropFree, Erased, FoldedPlacement, PinBundle, PinsRegion,
    ReachDescription, Reattachable, ReferenceFamily, RegionOwner, Retained, SealedExtern,
    StepCoverage, Witness, Witnessed,
};

/// A workload's storage declaration: the frame-owner type a [`Region`]'s reach descriptions name.
/// Value storage needs no declaration at all — every family a region hosts is `Drop`-free and lives
/// in the region's bump, which is untyped.
pub trait StorageProfile: Sized {
    /// A [`Region`] interns its reach descriptions in a side table typed at this owner
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
    /// region; what keeps those members alive is [`retained_reach`](Self::retained_reach) below,
    /// folded by the same act that interned the entry.
    ///
    /// `elsa::FrozenMap` is what makes get-or-mint expressible through `&self`: it inserts through a
    /// shared borrow and hands back a `&` to the boxed entry valid for the region's whole life. The
    /// map owns that fixed-address guarantee, so a carrier can share a thin reference into it with
    /// no `Drop`-order, dangling, or hand-audited-pointer hazard.
    reach_table: FrozenMap<Box<[usize]>, Box<ReachDescription<W::FrameOwner>>>,
    /// The region's **bump**: the storage home for every `Drop`-free value that names the region's
    /// own lifetime, reached as a [`BumpAllocator`] over this field. Bumped rather than arena'd
    /// because the allocator itself is lifetime-free, so `'a` enters only at the allocating call —
    /// which is what lets a bumped value hold an `&'a` back into this same region with no erasure
    /// and no residence audit. A lifetime-*typed* cell could not: its type would have to name `'a`,
    /// and [`Region`] has no lifetime parameter, which is why a [`ReachDescription`] is
    /// lifetime-free.
    ///
    /// A `Bump` runs **no destructor** for what it holds — it releases its chunks whole. That is the
    /// point: everything allocated here costs nothing at region teardown, which is what keeps a
    /// [`Sectioned`](super::Sectioned) container `Copy` and `Drop`-free so a frame drop need not
    /// walk one. The `T: Copy` bound the allocator's value, slice and text verbs carry is what
    /// statically holds callers to it — a `Copy` type has no `Drop` to skip. ("`Drop`-free" itself
    /// has no expressible bound; `Copy` is the static proxy.) A **collection** built over the same
    /// allocator's raw [`Allocator`](allocator_api2::alloc::Allocator) seam is where that bound
    /// stops travelling with the bytes; such a writer proves its elements glue-free with a
    /// `const { assert!(!needs_drop::<_>()) }` at the declaration site that names their type.
    ///
    /// A cycle among bumped entries is harmless: everything here dies with the region, at once.
    /// Occupancy is one figure for the whole bump ([`bump_capacity`](Self::bump_capacity)) —
    /// the copy-versus-pin decision reads a region's total against a candidate value's own copy
    /// size and never needs a per-writer breakdown.
    bump: Bump,
    /// The region's **union bundle**: one deduped [`PinBundle`] owning a pin for every region
    /// anything resident here reaches, retained for the region's whole life. It is what a non-owning
    /// description cannot supply — the liveness backing a value re-anchored at this region's
    /// lifetime ([`Region::retain_reach`]).
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
    /// The region's **owner** — the frame-owner value this region is a part of, stamped onto every
    /// description minted here as its host, so a value's residence is read off the description
    /// rather than carried a second time on the envelope.
    ///
    /// `Weak` because the owner owns this region: a strong link would be a self-cycle. The upgrade
    /// is infallible wherever it is reached — a live region *is* live storage inside its owner, so
    /// anything holding a pin that keeps this region alive holds the owner alive too.
    ///
    /// Supplied at construction (there is no un-owned region): an owner builds itself through
    /// `Rc::new_cyclic` and hands its own `Weak` down.
    host: Weak<W::FrameOwner>,
}

/// The byte capacity a fresh region asks its bump for up front, sized so a frame's whole residency
/// lands in **one** chunk. Left to itself bumpalo reserves nothing until the first store, then
/// takes 448 bytes and doubles on each overflow — 448, 960, 1984, 4032 — and a frame overruns the
/// early rungs: a per-call region's high-water occupancy measures between 1 KiB and 7 KiB across
/// the audit shapes, so the ladder is climbed three or four times per frame, reserving more in
/// total than one right-sized chunk does and paying an allocation at every rung.
///
/// 4 KiB is the smallest ask that clears the whole measured spread. bumpalo rounds a sub-page
/// request up to a power of two and a page-or-larger one up to a page, so the ask does not land
/// where it is aimed: the largest reservation any smaller ask can reach is the 4032-byte rung,
/// which cuts the top of the distribution off, and this ask yields 8128 usable bytes — enough that no measured frame
/// reaches a second chunk, and enough headroom that an ordinary layout change cannot push one there.
///
/// Sizing the chunk to a frame is also what makes [`bump_capacity`](Region::bump_capacity) a stable
/// price: the figure a pin is costed at stops moving with the byte size of what a frame happens to
/// hold, because the reservation no longer tracks it.
///
/// Paid eagerly, but at the mint [`RegionHost::region`](super::RegionHost::region) performs on
/// first access rather than at frame construction — so a frame that never reaches its region still
/// allocates nothing.
pub(super) const FIRST_CHUNK_BYTES: usize = 4096;

impl<W: StorageProfile> Region<W> {
    /// The library's raw-region constructor — `pub(crate)`, so the mint point an embedder reaches
    /// is [`RegionHost::region`](super::RegionHost::region), which calls this lazily on first
    /// access. An owner sources `host` from `Rc::new_cyclic`, so a region cannot exist without
    /// naming the owner that holds it.
    ///
    /// Minting allocates: the bump takes its [`FIRST_CHUNK_BYTES`] chunk here rather than on the
    /// first value written into it. That is the one allocation a region makes for itself, and the
    /// laziness of the mint is what keeps it from being paid by a frame that stores nothing.
    pub(crate) fn new(host: Weak<W::FrameOwner>) -> Self {
        Self {
            reach_table: FrozenMap::new(),
            bump: Bump::with_capacity(FIRST_CHUNK_BYTES),
            retained_reach: RefCell::new(PinBundle::empty()),
            host,
        }
    }

    /// The host stamped onto every description frozen into this region's side table.
    pub(crate) fn host(&self) -> Weak<W::FrameOwner> {
        self.host.clone()
    }

    /// Get-or-mint the description of `composed`'s member set in this region's side table **and
    /// establish its retention here** — the reach-allocation path, reached through
    /// [`ReachDescription::mint_resident`] and its threaded twin. Keyed on the canonical member set
    /// ([`PinBundle::intern_key`]): a miss builds the description, stores it, and folds `composed`
    /// (self rule then eternal rule) into this region's union bundle; a hit returns the existing
    /// entry and folds nothing, because an identical member set was already folded at that entry's
    /// own miss.
    ///
    /// Fusing the two is what makes the retention proof available: one description exists per
    /// distinct reach per region, so an entry's existence *is* the proof that this region pins what
    /// the entry names. Nothing records retention separately, and no caller can hold the composed
    /// bundle to fold by hand.
    ///
    /// No lifetime-retype and no `unsafe`: a [`ReachDescription`] is lifetime-free, and the map's
    /// own append-stable guarantee returns a reference valid for the `&'a self` borrow — the
    /// region's life — so the description a carrier references outlives every read pinned by this
    /// region's owner.
    ///
    /// The probe key is built in the caller's own frame and boxed **only on the miss**, where the
    /// map takes ownership of it: a hit costs a hash and a compare, not an allocation.
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

    /// This region's bump as a [`BumpAllocator`] — the write surface for its bytes, carrying both
    /// the `Copy`-guarded verbs and the raw allocator seam a **mutable** collection is built over.
    /// Crate-private: an embedder reaches the same allocator through [`RegionHandle::allocator`],
    /// the accessor that names a region it is authorized over. Bytes taken either way are priced by
    /// [`bump_capacity`](Self::bump_capacity), which is what keeps an off-verb allocation honest.
    pub(crate) fn allocator(&self) -> BumpAllocator<'_> {
        BumpAllocator::over(&self.bump)
    }

    /// The region's bump footprint, in **reserved chunk capacity** (`Bump::allocated_bytes`):
    /// padding and the newest chunk's unused tail included. Capacity rather than a live-byte tally
    /// because this figure prices the copy-versus-pin decision and a pin retains chunks whole —
    /// capacity *is* what a pin costs. Reading it off the allocator also means an allocation that
    /// bypasses the doors here (a collection built over the raw `allocator-api2` seam) is priced
    /// like any other. Monotonic, like the bump itself: nothing is freed before the region dies.
    pub fn bump_capacity(&self) -> usize {
        self.bump.allocated_bytes()
    }

    /// Fold an owning [`PinBundle`] into the region's union bundle, retained for the region's whole
    /// life — the liveness a non-owning description cannot supply for a value re-anchored at this
    /// region's lifetime (adoption, [`Delivered::adopt_into`](super::Delivered::adopt_into); resting,
    /// [`RegionHandle::retain_reach`]).
    ///
    /// The fold is [`PinBundle::absorb`], so retaining the same region twice costs one `Rc` in total
    /// and a retention subsumed by an outer member costs none — the field stays a single antichain
    /// rather than a bundle per retention.
    ///
    /// The eternal rule is applied **before** the debug-mode ring detector runs, so what the
    /// detector walks is the retention that actually lands here: a member the rule strips closes no
    /// ring, because no pin on it is ever taken.
    pub(crate) fn retain_reach(&self, bundle: PinBundle<W::FrameOwner>)
    where
        W::FrameOwner: RegionOwner<Region = Region<W>>,
    {
        #[cfg(any(test, feature = "test-hooks"))]
        super::host::note_reach_retention_fold();
        let retained = bundle.without_eternal();
        #[cfg(debug_assertions)]
        self.detect_pin_cycle(&retained);
        self.retained_reach.borrow_mut().absorb(retained);
    }

    /// **The pin-ring detector**: report every way `incoming`'s members lead liveness back to this
    /// region, run just before the retention that would close the ring is installed. Diagnostic
    /// only — nothing here changes what is retained.
    ///
    /// It runs *online*, at the fold, because a mutual pin is by construction unreachable from any
    /// live root once the external references drop — that disconnection **is** the leak — so a
    /// walk started later can never find it. This is also the one moment both ends are in hand: the
    /// region doing the retaining, and the members it is about to hold.
    ///
    /// The graph walked has two edge kinds, both of which transmit liveness: *retention* (a
    /// region's union bundle owns an `Rc` on a member) and *chain* (an owner pins its own region
    /// and every ancestor's, [`PinsRegion::for_each_pinned_region`]). A ring exists iff, from a
    /// member about to be retained here, that closure reaches an owner pinning **this** region.
    ///
    /// No `RefCell` hazard against the `borrow_mut` the caller takes next: a path arriving back at
    /// this region is caught by the `pins_region(self)` test *before* its bundle would be expanded,
    /// so `self.retained_reach` is never borrowed inside the walk. Iterative rather than recursive
    /// because the retention graph's depth is a property of the workload, not of this crate.
    #[cfg(debug_assertions)]
    fn detect_pin_cycle(&self, incoming: &PinBundle<W::FrameOwner>)
    where
        W::FrameOwner: RegionOwner<Region = Region<W>>,
    {
        use std::collections::HashSet;

        let identity = |owner: &Rc<W::FrameOwner>| Rc::as_ptr(owner) as usize;
        for member in incoming.detector_members() {
            // One walk per newly retained member, so a report names the member that closed the
            // ring rather than the whole bundle it arrived in.
            let mut visited: HashSet<*const Region<W>> = HashSet::new();
            let mut stack: Vec<(Rc<W::FrameOwner>, Vec<usize>)> =
                vec![(Rc::clone(member), vec![identity(member)])];
            while let Some((owner, path)) = stack.pop() {
                if owner.pins_region(self) {
                    super::host::note_pin_cycle(super::host::PinCycleReport {
                        retainer: identity(&self.host_owner()),
                        path,
                    });
                    break;
                }
                // The successors are pushed from *inside* the visit, because a visited region is
                // only named for the length of the call — it is a `&Self::Region` under the
                // trait's `for<'_>` quantifier, so nothing borrowed from it may be collected and
                // walked afterwards. What crosses back out is owned: `Rc` members and addresses.
                owner.for_each_pinned_region(&mut |region| {
                    if !visited.insert(region as *const Region<W>) {
                        return;
                    }
                    for next in region.retained_reach.borrow().detector_members() {
                        let mut extended = path.clone();
                        extended.push(identity(next));
                        stack.push((Rc::clone(next), extended));
                    }
                });
            }
        }
    }

    /// The blame target a ring report names. Infallible wherever a retention runs: a live region is
    /// live storage inside its owner, so anything reaching this region holds the owner alive too.
    #[cfg(debug_assertions)]
    fn host_owner(&self) -> Rc<W::FrameOwner> {
        self.host
            .upgrade()
            .expect("a region is storage inside its owner: reaching one means the owner is live")
    }

    /// Drop everything this region retains — **the ring slate's teardown, and nothing else.**
    ///
    /// A detected pin ring is a genuine leak by construction: the two regions own each other's
    /// owners, so neither ever drops and the process-exit leak detector reports every host in the
    /// ring. A test that builds one deliberately must dismantle it. `cfg(test)`-only because a
    /// retention's whole contract is that it lives exactly as long as the region does.
    #[cfg(test)]
    pub(in crate::witnessed) fn release_retained_for_test(&self) {
        *self.retained_reach.borrow_mut() = PinBundle::empty();
    }

    /// Number of distinct owners in the region's union bundle. Exposes a count, not the bundle, so
    /// it cannot be used to narrow a claim.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn retained_reach_len(&self) -> usize {
        self.retained_reach.borrow().members().len()
    }
}

// No `Default` impl: `Default` is a public trait, so implementing it here would hand every
// embedder back a public mint route (`Region::<W>::default()`) even with `new` sealed above —
// the raw-region constructor stays `pub(crate)`.

// SAFETY: a `Region`'s values live in its `Bump`, which never moves an allocated chunk — an
// allocation is served from a chunk's free space and a new chunk is linked in rather than the old
// one grown — so a held `&Region` keeps any pointee alloc'd in it (or a strict ancestor it roots) at
// a fixed address, the bound the consumer-pull lift's frameless re-anchor relies on to witness the
// destination lifetime.
unsafe impl<W: StorageProfile> super::Witness for Region<W> {}

/// The at-will allocation capability for a [`Region`] — a `Copy` newtype over `&'a Region<W>`
/// carrying the public allocation surface. A bare `&Region` cannot allocate (the engine's alloc
/// methods are crate-private) and safe embedder code cannot wrap one into a handle (the field and
/// the raw constructor are crate-private): the embedder-reachable mint is [`Self::from_owner`], which
/// requires the region's *owner* and so rests on an audited, `unsafe`-opt-in `RegionOwner` impl.
/// The library additionally hands one out at a `for<'b>` brand from its construction combinators
/// ([`Witnessed::yoke_handle`](super::Witnessed::yoke_handle), [`StepContext::alloc`](super::StepContext::alloc),
/// [`StepContext::alloc_with`](super::StepContext::alloc_with)).
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

    /// Mint the allocation capability from a region owner — the public minter. It requires `&F`
    /// where `F: RegionOwner`, an owner type whose (unsafe-to-implement) contract pins the region,
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

    /// **This region's owner, named weakly** — so a value resident here reads the frame that owns
    /// it off the region rather than carrying a copy of the same `Weak` in a field of its own.
    ///
    /// On the handle rather than on a bare `&Region`: a handle holder already has the region's full
    /// allocation capability, so handing it the owner grants nothing new — where a bare `&Region`
    /// gaining an upgradeable owner path would reopen the mint route [`Self::from_owner`] closes
    /// (upgrade the `Weak`, mint a handle off the owner).
    pub fn host(self) -> Weak<W::FrameOwner> {
        self.region.host()
    }

    /// **This handle's region's bump as a [`BumpAllocator`](super::BumpAllocator)** — the
    /// frame-lifetime bytes door.
    ///
    /// Deliberately not the same door as
    /// [`fold_and_bump`](super::FoldedPlacement::fold_and_bump), which exists to compose its
    /// **operands'** reach into the product's carrier. Bare bytes have no operands and no reach to
    /// compose, so what is left is an ordinary borrow: nothing written through the returned
    /// allocator can outlive the `&'a Region` this handle holds, which the borrow checker enforces
    /// with no audit and no `unsafe`. A value built *around* those bytes that embeds a **foreign**
    /// operand is gated at its own rank-2 brand ([`bump_born_with`](Self::bump_born_with),
    /// [`fold_and_bump`](super::FoldedPlacement::fold_and_bump)); one whose fields are all at this
    /// handle's own `'a` needs no gate and takes
    /// [`in_place`](super::BumpAllocator::in_place) directly.
    ///
    /// The destructor obligation travels on the allocator, not here: the `Copy`-bounded verbs carry
    /// it statically, and a collection over the raw seam restates it at the declaration site naming
    /// its element type.
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

    /// Fold an owned [`StepCoverage`] into this handle's region's union bundle — the resting-cell
    /// fold backing [`Delivered::rest_in`](super::Delivered::rest_in): a value dropped to rest here
    /// keeps referencing the description its producer stamped, so nothing is minted and the coverage
    /// it arrives with is what must be pinned for as long as this region lives.
    ///
    /// **Crate-private.** Every embedder-reachable retention rides a mint
    /// ([`Self::mint_retained`]), which performs its own.
    ///
    /// The **self rule** applies as it does at [`Self::mint_retained`]: this region is stripped from
    /// the retained bundle, because a region owning a pin on itself is a cycle nothing ever breaks.
    /// It is what lets a caller hand over a whole coverage — a resting cell's claim, home included —
    /// without first asking whether that home happens to be this very region.
    pub(crate) fn retain_reach(self, coverage: StepCoverage<W::FrameOwner>)
    where
        W::FrameOwner: RegionOwner<Region = Region<W>>,
    {
        // The coverage arrives owned, so the strip is in place: no second buffer and no refcount
        // traffic, where `without_region` would clone the whole bundle to drop one member.
        let mut bundle = coverage.0;
        bundle.remove_region(self.region);
        self.region.retain_reach(bundle)
    }

    /// **Mint and retain in one verb** — the embedder-facing reach-derivation door. Freezing
    /// `sources`' composed reach into this region's side table is the same act that folds the owning
    /// bundle into this region's union bundle ([`ReachDescription::mint_resident`]), so there is no
    /// in-between state where an embedder could hold the pins.
    ///
    /// No policy is threaded in: the mint applies subsumption and the self rule alone, so the
    /// description is the value's exact reach and the retained bundle is that reach minus this
    /// region itself (a region owning a pin on itself is a cycle).
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
    /// to. The value pre-exists; this names its reach.
    ///
    /// It sits on the handle that [`mint_retained`](Self::mint_retained) sits on, so the residence
    /// the description is stamped with and the capability that seals under it are the *same*
    /// handle — there is no second door for a caller to bring a foreign description to. `'v: 'a` is
    /// the residence check: a borrow that does not live as long as the region handle cannot be
    /// sealed under it at all. `'v` is free rather than pinned to `'a` so an *invariant* family —
    /// one whose `At` cannot shrink — can still seal a value that outlives the handle, which is the
    /// safe direction.
    ///
    /// The witness is the reference-only [`Carrier`]: it names the reach without pinning it, so
    /// every read opens under an external pin — the active frame during the producing step, the
    /// delivery envelope's own bundle while the terminal walks, the destination region once it has
    /// been adopted there. A value whose reach is genuinely empty takes `mint_retained(&[])` for
    /// its description, the degenerate case of the same door.
    pub fn seal_reaching<'v: 'a, T: Reattachable + DropFree>(
        self,
        value: T::At<'v>,
        reach: &'a ReachDescription<W::FrameOwner>,
    ) -> Witnessed<T, Carrier<W::FrameOwner>> {
        Witnessed::from_erased(Erased::erase(value), Carrier::new(reach))
    }

    /// This region's owner as an owned pin. Infallible while a handle exists: a handle borrows the
    /// region, a live region *is* live storage inside its owner, so the owner cannot have dropped.
    fn home_pin(self) -> Rc<W::FrameOwner> {
        self.host()
            .upgrade()
            .expect("a region handle borrows live storage, so its owner is live")
    }

    /// **Envelope a value that already lives in this region and reaches nothing** — the delivery
    /// twin of [`seal_reaching`](Self::seal_reaching)`(value, mint_retained(&[]))`.
    ///
    /// The description's residence, the seal, and the home pin the envelope travels under all come
    /// off *this* handle, so there is no second argument for a caller to pair wrongly and no owned
    /// coverage to assemble: a value reaching nothing beyond the region it lives in is covered by
    /// its home alone. `'v: 'a` is the residence check, exactly as at
    /// [`seal_reaching`](Self::seal_reaching).
    ///
    /// A value whose borrows *do* reach elsewhere never arrives here: it reaches an envelope as a
    /// composition's product, which mints its reach at the destination and owns it, or as the
    /// [`lift`](Delivered::lift) of a carrier whose description already names it.
    pub fn deliver_resident<'v: 'a, T: Reattachable + DropFree>(
        self,
        value: T::At<'v>,
    ) -> Delivered<T, Carrier<W::FrameOwner>, W::FrameOwner>
    where
        W::FrameOwner: RegionOwner<Region = Region<W>>,
    {
        let home = self.home_pin();
        let cell =
            Retained::from_witnessed(self.seal_reaching::<T>(value, self.mint_retained(&[])));
        Delivered::hosted(cell, home, StepCoverage::empty())
    }

    /// **Build a value inside this region and hand back its envelope** — the born-witnessed door for
    /// a construction whose product is region-pure: `build` runs under a `for<'b>` quantifier and
    /// receives a handle on this same region, so the only references it can return are region-derived
    /// or owned, and the product's reach is exactly this region.
    ///
    /// The delivery twin of [`Self::deliver_resident`] for a value that does not pre-exist: the yoke
    /// witness and the envelope's home pin are one `Rc` on this region's own owner, so the value is
    /// *born* under the pin it travels under. Coverage is empty by construction — nothing the brand
    /// admits reaches another region.
    ///
    /// A construction that embeds a **foreign** operand composes that operand's reach instead, at
    /// [`bump_born_with`](Self::bump_born_with) or the fold doors
    /// ([`StepContext::alloc_with`](super::StepContext::alloc_with)).
    pub fn deliver_yoked<T: Reattachable + DropFree>(
        self,
        build: impl for<'b> FnOnce(RegionHandle<'b, W>) -> T::At<'b>,
    ) -> Delivered<T, Carrier<W::FrameOwner>, W::FrameOwner>
    where
        W: 'static,
        W::FrameOwner: RegionOwner<Region = Region<W>>,
    {
        let home = self.home_pin();
        let born = Witnessed::<T, Rc<W::FrameOwner>>::yoke_handle(Rc::clone(&home), build)
            .into_reference_only::<W>();
        Delivered::hosted(Retained::from_witnessed(born), home, StepCoverage::empty())
    }

    /// **Build a region-borrowing value from a crossing operand and bump it here** — the born door,
    /// for a value built *from* a reference the caller already holds where the surrounding
    /// construction lives at an enclosing lifetime the closure's `'b` cannot see.
    ///
    /// `build` runs under a `for<'b>` quantifier and receives a [`FoldedPlacement`] over *this*
    /// handle's region plus `operand` re-anchored to that same `'b` — one [`zip`](SealedExtern::zip)ped
    /// open, so an invariant family is well typed at the brand. That quantifier is the residence
    /// proof and it is a compile one: `'b` has no outlives relation to any enclosing lifetime, so
    /// the only `&'b Region<W>` a closure body can name is the placement's. `pin` is borrowed for
    /// `'a`, the destination region's lifetime, so it covers the stored reference's whole life
    /// rather than merely the call.
    ///
    /// The `const` assert is the family's side of the bargain: a bump-hosted family runs **no
    /// destructor**, so it must have none to run. It monomorphizes per family, so a field that later
    /// grows a `Drop` is a build error here.
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
        // itself never is — it is built at `'b` and stored at `'b`.
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

/// [`Reattachable`] family for a [`RegionHandle`], so the capability itself can be erased and
/// re-anchored through the witnessed substrate.
pub struct RegionHandleFamily<W>(PhantomData<W>);

// SAFETY: `RegionHandle<'r, W>` is a newtype over `&'r Region<W>`, a thin pointer whose layout is
// identical for every choice of `'r`.
unsafe impl<W: StorageProfile + 'static> Reattachable for RegionHandleFamily<W> {
    type At<'r> = RegionHandle<'r, W>;
}

/// A [`RegionHandle`] is a `Copy` thin pointer, so the handle family rests in the Copy tier.
impl<W> DropFree for RegionHandleFamily<W> {}
