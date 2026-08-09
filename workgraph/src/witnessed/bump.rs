//! The **bump door**: the public path from an embedder's `Drop`-free value family into a
//! [`Region`]'s byte arena, and the [`BumpAllocator`] write surface a constructor builds through.
//!
//! A value stored here is *not* erased. The bump allocator is lifetime-free, so `'b` enters only at
//! the allocating call — which is why a bumped value may hold an `&'b` back into the very region it
//! lives in with no [`erase_to_static`](super::erase_to_static), no
//! [`Stored`](super::Stored) impl, and no residence check. The typed
//! [`FamilyArena`](super::FamilyArena) cells cannot do that: their slot type is `K::At<'static>`, so
//! a region-self-referential value reaching one has to have its borrow erased and vetted back.
//!
//! Two arguments carry the door, stated here once so the method docs point at them rather than
//! restating them.
//!
//! **Confinement is the brand, not the capability's privacy.** The door is a method on
//! [`FoldedPlacement`], the fold engines' unforgeable placement capability, and `'b` is the
//! *enclosing* fold's brand — the same one [`FoldedPlacement::alloc_resident_folded`] discharges its
//! residence obligation against. A constructor cannot capture an ambient `&'x` into the value it
//! builds, because `'x` has no outlives relation to a universally-quantified `'b`; and the door's
//! product, an [`Opened<'b, …>`](super::Opened), cannot leave the enclosing fold closure for the
//! same reason. Bare inside the brand, bundled on the way out. A door that minted a *fresh*
//! `for<'b>` brand of its own could not name that brand in its return type, so its product could
//! never escape the call at all — and a value built at a fresh brand could not be written into a
//! bump borrowed at the enclosing one without a lifetime retype, which is exactly the erasure this
//! door exists not to do. That the brand alone does the confining is why the write surface handed to
//! a constructor is the plain [`BumpAllocator`] any handle-holder can mint, rather than a
//! door-minted type of its own: a mint restriction on top would guard nothing `'b` does not.
//!
//! **No erasure, because the operands already live at the door's brand.** Operands arrive as
//! [`Opened<'b, V, Carrier<F>>`](super::Opened) — their values are *already* `V::At<'b>`, so the door
//! re-anchors nothing: no [`Erased::reattach`](super::Erased::reattach), no retype, no `unsafe`. An
//! `Opened` borrows at `'b` under the pin that opened it, and that borrow **is** the holder-rule
//! coverage [`ReachDescription::to_bundle`] needs, so the door takes no pin argument and the call
//! site names no [`StepCoverage`](super::StepCoverage), no [`ReachDescription`] and no
//! [`PinBundle`](super::PinBundle). An operand living in another region enters the brand through the
//! enclosing fold engine ([`StepContext::alloc_with`](super::StepContext::alloc_with)), which is
//! where cross-region reach ownership already flows; the door composes *within* the brand.

use std::alloc::Layout;
use std::ptr;
use std::ptr::NonNull;

use allocator_api2::alloc::{AllocError, Allocator};
use bumpalo::Bump;

use super::{
    Carrier, DropFree, FoldedPlacement, Opened, PinBundle, PinsRegion, ReachDescription,
    Reattachable, Region, RegionOwner, StorageProfile,
};

/// **The region's bump, as one handle carrying both storage tiers** — the guarded verbs
/// ([`value`](Self::value), [`slice`](Self::slice), [`text`](Self::text), [`place`](Self::place))
/// every write-once byte goes through, and the raw [`Allocator`] impl a **mutable** collection is
/// built over so its backing allocation lands in the region's own chunks instead of the global heap.
///
/// The verbs live here and nowhere else. Every surface that can reach a region's bytes —
/// [`RegionHandle::allocator`](super::RegionHandle::allocator),
/// [`FoldedPlacement::allocator`], an embedder's own brand veneer — hands back this one type rather
/// than restating a verb set of its own, so the `Copy` guard and the rationale behind it are written
/// once. Their primitives are std shapes only — a `Copy` value, a `Copy` slice, a `str` — so the
/// door names no workload type and grows no per-family verb, and each hands back a shared `&'b`,
/// never the `&mut` the bump itself returns: a bumped value is region state a holder names, not one
/// it owns.
///
/// `Copy`, and the field is private, so those `allocator()` accessors are the only mint and `'b`
/// stays the region's own brand: nothing built through one can outlive the region whose bytes it
/// holds. Where `'b` is a rank-2 fold brand, that same lifetime is the confinement — which is why
/// the allocator needs no mint privacy of its own to serve as a fold's write surface.
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

