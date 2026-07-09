//! [`StepContext`] — the step construction context: a library-owned handle a step loop hands to a
//! finish, whose two verbs are guarantees 3 and 5 of the scheduler-library design made structural.
//! [`StepContext::alloc`] builds a value reachable only through the held frame's own region (reach =
//! own region only, by the `yoke` brand); [`StepContext::alloc_with`] folds a set of delivered dep
//! envelopes in first, so the built value's carrier names every dep's reach *and* residence host,
//! and a dep's payload is viewable only inside the build closure's brand — it cannot be smuggled out
//! and stored unwitnessed.

use std::marker::PhantomData;
use std::rc::Rc;

use super::{
    Carrier, Delivered, PinsRegion, Reattachable, Region, RegionHandle, RegionOwner, Residence,
    StorageProfile, Witnessed,
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
    /// Build a value reachable only through the held frame's own region: reach = own region only,
    /// so the carrier is the empty reference-only [`Carrier`] — its liveness is the frame the step
    /// loop holds (guarantee 4), then the retention hold once finalized. The `for<'b>` brand on
    /// `build` admits only region-derived or owned references, so purity is structural rather than
    /// asserted: the value is yoked from the frame's own region and only then re-bundled under the
    /// pin-free carrier.
    ///
    /// ```
    /// use std::rc::Rc;
    /// use workgraph::witnessed::doctest_fixture::{Cart, RefFamily};
    /// use workgraph::witnessed::{Carrier, StepContext, Witnessed};
    ///
    /// let cart = Rc::new(Cart(vec![1, 2, 3]));
    /// let ctx: StepContext<Cart> = StepContext::new(Rc::clone(&cart));
    /// let w: Witnessed<RefFamily, Carrier<Cart>> = ctx.alloc(|region| &region[0]);
    /// assert_eq!(w.with_pinned(&cart, |r| **r), 1);
    /// ```
    ///
    /// ```compile_fail
    /// use std::rc::Rc;
    /// use workgraph::witnessed::doctest_fixture::{Cart, RefFamily};
    /// use workgraph::witnessed::{Carrier, StepContext, Witnessed};
    ///
    /// let outside: u32 = 7;
    /// let cart = Rc::new(Cart(vec![1, 2, 3]));
    /// let ctx: StepContext<Cart> = StepContext::new(cart);
    /// // Try to capture a non-region borrow into the closure — rejected by the `for<'b>` brand.
    /// let _: Witnessed<RefFamily, Carrier<Cart>> = ctx.alloc(|_region| &outside);
    /// ```
    pub fn alloc<T: Reattachable>(
        &self,
        build: impl for<'b> FnOnce(&'b F::Region) -> T::At<'b>,
    ) -> Witnessed<T, Carrier<F>> {
        Witnessed::<T, Rc<F>>::yoke(Rc::clone(&self.frame), build).rewitness(Carrier::default())
    }

    /// Build a value whose carrier names the held frame's own region implicitly plus every named
    /// dep's reach **and residence host**, folded by the call shape (guarantee 5). Each dep arrives
    /// as its delivery envelope, and each fold is an envelope-bearing
    /// [`transfer_into`](Delivered::transfer_into) at [`Residence::Kept`] — the dep's payload keeps
    /// living in its producer's region while its view is embedded, so the producer host
    /// materializes as a member of the minted set. A dep's payload is handed to `build` only inside
    /// the shared `for<'b>` brand — the [`compile_fail`] guard below pins that a view cannot be
    /// smuggled out of the closure and stored unwitnessed.
    ///
    /// ```
    /// use std::rc::Rc;
    /// use workgraph::witnessed::doctest_fixture::{fresh_region, RefFamily, RegionCart};
    /// use workgraph::witnessed::{Carrier, Delivered, StepContext, Witnessed};
    ///
    /// static TEN: u32 = 10;
    /// let dep_cart = Rc::new(RegionCart(fresh_region()));
    /// let dep: Delivered<RefFamily, Carrier<RegionCart>, RegionCart> = Delivered::seal(
    ///     Witnessed::<RefFamily, Carrier<RegionCart>>::resident(&TEN),
    ///     Rc::clone(&dep_cart),
    /// );
    ///
    /// let cart = Rc::new(RegionCart(fresh_region()));
    /// let ctx: StepContext<RegionCart> = StepContext::new(Rc::clone(&cart));
    /// let w: Witnessed<RefFamily, Carrier<RegionCart>> =
    ///     ctx.alloc_with(&[&dep], |_region, views| views[0]);
    /// assert_eq!(w.with_pinned(&cart, |r| **r), 10);
    /// ```
    ///
    /// ```compile_fail
    /// use std::rc::Rc;
    /// use workgraph::witnessed::doctest_fixture::{fresh_region, RefFamily, RegionCart};
    /// use workgraph::witnessed::{Carrier, Delivered, StepContext, Witnessed};
    ///
    /// static TEN: u32 = 10;
    /// let dep_cart = Rc::new(RegionCart(fresh_region()));
    /// let dep: Delivered<RefFamily, Carrier<RegionCart>, RegionCart> = Delivered::seal(
    ///     Witnessed::<RefFamily, Carrier<RegionCart>>::resident(&TEN),
    ///     Rc::clone(&dep_cart),
    /// );
    ///
    /// let cart = Rc::new(RegionCart(fresh_region()));
    /// let ctx: StepContext<RegionCart> = StepContext::new(cart);
    /// let mut escaped: Option<&u32> = None;
    /// // Try to smuggle a dep view OUT of `alloc_with`'s closure — rejected by the `for<'b>` brand.
    /// let _: Witnessed<RefFamily, Carrier<RegionCart>> = ctx.alloc_with(&[&dep], |_region, views| {
    ///     escaped = Some(views[0]);
    ///     views[0]
    /// });
    /// println!("{}", *escaped.unwrap());
    /// ```
    pub fn alloc_with<T, V, P>(
        &self,
        deps: &[&Delivered<V, Carrier<F>, F>],
        build: impl for<'b> FnOnce(&'b F::Region, Vec<V::At<'b>>) -> T::At<'b>,
    ) -> Witnessed<T, Carrier<F>>
    where
        T: Reattachable,
        V: Reattachable,
        V::At<'static>: Copy,
        P: StorageProfile + 'static,
        F: RegionOwner<Region = Region<P>>,
        super::RegionSet<F>: super::Stored<P> + for<'r> Reattachable<At<'r> = super::RegionSet<F>>,
    {
        let acc0: Witnessed<AllocViews<V, F::Region>, Carrier<F>> = self
            .alloc::<AllocViews<V, F::Region>>(|region| (region, Vec::with_capacity(deps.len())));
        let acc = deps.iter().fold(acc0, |acc, dep| {
            dep.transfer_into::<AllocViews<V, F::Region>, AllocViews<V, F::Region>, P>(
                acc,
                Residence::Kept,
                fold_dep_view::<V, F::Region>(),
            )
        });
        acc.map_pinned::<T, _>(&self.frame, finalize_alloc_with::<F, T, V>(build))
    }

    /// [`Self::alloc`] for a frame owning a library [`Region`]: the build closure receives the
    /// region's [`RegionHandle`] instead of the bare region.
    pub fn alloc_handle<P, T>(
        &self,
        build: impl for<'b> FnOnce(RegionHandle<'b, P>) -> T::At<'b>,
    ) -> Witnessed<T, Carrier<F>>
    where
        P: StorageProfile + 'static,
        F: RegionOwner<Region = Region<P>>,
        T: Reattachable,
    {
        self.alloc::<T>(|region| build(RegionHandle::new(region)))
    }

    /// [`Self::alloc_with`] for a frame owning a library [`Region`]: same dep folding, build closure
    /// receives the [`RegionHandle`].
    pub fn alloc_with_handle<P, T, V>(
        &self,
        deps: &[&Delivered<V, Carrier<F>, F>],
        build: impl for<'b> FnOnce(RegionHandle<'b, P>, Vec<V::At<'b>>) -> T::At<'b>,
    ) -> Witnessed<T, Carrier<F>>
    where
        P: StorageProfile + 'static,
        F: RegionOwner<Region = Region<P>>,
        T: Reattachable,
        V: Reattachable,
        V::At<'static>: Copy,
        super::RegionSet<F>: super::Stored<P> + for<'r> Reattachable<At<'r> = super::RegionSet<F>>,
    {
        self.alloc_with::<T, V, P>(deps, |region, views| {
            build(RegionHandle::new(region), views)
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
/// The folded views ride the accumulator un-copied — sound because each fold ran at
/// [`Residence::Kept`], so every view's producer host is a member of the accumulator's minted set,
/// pinned by the consumer's own arena for the built value's life.
#[allow(clippy::type_complexity)]
fn fold_dep_view<V: Reattachable, R: ?Sized + 'static>() -> impl for<'b> FnOnce(
    V::At<'b>,
    (&'b R, Vec<V::At<'b>>),
    PhantomData<&'b ()>,
) -> (
    &'b R,
    Vec<V::At<'b>>,
) {
    |view, (region, mut views), _brand| {
        views.push(view);
        (region, views)
    }
}

/// [`StepContext::alloc_with`]'s final build step, factored out for the same reason as
/// [`fold_dep_view`]: it destructures the accumulator's `Vec<V::At<'b>>`, so it must be built outside
/// `alloc_with`'s `V::At<'static>: Copy` scope.
#[allow(clippy::type_complexity)]
fn finalize_alloc_with<F: RegionOwner, T: Reattachable, V: Reattachable>(
    build: impl for<'b> FnOnce(&'b F::Region, Vec<V::At<'b>>) -> T::At<'b>,
) -> impl for<'b> FnOnce((&'b F::Region, Vec<V::At<'b>>), PhantomData<&'b ()>) -> T::At<'b> {
    |(region, views), _brand| build(region, views)
}

/// `alloc_with`'s fold accumulator: the context's own region reference paired with the dep views
/// folded in so far, re-anchored as one carrier. Layout-invariant in `'r`: a reference and a `Vec` of
/// a layout-invariant family are each layout-invariant, so the pair is too — the [`Reattachable`]
/// contract, discharged componentwise, the same justification as [`super::And`].
struct AllocViews<V, R: ?Sized>(PhantomData<(V, *const R)>);

// SAFETY: `(&'r R, Vec<V::At<'r>>)` is one type up to `'r` when `V` is — see the type's doc comment.
// `R: 'static` is required for the GAT to type-check for every `'r` (a bound the concrete `Region`
// types this is instantiated with — lifetime-free arena handles — trivially satisfy).
unsafe impl<V: Reattachable, R: ?Sized + 'static> Reattachable for AllocViews<V, R> {
    type At<'r> = (&'r R, Vec<V::At<'r>>);
}

// SAFETY: the handle authorizes allocation into `self.0`'s own region — exactly the region a
// `Carrier` composed against this accumulator's live form re-homes into. Generic over the
// accumulated second component `T` (an `alloc_with`-family's `Vec<V::At<'b>>`, for any `V`) since
// only the region reference determines the handle.
unsafe impl<'b, T, P: StorageProfile> super::HasRegionHandle<'b, P> for (&'b Region<P>, T) {
    fn region_handle(&self) -> RegionHandle<'b, P> {
        RegionHandle::new(self.0)
    }
}
