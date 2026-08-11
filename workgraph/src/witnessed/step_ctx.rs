//! [`StepContext`] — the step construction context: a library-owned handle a step loop hands to a
//! finish, whose two verbs are guarantees 3 and 5 of the scheduler-library design made structural.
//! [`StepContext::alloc`] builds a value reachable only through the held frame's own region (reach =
//! own region only, by the `yoke` brand); [`StepContext::alloc_with`] folds a set of delivered dep
//! envelopes in first, so the built value's carrier names every dep's whole reach — its home region
//! among the ordinary members of the envelope's pins —
//! and a dep's payload is viewable only inside the build closure's brand — it cannot be smuggled out
//! and stored unwitnessed.

use std::marker::PhantomData;
use std::rc::Rc;

use super::{
    Carrier, Delivered, DropFree, FoldToken, FoldedPlacement, PinsRegion, Reattachable, Region,
    RegionHandle, RegionOwner, StepCoverage, StorageProfile, Witnessed,
};

/// The step construction context — handed to a finish by the step loop, whose held region owner is
/// what makes [`Self::region`] infallible (guarantee 4, reused). Cheap to clone (an `Rc` clone).
pub struct StepContext<F: RegionOwner> {
    frame: Rc<F>,
}

impl<F: RegionOwner> Clone for StepContext<F> {
    fn clone(&self) -> Self {
        StepContext {
            frame: Rc::clone(&self.frame),
        }
    }
}

impl<F: RegionOwner> StepContext<F> {
    /// Wrap the step loop's held region owner.
    pub fn new(frame: Rc<F>) -> Self {
        StepContext { frame }
    }

    /// The consumer's live region — infallible, since the context holds the owner that pins it.
    pub fn region(&self) -> &F::Region {
        RegionOwner::region(&*self.frame)
    }

    /// The held owner, for callers that need the `Rc` itself.
    pub fn frame(&self) -> Rc<F> {
        Rc::clone(&self.frame)
    }
}

