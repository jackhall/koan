//! The **bump door**: the public path from an embedder's `Drop`-free value family into a
//! [`Region`]'s byte arena, and the [`BumpAllocator`] write surface a constructor builds through
//! ([design/witnessed-memory.md § The bump allocator](../../design/witnessed-memory.md#the-bump-allocator)).
//!
//! A value stored here is *not* erased. The bump is lifetime-free, so `'b` enters only at the
//! allocating call — which is why a bumped value may hold an `&'b` back into the very region it
//! lives in with no [`erase_to_static`](super::erase_to_static), no storage policy of its own, and
//! no residence check. A lifetime-*typed* cell could not: its slot type would have to name a
//! lifetime a [`Region`] has no parameter for, so a region-self-referential value reaching one
//! would have to have its borrow erased and vetted back.
//!
//! **Confinement is the brand, not the capability's privacy.** The door hangs on
//! [`FoldedPlacement`], the fold engines' unforgeable placement capability, and `'b` is the
//! *enclosing* fold's brand. A constructor cannot capture an ambient `&'x` into the value it builds,
//! because `'x` has no outlives relation to a universally-quantified `'b`; and the door's product,
//! an [`Opened<'b, …>`](super::Opened), cannot leave the enclosing fold closure for the same reason.
//! Bare inside the brand, bundled on the way out. That the brand alone does the confining is why the
//! write surface handed to a constructor is the plain [`BumpAllocator`] any handle-holder can mint:
//! a mint restriction on top would guard nothing `'b` does not.
//!
//! **No erasure, because the operands already live at the door's brand.** Operands arrive as
//! [`Opened<'b, V, Carrier<F>>`](super::Opened) — their values are *already* `V::At<'b>`, so the door
//! re-anchors nothing: no [`Erased::reattach`](super::Erased::reattach), no retype, no `unsafe`. An
//! `Opened` borrows at `'b` under the pin that opened it, and that borrow **is** the holder-rule
//! coverage [`ReachDescription::to_bundle`] needs, so the door takes no pin argument. An operand
//! living in another region enters the brand through the enclosing fold engine
//! ([`StepContext::alloc_with`](super::StepContext::alloc_with)), which is where cross-region reach
//! ownership already flows; the door composes *within* the brand.

use std::alloc::Layout;
use std::hash::Hash;
use std::ptr;
use std::ptr::NonNull;

use allocator_api2::alloc::{AllocError, Allocator};
use bumpalo::Bump;
use hashbrown::{DefaultHashBuilder, HashMap};

use super::{
    Carrier, DropFree, FoldedPlacement, Opened, PinBundle, PinsRegion, ReachDescription,
    Reattachable, Region, RegionOwner, StorageProfile,
};

/// **The region's bump, as one handle carrying both storage tiers** — the guarded verbs every
/// write-once byte goes through, and the raw [`Allocator`] impl a **mutable** collection is built
/// over so its backing allocation lands in the region's own chunks instead of the global heap.
///
/// A surface that can reach a region's bytes hands back this type rather than restating a verb set
/// of its own, so the `Copy` guard and the rationale behind it are written once. The verbs'
/// primitives are std shapes only — a `Copy` value, a `Copy` slice, a `str` — so the door names no
/// workload type and grows no per-family verb, and each hands back a shared `&'b`, never the `&mut`
/// the bump itself returns: a bumped value is region state a holder names, not one it owns.
///
/// `Copy`, with a private field and a `pub(crate)` wrapping constructor, so a value of this type
/// exists only where a region hands one out and `'b` stays the region's own brand: nothing built
/// through one can outlive the region whose bytes it holds. Where `'b` is a rank-2 fold brand, that
/// same lifetime is the confinement — which is why the allocator needs no mint privacy of its own
/// to serve as a fold's write surface.
///
/// **The two tiers, and why the guard cannot cover both.** What licenses handing the raw allocator
/// out at all is that a `Bump` releases its chunks whole and [`Region::bump_capacity`] prices every
/// byte taken through it, counted verb or not — so an off-verb allocation is still an
/// honestly-reported one. Deallocation is a **no-op**: a collection that shrinks or rehashes
/// abandons its old allocation as dead bump bytes, which is why that seam suits a table whose churn
/// is bounded (geometric growth, in-place slot reuse) and not one that frees in a loop. The
/// [`Allocator`] trait says nothing about destructors, so the `T: Copy` guard the verbs carry cannot
/// ride there — an element stored through a collection over this allocator is never dropped, and it
/// is the **embedder's own declaration site** that has to hold the line (a
/// `const { assert!(!needs_drop::<T>()) }` where the collection's element type is named).
pub struct BumpAllocator<'b>(&'b Bump);