// Manual impls: `Copy` grants nothing new, since the handle names no more than the `&'b Bump` it
// wraps and `'b` already bounds every use.
impl Clone for BumpAllocator<'_> {
    fn clone(&self) -> Self {
        *self
    }
}
impl Copy for BumpAllocator<'_> {}

/// Every method forwards to `&Bump`'s own [`Allocator`] impl rather than leaning on the trait's
/// defaults: bumpalo's `grow` extends the newest chunk in place when the allocation is the last one
/// out, which the default (allocate-copy-deallocate) would give up. The safety obligation on each
/// method is discharged by the delegate, which receives exactly the arguments this one was given.
unsafe impl Allocator for BumpAllocator<'_> {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        Allocator::allocate(&self.0, layout)
    }

    fn allocate_zeroed(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        Allocator::allocate_zeroed(&self.0, layout)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        Allocator::deallocate(&self.0, ptr, layout)
    }

    unsafe fn grow(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        Allocator::grow(&self.0, ptr, old_layout, new_layout)
    }

    unsafe fn grow_zeroed(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        Allocator::grow_zeroed(&self.0, ptr, old_layout, new_layout)
    }

    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        Allocator::shrink(&self.0, ptr, old_layout, new_layout)
    }
}

impl<'b> BumpAllocator<'b> {
    /// Wrap `bump` — crate-internal, so [`RegionHandle::allocator`](super::RegionHandle::allocator)
    /// is the only way an embedder
    /// reaches one and no caller can pair the allocator with a bump that is not a region's.
    pub(crate) fn over(bump: &'b Bump) -> Self {
        BumpAllocator(bump)
    }

    /// Bump one `Copy` value into the region and hand back the co-located `&'b`.
    ///
    /// `T: Copy` is the static proxy for "no destructor to skip": the bump releases its chunks whole
    /// and runs no `Drop`, so a `Drop`-bearing `T` would silently leak its owned contents.
    /// "`Drop`-free" has no expressible bound; `Copy` is the honest approximation, and every family
    /// queued behind this door satisfies it by construction. A value that is glue-free without being
    /// `Copy` takes [`place`](Self::place), which trades the bound for a per-site assert.
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

    /// Copy a `Copy` slice into the region and hand back the co-located `&'b [T]` — the shape an
    /// operator group's member block or an expression's part list takes. Same `T: Copy` rationale as
    /// [`value`](Self::value).
    pub fn slice<T: Copy>(self, items: &[T]) -> &'b [T] {
        self.0.alloc_slice_copy(items)
    }

    /// Copy `text` into the region and hand back the co-located `&'b str`. A `str` is a slice of
    /// `Copy` bytes under a UTF-8 invariant, so it carries no destructor either and needs no bound to
    /// say so.
    pub fn text(self, text: &str) -> &'b str {
        self.0.alloc_str(text)
    }

    /// [`value`](Self::value) for a `T` that is glue-free **without** being `Copy`, guarded by a
    /// monomorphization-time assert instead of a bound: a `T` with drop glue is a build error at the
    /// call that named it.
    ///
    /// The intended user is a frozen table's header — a `hashbrown` map owns its bucket array, so it
    /// has a `Drop`, and a [`ManuallyDrop`](std::mem::ManuallyDrop) wrapper is what makes it pass
    /// this assert. That wrapper is a per-site *declaration* that the suppressed destructor had
    /// nothing to do: it is lossless exactly when the table's elements are themselves glue-free (the
    /// declaration site says so with its own const assert) and its buckets are bump memory the region
    /// releases whole. Because that reasoning cannot be checked here, the plain-value verb keeps the
    /// stricter `Copy` tier and only a caller willing to write the wrapper reaches this one.
    ///
    /// ```compile_fail
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, FixtureProfile};
    /// use workgraph::witnessed::RegionHandle;
    ///
    /// let cart = fresh_cart();
    /// let handle: RegionHandle<'_, FixtureProfile> = RegionHandle::from_owner(&*cart);
    /// // A bare `String` has drop glue — rejected by the const assert, not by a bound.
    /// let _stored: &String = handle.allocator().place(String::from("leaks"));
    /// ```
    pub fn place<T>(self, value: T) -> &'b T {
        const {
            assert!(
                !std::mem::needs_drop::<T>(),
                "a bump-placed value must carry no drop glue: the bump runs no destructor",
            )
        };
        self.0.alloc(value)
    }
}