impl<F: RegionOwner + PinsRegion + 'static> StepContext<F> {
    /// [`Self::alloc`] handing `build` the bare `&F::Region` instead of its [`RegionHandle`] — the
    /// raw yoke the public door adapts. Crate-internal: every embedder allocation goes through the
    /// handle capability.
    pub(in crate::witnessed) fn alloc_in_region<T, P>(
        &self,
        build: impl for<'b> FnOnce(&'b F::Region) -> T::At<'b>,
    ) -> Witnessed<T, Carrier<F>>
    where
        T: Reattachable + DropFree,
        P: StorageProfile<FrameOwner = F> + 'static,
        F: RegionOwner<Region = Region<P>>,
    {
        Witnessed::<T, Rc<F>>::yoke(Rc::clone(&self.frame), build).into_reference_only::<P>()
    }

    /// [`Self::alloc_with`] handing `build` the bare region plus a [`FoldToken`] instead of a
    /// [`FoldedPlacement`] over the region's handle — the raw dep fold the public door adapts.
    pub(in crate::witnessed) fn alloc_with_in_region<T, V, P>(
        &self,
        deps: &[&Delivered<V, Carrier<F>, F>],
        build: impl for<'b> FnOnce(&'b F::Region, &'b [V::At<'b>], FoldToken<'b>) -> T::At<'b>,
    ) -> Delivered<T, Carrier<F>, F>
    where
        T: Reattachable + DropFree,
        V: Reattachable + DropFree,
        for<'b> V::At<'b>: Copy,
        P: StorageProfile<FrameOwner = F> + 'static,
        F: RegionOwner<Region = Region<P>>,
    {
        // The accumulator is enveloped over this context's own frame — the region it is yoked into —
        // so each fold step composes into an envelope rather than a carrier plus a loose bundle.
        let acc0 = Delivered::seal(
            self.alloc_in_region::<AllocViews<V, F::Region>, P>(|region| (region, &[][..])),
            Rc::clone(&self.frame),
            StepCoverage::empty(),
        );
        let acc = deps.iter().fold(acc0, |acc, dep| {
            dep.transfer_into::<AllocViews<V, F::Region>, AllocViews<V, F::Region>, P>(
                acc,
                // The view rides the accumulator un-copied, so the built value genuinely
                // borrows into every region the dep does: the predicate releases none.
                |_product, _region| true,
                fold_dep_view::<V, P>(),
            )
        });
        // The projection re-anchors under the accumulator's own pins and re-seals under the same
        // witness, so the accumulated envelope's coverage — this context's frame among its members,
        // unioned in at `acc0` — carries over unchanged as the built carrier's owned coverage.
        acc.project::<T>(finalize_alloc_with::<F, T, V>(build))
    }

    /// Build a value reachable only through the held frame's own region: reach = own region only,
    /// so the carrier references a description with empty members hosted in that same region — its
    /// liveness is the frame the step loop holds (guarantee 4), then the retention hold once
    /// finalized. `build` receives the region's [`RegionHandle`], the one allocation capability, and
    /// the `for<'b>` brand on it admits only region-derived or owned references — so purity is
    /// structural rather than asserted: the value is yoked from the frame's own region and only then
    /// re-bundled under the pin-free carrier.
    ///
    /// ```
    /// use std::rc::Rc;
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, FixtureProfile, RefFamily, RegionCart};
    /// use workgraph::witnessed::{Carrier, Sealed, StepContext, Witnessed};
    ///
    /// static SEVEN: u32 = 7;
    /// let cart = fresh_cart();
    /// let ctx: StepContext<RegionCart> = StepContext::new(Rc::clone(&cart));
    /// let w: Witnessed<RefFamily, Carrier<RegionCart>> =
    ///     ctx.alloc::<FixtureProfile, RefFamily>(|_handle| &SEVEN);
    /// // The carrier pins nothing, so a read names its coverage: the frame it was yoked into.
    /// assert_eq!(Sealed::seal(w).open_with(&cart, |r| *r), 7);
    /// ```
    ///
    /// ```compile_fail
    /// use std::rc::Rc;
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, FixtureProfile, RefFamily, RegionCart};
    /// use workgraph::witnessed::{Carrier, StepContext, Witnessed};
    ///
    /// let outside: u32 = 7;
    /// let cart = fresh_cart();
    /// let ctx: StepContext<RegionCart> = StepContext::new(cart);
    /// // Try to capture a non-region borrow into the closure — rejected by the `for<'b>` brand.
    /// let _: Witnessed<RefFamily, Carrier<RegionCart>> =
    ///     ctx.alloc::<FixtureProfile, RefFamily>(|_handle| &outside);
    /// ```
    pub fn alloc<P, T>(
        &self,
        build: impl for<'b> FnOnce(RegionHandle<'b, P>) -> T::At<'b>,
    ) -> Witnessed<T, Carrier<F>>
    where
        P: StorageProfile<FrameOwner = F> + 'static,
        F: RegionOwner<Region = Region<P>>,
        T: Reattachable + DropFree,
    {
        self.alloc_in_region::<T, P>(|region| build(RegionHandle::new(region)))
    }

    /// Build a value whose carrier names the held frame's own region implicitly plus every named
    /// dep's whole reach, folded by the call shape (guarantee 5). Each dep arrives as its delivery
    /// envelope, and each fold is an envelope-bearing
    /// [`transfer_into`](Delivered::transfer_into) claiming the dep's own pins — the dep's payload
    /// keeps living in its producer's region while its view is embedded, so the producer's frame
    /// (an ordinary member of those pins) composes into the minted set. A dep's payload is handed
    /// to `build` only inside the shared `for<'b>` brand — the [`compile_fail`] guard below pins
    /// that a view cannot be smuggled out of the closure and stored unwitnessed.
    ///
    /// `build` receives a [`FoldedPlacement`] over the destination region: it carries the
    /// destination handle and is itself the fold-brand proof, minted over the same region this door
    /// folds the deps' reach into, so the two never have to be paired by hand.
    ///
    /// ```
    /// use std::rc::Rc;
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, FixtureProfile, RefFamily, RegionCart};
    /// use workgraph::witnessed::{Carrier, Delivered, RegionHandle, StepContext};
    ///
    /// static TEN: u32 = 10;
    /// let dep_cart = fresh_cart();
    /// let dep: Delivered<RefFamily, Carrier<RegionCart>, RegionCart> =
    ///     RegionHandle::<FixtureProfile>::from_owner(&*dep_cart).deliver_resident::<RefFamily>(&TEN);
    ///
    /// let cart = fresh_cart();
    /// let ctx: StepContext<RegionCart> = StepContext::new(Rc::clone(&cart));
    /// let built: Delivered<RefFamily, Carrier<RegionCart>, RegionCart> =
    ///     ctx.alloc_with::<FixtureProfile, RefFamily, RefFamily>(&[&dep], |_placement, views| views[0]);
    /// assert_eq!(built.open(|r| *r), 10);
    /// ```
    ///
    /// ```compile_fail
    /// use std::rc::Rc;
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, FixtureProfile, RefFamily, RegionCart};
    /// use workgraph::witnessed::{Carrier, Delivered, RegionHandle, StepContext};
    ///
    /// static TEN: u32 = 10;
    /// let dep_cart = fresh_cart();
    /// let dep: Delivered<RefFamily, Carrier<RegionCart>, RegionCart> =
    ///     RegionHandle::<FixtureProfile>::from_owner(&*dep_cart).deliver_resident::<RefFamily>(&TEN);
    ///
    /// let cart = fresh_cart();
    /// let ctx: StepContext<RegionCart> = StepContext::new(cart);
    /// let mut escaped: Option<&u32> = None;
    /// // Try to smuggle a dep view OUT of `alloc_with`'s closure — rejected by the `for<'b>` brand.
    /// let _built: Delivered<RefFamily, Carrier<RegionCart>, RegionCart> = ctx.alloc_with::<FixtureProfile, RefFamily, RefFamily>(&[&dep], |_placement, views| {
    ///     escaped = Some(views[0]);
    ///     views[0]
    /// });
    /// println!("{}", *escaped.unwrap());
    /// ```
    pub fn alloc_with<P, T, V>(
        &self,
        deps: &[&Delivered<V, Carrier<F>, F>],
        build: impl for<'b> FnOnce(FoldedPlacement<'b, P>, &'b [V::At<'b>]) -> T::At<'b>,
    ) -> Delivered<T, Carrier<F>, F>
    where
        P: StorageProfile<FrameOwner = F> + 'static,
        F: RegionOwner<Region = Region<P>>,
        T: Reattachable + DropFree,
        V: Reattachable + DropFree,
        for<'b> V::At<'b>: Copy,
    {
        self.alloc_with_in_region::<T, V, P>(deps, |region, views, token| {
            // The accumulator was yoked over this frame's own region and `alloc_with` folds every
            // dep's reach into it, so a placement over its handle is honestly minted here; the fold
            // token is subsumed by the placement as the `'b` brand proof.
            let _ = token;
            build(FoldedPlacement::mint(RegionHandle::new(region)), views)
        })
    }
}