// `Copy` grants nothing new: the handle names no more than the `&'b Bump` it wraps.
impl Clone for BumpAllocator<'_> {
    fn clone(&self) -> Self {
        *self
    }
}
impl Copy for BumpAllocator<'_> {}

/// Every method forwards to `&Bump`'s own [`Allocator`] impl rather than leaning on the trait's
/// defaults: bumpalo's `grow` extends the newest chunk in place when the allocation is the last one
/// out, which the default (allocate-copy-deallocate) would give up. The safety obligation on each
/// method is discharged by the delegate, which receives exactly the arguments this one was given —
/// so every inner `unsafe` block below re-states that one obligation rather than adding its own.
unsafe impl Allocator for BumpAllocator<'_> {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        Allocator::allocate(&self.0, layout)
    }

    fn allocate_zeroed(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        Allocator::allocate_zeroed(&self.0, layout)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // SAFETY: `ptr`/`layout` reach the delegate exactly as this method received them.
        unsafe { Allocator::deallocate(&self.0, ptr, layout) }
    }

    unsafe fn grow(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        // SAFETY: as in `deallocate` — the arguments are forwarded unchanged.
        unsafe { Allocator::grow(&self.0, ptr, old_layout, new_layout) }
    }

    unsafe fn grow_zeroed(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        // SAFETY: as in `deallocate` — the arguments are forwarded unchanged.
        unsafe { Allocator::grow_zeroed(&self.0, ptr, old_layout, new_layout) }
    }

    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        // SAFETY: as in `deallocate` — the arguments are forwarded unchanged.
        unsafe { Allocator::shrink(&self.0, ptr, old_layout, new_layout) }
    }
}

impl<'b> BumpAllocator<'b> {
    /// Wrap `bump` — `pub(crate)`, so no embedder can pair a `BumpAllocator` with a bump that is
    /// not a region's.
    pub(crate) fn over(bump: &'b Bump) -> Self {
        BumpAllocator(bump)
    }