/// The bump door itself, hung on the fold engines' placement capability — see the module docs for the
/// confinement and no-erasure arguments it rests on.
impl<'b, W: StorageProfile> FoldedPlacement<'b, W> {
    /// **Fold the operands' reach and bump the bytes, in one verb.** Compose every operand's reach
    /// into this fold's destination region, retain it there, then run `construct` — which builds the
    /// stored value through a [`BumpAllocator`] over that region and the operands' opened views — and
    /// hand back the
    /// product as one bundled carrier.
    ///
    /// One door, generic over the stored family `K`: a bytes-only allocation (a string literal, a
    /// keyword slice) is this same verb with an **empty** operand list, whose reach is then empty
    /// *structurally* — there is no coverage claim for a call site to write, correctly or otherwise.
    /// The product is an [`Opened<'b, K, Carrier<F>>`](super::Opened): never a bare region reference, never a
    /// `(value, reach)` pair. It rests through [`Opened::reseal`] like any other open.
    ///
    /// Reach composition mirrors [`Sectioned::build`](super::Sectioned::build)'s pinned-cell arm. Per
    /// operand the source is its own description's members, plus its home region under the run-level
    /// self rule — folded in only when the operand lives *somewhere else*, since an operand already
    /// resident in the destination is covered by the destination's own liveness. Composition and
    /// retention run **before** `construct`, so the retention keeping the operands' regions alive is
    /// established before the value depending on them exists.
    ///
    /// The embedder names no reach vocabulary at any point: reach is a consequence of which carriers
    /// were passed in, so it is neither derivable nor forgeable at the call site. No `unsafe`, and no
    /// lifetime erasure — the operands' values are already at `'b`, and the bump stores them there.
    ///
    /// The `construct` closure is what ties the write surface to this fold's brand: the
    /// [`BumpAllocator<'b>`] it receives is over the destination region, and `'b` is unnameable
    /// outside the call, so neither the allocator nor anything written through it escapes. Operands
    /// are homogeneous in `V` — the same
    /// limitation [`StepContext::alloc_with`](super::StepContext::alloc_with) carries; heterogeneous
    /// operands compose through the [`And`](super::And) family or nested door calls.
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
    ///     .alloc_with_handle::<FixtureProfile, StrFamily, RefFamily>(&[], |placement, _views| {
    ///         placement
    ///             .fold_and_bump::<StrFamily, RefFamily, RegionCart>(&[], |bump, _operands| {
    ///                 bump.text("hello")
    ///             })
    ///             .value()
    ///     });
    /// assert_eq!(built.open(|text| text.to_owned()), "hello");
    /// // `StrFamily` has no `Stored` impl at all: the bump needs none.
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
    ///     .alloc_with_handle::<FixtureProfile, StrFamily, RefFamily>(&[], |placement, _views| {
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
    /// let _ = ctx.alloc_with_handle::<FixtureProfile, StrFamily, RefFamily>(&[], |placement, _views| {
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
    /// let _ = ctx.alloc_with_handle::<FixtureProfile, StrFamily, RefFamily>(&[], |placement, _views| {
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
        // Per operand: its own exact reach, plus its home region under the run-level self rule. An
        // operand resident in `dest` is covered by `dest`'s own liveness, so naming it would make
        // every product holding a co-resident operand read as borrowing its own home. One resident
        // elsewhere is a genuine cross-region borrow, and nothing else would pin its host.
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