/// [`StepContext::alloc_with`]'s per-dep fold step, factored into its own generic function that
/// carries no `V::At<'static>: Copy` bound. Binding a `V::At<'b>` view directly inside a scope that
/// *also* carries that bound (as `alloc_with` must, to fold the envelopes) trips a rustc region-
/// inference gap over GAT projections — a fresh, non-`'static` instantiation gets spuriously
/// required to outlive `'static`. Building the closure here, where no such bound is in scope, and
/// handing back only the finished opaque `impl for<'b> FnOnce(..)` value sidesteps it: `alloc_with`
/// itself never binds a `V::At<'b>` value, only moves this closure around.
///
/// The folded views ride the accumulator un-copied — sound because each fold claimed the dep's own
/// pins, so every view's producer frame is a member of the accumulator's minted set, pinned by the
/// consumer's own arena for the built value's life.
#[allow(clippy::type_complexity)]
fn fold_dep_view<V, P>() -> impl for<'b> FnOnce(
    V::At<'b>,
    (&'b Region<P>, &'b [V::At<'b>]),
    FoldedPlacement<'b, P>,
) -> (&'b Region<P>, &'b [V::At<'b>])
where
    V: Reattachable,
    P: StorageProfile + 'static,
    for<'b> V::At<'b>: Copy,
{
    |view, (region, views), placement| {
        // Re-bump the accumulated views plus this one into the destination region. The step's
        // allocations interleave with the reach mints each fold composes, so extending the previous
        // slice in place is not available; the copy is the price of a slot the Copy tier can hold.
        let mut grown = Vec::with_capacity(views.len() + 1);
        grown.extend_from_slice(views);
        grown.push(view);
        (region, placement.allocator().slice(&grown))
    }
}

/// [`StepContext::alloc_with`]'s final build step, factored out for the same reason as
/// [`fold_dep_view`]: it destructures the accumulator's `Vec<V::At<'b>>`, so it must be built outside
/// `alloc_with`'s `V::At<'static>: Copy` scope.
#[allow(clippy::type_complexity)]
fn finalize_alloc_with<F: RegionOwner, T: Reattachable, V: Reattachable>(
    build: impl for<'b> FnOnce(&'b F::Region, &'b [V::At<'b>], FoldToken<'b>) -> T::At<'b>,
) -> impl for<'b> FnOnce((&'b F::Region, &'b [V::At<'b>]), FoldToken<'b>) -> T::At<'b> {
    |(region, views), token| build(region, views, token)
}