    /// Bump one `Copy` value into the region and hand back the co-located `&'b`.
    ///
    /// `T: Copy` is the static proxy for "no destructor to skip": the bump releases its chunks whole
    /// and runs no `Drop`, so a `Drop`-bearing `T` would silently leak its owned contents.
    /// "`Drop`-free" has no expressible bound; `Copy` is the honest approximation. The stored shapes
    /// that are glue-free without being `Copy` — a frozen table's header, a value that keeps mutating
    /// through interior mutability — have verbs of their own ([`frozen_table`](Self::frozen_table),
    /// [`in_place`](Self::in_place)) rather than a relaxation of this one.
    ///
    /// ```compile_fail
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, FixtureProfile};
    /// use workgraph::witnessed::RegionHandle;
    ///
    /// let cart = fresh_cart();
    /// let handle: RegionHandle<'_, FixtureProfile> = RegionHandle::from_owner(&*cart);
    /// // A `String` owns a heap buffer the bump would never free — rejected by `T: Copy`.
    /// let _stored: &String = handle.allocator().value(String::from("leaks"));
    /// ```
    pub fn value<T: Copy>(self, value: T) -> &'b T {
        self.0.alloc(value)
    }

    /// Bump a value that **stays resident and keeps mutating in place** — the glue-free verb for the
    /// shape `Copy` cannot spell. A value whose fields are `Copy`, `Cell`s of `Copy`, and tables
    /// built over this same allocator carries no drop glue, but interior mutability rules `Copy` out:
    /// copying it would fork the mutable state its holders share. This verb takes the proof directly.
    ///
    /// The `const` block is that proof: it monomorphizes per `T`, so a field that later grows a
    /// `Drop` is a build error at the call that admitted it rather than a silent leak. What the
    /// assert cannot state — that a suppressed destructor would have freed only bump bytes — is
    /// structural, since the interior tables are built over this allocator and the region releases
    /// them whole.
    ///
    /// Shared `&'b`, never `&mut`: every write goes through the value's own interior-mutable fields.
    ///
    /// ```
    /// use std::cell::Cell;
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, FixtureProfile};
    /// use workgraph::witnessed::RegionHandle;
    ///
    /// let cart = fresh_cart();
    /// let handle: RegionHandle<'_, FixtureProfile> = RegionHandle::from_owner(&*cart);
    /// let counter: &Cell<u32> = handle.allocator().in_place(Cell::new(0));
    /// counter.set(counter.get() + 1);
    /// assert_eq!(counter.get(), 1);
    /// ```
    ///
    /// ```compile_fail
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, FixtureProfile};
    /// use workgraph::witnessed::RegionHandle;
    ///
    /// let cart = fresh_cart();
    /// let handle: RegionHandle<'_, FixtureProfile> = RegionHandle::from_owner(&*cart);
    /// // A `String` owns a heap buffer the bump would never free — rejected by the assert.
    /// let _stored: &String = handle.allocator().in_place(String::from("leaks"));
    /// ```
    pub fn in_place<T>(self, value: T) -> &'b T {
        const {
            assert!(
                !std::mem::needs_drop::<T>(),
                "a bump-hosted value must carry no drop glue: the bump runs no destructor",
            )
        };
        self.0.alloc(value)
    }

    /// Copy a `Copy` slice into the region and hand back the co-located `&'b [T]` — the shape an
    /// operator group's member block or an expression's part list takes. Same `T: Copy` rationale as
    /// [`value`](Self::value).
    pub fn slice<T: Copy>(self, items: &[T]) -> &'b [T] {
        self.0.alloc_slice_copy(items)
    }

    /// Fill a `Copy` slice in the region **straight from `items`** and hand back the co-located
    /// `&'b [T]` — the peer of [`slice`](Self::slice) for a run whose elements are computed rather
    /// than already sitting in one. Same `T: Copy` rationale as [`value`](Self::value).
    ///
    /// The run is reserved before the first element lands, so the iterator's length has to be known
    /// up front — hence `ExactSizeIterator` rather than the looser bound
    /// [`frozen_table`](Self::frozen_table) takes, where a wrong `size_hint` costs a rehash and not
    /// correctness. Staging into an owned run and handing it to [`slice`](Self::slice) instead pays a
    /// heap allocation and two copies; this pays one copy into the bytes the value keeps.
    pub fn slice_from_iter<T: Copy>(
        self,
        items: impl IntoIterator<Item = T, IntoIter: ExactSizeIterator>,
    ) -> &'b [T] {
        self.0.alloc_slice_fill_iter(items)
    }

    /// Copy `text` into the region and hand back the co-located `&'b str`. A `str` is a slice of
    /// `Copy` bytes under a UTF-8 invariant, so it carries no destructor either and needs no bound to
    /// say so.
    pub fn text(self, text: &str) -> &'b str {
        self.0.alloc_str(text)
    }

    /// Build a **frozen** key→value table over this bump and place its header there too — the shape
    /// an embedder's keyed index takes when a sorted run and a binary search will not do.
    ///
    /// The verb builds the table itself rather than placing one handed in, and that is the point:
    /// the buckets are allocated over *this* allocator, so the destructor the header forgoes would
    /// have freed only bump memory the region releases whole. That a table backed by some other
    /// allocator cannot be supplied is the half of the argument no assert could check.
    ///
    /// The other half is checked: the `const` block proves the entries carry no drop glue, so the
    /// suppressed destructor has nothing to do *but* free that bucket array. It monomorphizes per
    /// `(K, V)`, so an entry type that later grows a `Drop` is a build error at the declaration that
    /// admitted it rather than a silent leak. Together those are why the internal
    /// [`ManuallyDrop`](std::mem::ManuallyDrop) header placement is lossless; the wrapper is deref'd
    /// away here, so no holder's type mentions it.
    ///
    /// Sized to the iterator's `size_hint` lower bound up front so an exact hint fills without a
    /// rehash; a loose one costs a rehash, not correctness — though the growth reallocation strands
    /// the old bucket array in the bump as garbage [`Region::bump_capacity`] then over-reports.
    /// Duplicate keys resolve last-wins, as [`HashMap::insert`] does. The returned shared reference
    /// **is** the freeze — no mutation is reachable through it, which is what distinguishes this from
    /// a table built over the raw [`Allocator`] seam and kept writable.
    ///
    /// ```
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, FixtureProfile};
    /// use workgraph::witnessed::RegionHandle;
    ///
    /// let cart = fresh_cart();
    /// let handle: RegionHandle<'_, FixtureProfile> = RegionHandle::from_owner(&*cart);
    /// let index = handle.allocator().frozen_table([("width", 0usize), ("height", 1)]);
    /// assert_eq!(index.get(&"height"), Some(&1));
    /// assert_eq!(index.len(), 2);
    /// ```
    ///
    /// ```compile_fail
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, FixtureProfile};
    /// use workgraph::witnessed::RegionHandle;
    ///
    /// let cart = fresh_cart();
    /// let handle: RegionHandle<'_, FixtureProfile> = RegionHandle::from_owner(&*cart);
    /// // A `String` value owns a heap buffer the bump would never free — rejected by the assert.
    /// let _index = handle.allocator().frozen_table([("k", String::from("leaks"))]);
    /// ```
    pub fn frozen_table<K, V>(
        self,
        entries: impl IntoIterator<Item = (K, V)>,
    ) -> &'b BumpBackedMap<'b, K, V>
    where
        K: Eq + Hash,
    {
        const {
            assert!(
                !std::mem::needs_drop::<K>() && !std::mem::needs_drop::<V>(),
                "a bump-hosted table's entries must carry no drop glue: the bump runs no destructor",
            )
        };
        let entries = entries.into_iter();
        let mut table = BumpBackedMap::with_capacity_and_hasher_in(
            entries.size_hint().0,
            DefaultHashBuilder::default(),
            self,
        );
        table.extend(entries);
        // `ManuallyDrop` suppresses the table's own destructor, which the two arguments above make
        // lossless. Bound rather than returned inline, so the deref coercion that strips the wrapper
        // happens at the return instead of confusing `alloc`'s type inference.
        let header: &'b std::mem::ManuallyDrop<_> =
            self.0.alloc(std::mem::ManuallyDrop::new(table));
        header
    }
}