/// `alloc_with`'s fold accumulator: the context's own region reference paired with the dep views
/// folded in so far, re-anchored as one carrier. Layout-invariant in `'r`: a reference and a slice
/// of a layout-invariant family are each layout-invariant, so the pair is too — the
/// [`Reattachable`] contract, discharged componentwise, the same justification as [`super::And`].
///
/// The views ride a **region-bumped slice** rather than an owned `Vec`, which is what puts the
/// accumulator in the Copy tier: the accumulator *rests* between fold steps (each
/// [`Delivered::transfer_into`] seals it), and the Copy tier's dormant slot is glue-free, so an
/// owned buffer resting there would be dropped by nobody. A bumped slice needs no destructor — the
/// region releases its chunks whole — so the family is honestly [`DropFree`].
struct AllocViews<V, R: ?Sized>(PhantomData<(V, *const R)>);

// SAFETY: `(&'r R, &'r [V::At<'r>])` is one type up to `'r` when `V` is — see the type's doc
// comment. `R: 'static` is required for the GAT to type-check for every `'r` (a bound the concrete
// `Region` types this is instantiated with — lifetime-free arena handles — trivially satisfy).
unsafe impl<V: Reattachable, R: ?Sized + 'static> Reattachable for AllocViews<V, R> {
    type At<'r> = (&'r R, &'r [V::At<'r>]);
}

/// A pair of shared references needs no drop, so the accumulator rests in the Copy tier.
impl<V, R: ?Sized> DropFree for AllocViews<V, R> {}

// SAFETY: the handle authorizes allocation into `self.0`'s own region — exactly the region a
// `Carrier` composed against this accumulator's live form re-homes into. Generic over the
// accumulated second component `T` (an `alloc_with`-family's `Vec<V::At<'b>>`, for any `V`) since
// only the region reference determines the handle.
unsafe impl<'b, T, P: StorageProfile> super::HasRegionHandle<'b, P> for (&'b Region<P>, T) {
    fn region_handle(&self) -> RegionHandle<'b, P> {
        RegionHandle::new(self.0)
    }
}