/// A table whose bucket array lives in a region's bump. `hashbrown` rather than
/// `std::collections::HashMap` for the custom-allocator seam, which std has no stable equivalent of;
/// the hash function and probe cost are the same either way, since std's map *is* a hashbrown table.
///
/// Covers both bump-backed shapes: the frozen indexes [`BumpAllocator::frozen_table`] builds, and a
/// table built over the raw [`Allocator`] seam and kept writable — the latter owing its own
/// no-drop-glue assert at the declaration that names its entry types.
pub type BumpBackedMap<'b, K, V> = HashMap<K, V, DefaultHashBuilder, BumpAllocator<'b>>;

/// The bump door itself, hung on the fold engines' placement capability — see the module docs for the
/// confinement and no-erasure arguments it rests on.
impl<'b, W: StorageProfile> FoldedPlacement<'b, W> {
    /// **Fold the operands' reach and bump the bytes, in one verb.**
    ///
    /// One door, generic over the stored family `K`: a bytes-only allocation (a string literal, a
    /// keyword slice) is this same verb with an **empty** operand list, whose reach is then empty
    /// *structurally* — there is no coverage claim for a call site to write, correctly or otherwise.
    /// The product is an [`Opened<'b, K, Carrier<F>>`](super::Opened): never a bare region reference,
    /// never a `(value, reach)` pair. It rests through [`Opened::reseal`] like any other open.
    ///
    /// Reach composition mirrors [`Sectioned::build`](super::Sectioned::build)'s pinned-cell arm. Per
    /// operand the source is its own description's members, plus its home region under the run-level
    /// self rule — folded in only when the operand lives *somewhere else*, since an operand already
    /// resident in the destination is covered by the destination's own liveness. Composition and
    /// retention run **before** `construct`, so the retention keeping the operands' regions alive is
    /// established before the value depending on them exists.
    ///
    /// The embedder names no reach vocabulary at any point: reach is a consequence of which carriers
    /// were passed in, so it is neither derivable nor forgeable at the call site. The `construct`
    /// closure is where the module doc's brand argument bites — the [`BumpAllocator<'b>`] it receives
    /// is over the destination region, and the two `compile_fail` guards below are that argument's
    /// tests.
    ///
    /// Operands are homogeneous in `V` — the same limitation
    /// [`StepContext::alloc_with`](super::StepContext::alloc_with) carries; heterogeneous operands
    /// compose through the [`And`](super::And) family or nested door calls.
    ///
    /// ```
    /// use std::rc::Rc;
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, FixtureProfile, RefFamily, RegionCart, StrFamily};
    /// use workgraph::witnessed::{Carrier, Delivered, StepContext};
    ///
    /// let cart = fresh_cart();
    /// let ctx: StepContext<RegionCart> = StepContext::new(Rc::clone(&cart));
    /// // No operands: bytes only, and the product's reach is empty structurally.
    /// let built: Delivered<StrFamily, Carrier<RegionCart>, RegionCart> = ctx
    ///     .alloc_with::<FixtureProfile, StrFamily, RefFamily>(&[], |placement, _views| {
    ///         placement
    ///             .fold_and_bump::<StrFamily, RefFamily, RegionCart>(&[], |bump, _operands| {
    ///                 bump.text("hello")
    ///             })
    ///             .value()
    ///     });
    /// assert_eq!(built.open(|text| text.to_owned()), "hello");
    /// // `StrFamily` declares only its lifetime shape: the bump needs no storage policy.
    /// ```
    ///
    /// ```
    /// use std::rc::Rc;
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, FixtureProfile, RefFamily, RegionCart, StrFamily};
    /// use workgraph::witnessed::{Carrier, Delivered, StepContext};
    ///
    /// let cart = fresh_cart();
    /// let ctx: StepContext<RegionCart> = StepContext::new(Rc::clone(&cart));
    /// // A second call takes the first's product as an operand — composition within one brand.
    /// let built: Delivered<StrFamily, Carrier<RegionCart>, RegionCart> = ctx
    ///     .alloc_with::<FixtureProfile, StrFamily, RefFamily>(&[], |placement, _views| {
    ///         let head = placement
    ///             .fold_and_bump::<StrFamily, RefFamily, RegionCart>(&[], |bump, _| bump.text("koan"));
    ///         placement
    ///             .fold_and_bump::<StrFamily, StrFamily, RegionCart>(&[&head], |bump, operands| {
    ///                 bump.text(&format!("{}!", operands[0]))
    ///             })
    ///             .value()
    ///     });
    /// assert_eq!(built.open(|text| text.to_owned()), "koan!");
    /// ```
    ///
    /// ```compile_fail
    /// use std::rc::Rc;
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, FixtureProfile, RefFamily, RegionCart, StrFamily};
    /// use workgraph::witnessed::StepContext;
    ///
    /// let cart = fresh_cart();
    /// let ctx: StepContext<RegionCart> = StepContext::new(Rc::clone(&cart));
    /// let outside = String::from("ambient");
    /// // Try to build the stored value out of an enclosing borrow — rejected by the fold brand:
    /// // `outside`'s lifetime has no outlives relation to the universally-quantified `'b`.
    /// let _ = ctx.alloc_with::<FixtureProfile, StrFamily, RefFamily>(&[], |placement, _views| {
    ///     placement
    ///         .fold_and_bump::<StrFamily, RefFamily, RegionCart>(&[], |_bump, _operands| &outside[..])
    ///         .value()
    /// });
    /// ```
    ///
    /// ```compile_fail
    /// use std::rc::Rc;
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, FixtureProfile, RefFamily, RegionCart, StrFamily};
    /// use workgraph::witnessed::StepContext;
    ///
    /// let cart = fresh_cart();
    /// let ctx: StepContext<RegionCart> = StepContext::new(Rc::clone(&cart));
    /// let mut escaped: Option<&str> = None;
    /// // Try to smuggle the product's view past the enclosing fold closure — rejected by `for<'b>`.
    /// let _ = ctx.alloc_with::<FixtureProfile, StrFamily, RefFamily>(&[], |placement, _views| {
    ///     let product = placement
    ///         .fold_and_bump::<StrFamily, RefFamily, RegionCart>(&[], |bump, _| bump.text("hello"));
    ///     escaped = Some(product.value());
    ///     product.value()
    /// });
    /// println!("{}", escaped.unwrap());
    /// ```
    pub fn fold_and_bump<K, V, F>(
        self,
        operands: &[&Opened<'b, V, Carrier<F>>],
        construct: impl FnOnce(BumpAllocator<'b>, &[V::At<'b>]) -> K::At<'b>,
    ) -> Opened<'b, K, Carrier<F>>
    where
        K: Reattachable + DropFree,
        V: Reattachable + DropFree,
        V::At<'b>: Copy,
        F: PinsRegion + RegionOwner<Region = Region<W>> + 'static,
        W: StorageProfile<FrameOwner = F>,
    {
        let dest = self.handle();
        // An operand resident in `dest` is covered by `dest`'s own liveness, so naming its home
        // would make every product holding a co-resident operand read as borrowing its own home. One
        // resident elsewhere is a genuine cross-region borrow, and nothing else would pin its host.
        let sources: Vec<PinBundle<F>> = operands
            .iter()
            .map(|operand| {
                let mut source = operand.with_reach(ReachDescription::to_bundle);
                if !operand.with_home_region(|home| ptr::eq(home, dest.region())) {
                    source.insert(operand.witness().home_owner());
                }
                source
            })
            .collect();
        let source_refs: Vec<&PinBundle<F>> = sources.iter().collect();

        // Mint BEFORE the value exists: the mint's own retention keeps the operands' regions alive,
        // and it has to be established before anything depending on them is built.
        let description = ReachDescription::mint_resident(dest, &source_refs);

        let views: Vec<V::At<'b>> = operands.iter().map(|operand| operand.value()).collect();
        let built = construct(dest.allocator(), &views);
        // `Opened::adopted`'s precondition is discharged above: the door retained the pins covering
        // `'b` before handing the value over, so the value↔reach pairing is still library-minted.
        Opened::adopted(built, Carrier::new(description))
    }
}
