//! [`StepContext`] — the step construction context: a library-owned handle a step loop hands to a
//! finish, whose two verbs are guarantees 3 and 5 of the scheduler-library design made structural.
//! [`StepContext::alloc`] builds a value reachable only through the held frame's own region (reach =
//! own region only, by the `yoke` brand); [`StepContext::alloc_with`] folds a set of dep carriers in
//! first, so the built value's reach is the frame's region unioned with every named dep's reach, and
//! a dep's payload is viewable only inside the build closure's brand — it cannot be smuggled out and
//! stored unwitnessed.

use std::marker::PhantomData;
use std::rc::Rc;

use super::{
    Reattachable, Region, RegionHandle, RegionOwner, Sealed, SetWitness, StorageProfile,
    UnionWitness, Witnessed,
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

    /// Build a value reachable only through the held frame's own region: reach = own region only.
    /// The `for<'b>` brand on `build` admits only region-derived or owned references, so purity is
    /// structural rather than asserted.
    ///
    /// ```
    /// use std::rc::Rc;
    /// use workgraph::witnessed::doctest_fixture::{Cart, RefFamily};
    /// use workgraph::witnessed::{RegionSet, StepContext, Witnessed};
    ///
    /// let cart = Rc::new(Cart(vec![1, 2, 3]));
    /// let ctx: StepContext<Cart> = StepContext::new(cart);
    /// let w: Witnessed<RefFamily, RegionSet<Cart>> = ctx.alloc(|region| &region[0]);
    /// assert_eq!(w.with(|r| **r), 1);
    /// ```
    ///
    /// ```compile_fail
    /// use std::rc::Rc;
    /// use workgraph::witnessed::doctest_fixture::{Cart, RefFamily};
    /// use workgraph::witnessed::{RegionSet, StepContext, Witnessed};
    ///
    /// let outside: u32 = 7;
    /// let cart = Rc::new(Cart(vec![1, 2, 3]));
    /// let ctx: StepContext<Cart> = StepContext::new(cart);
    /// // Try to capture a non-region borrow into the closure — rejected by the `for<'b>` brand.
    /// let _: Witnessed<RefFamily, RegionSet<Cart>> = ctx.alloc(|_region| &outside);
    /// ```
    pub fn alloc<T: Reattachable, W: SetWitness<Rc<F>>>(
        &self,
        build: impl for<'b> FnOnce(&'b F::Region) -> T::At<'b>,
    ) -> Witnessed<T, W> {
        Witnessed::<T, Rc<F>>::yoke(Rc::clone(&self.frame), build).into_set::<W>()
    }

    /// Build a value whose reach is the held frame's own region unioned with every named dep's reach,
    /// folded by the call shape (guarantee 5). Each dep's payload is handed to `build` only inside the
    /// shared `for<'b>` brand — the [`compile_fail`] guard below pins that a view cannot be smuggled
    /// out of the closure and stored unwitnessed.
    ///
    /// Built from the same primitives [`Sealed::transfer_into`] / [`Witnessed::map`] route: `deps` are
    /// folded one at a time into an accumulator yoked from this context's own region, each fold
    /// re-anchoring the accumulated views at the new shared brand and unioning the dep's witness in;
    /// the final `map` erases and seals the built value under `{frame} ∪ ⋃ deps` by construction.
    ///
    /// ```
    /// use std::rc::Rc;
    /// use workgraph::witnessed::doctest_fixture::{Cart, RefFamily};
    /// use workgraph::witnessed::{RegionSet, Sealed, StepContext, Witnessed};
    ///
    /// let dep_cart = Rc::new(Cart(vec![10]));
    /// let dep: Sealed<RefFamily, RegionSet<Cart>> =
    ///     Sealed::seal(Witnessed::yoke(dep_cart, |region| &region[0]).into_set());
    ///
    /// let cart = Rc::new(Cart(vec![1, 2, 3]));
    /// let ctx: StepContext<Cart> = StepContext::new(cart);
    /// let w: Witnessed<RefFamily, RegionSet<Cart>> =
    ///     ctx.alloc_with(&[&dep], |region, _views| &region[0]);
    /// assert_eq!(w.with(|r| **r), 1);
    /// ```
    ///
    /// ```compile_fail
    /// use std::rc::Rc;
    /// use workgraph::witnessed::doctest_fixture::{Cart, RefFamily};
    /// use workgraph::witnessed::{RegionSet, Sealed, StepContext, Witnessed};
    ///
    /// let dep_cart = Rc::new(Cart(vec![10]));
    /// let dep: Sealed<RefFamily, RegionSet<Cart>> =
    ///     Sealed::seal(Witnessed::yoke(dep_cart, |region| &region[0]).into_set());
    ///
    /// let cart = Rc::new(Cart(vec![1, 2, 3]));
    /// let ctx: StepContext<Cart> = StepContext::new(cart);
    /// let mut escaped: Option<&u32> = None;
    /// // Try to smuggle a dep view OUT of `alloc_with`'s closure — rejected by the `for<'b>` brand.
    /// let _: Witnessed<RefFamily, RegionSet<Cart>> = ctx.alloc_with(&[&dep], |region, views| {
    ///     escaped = Some(views[0]);
    ///     &region[0]
    /// });
    /// println!("{}", *escaped.unwrap());
    /// ```
    pub fn alloc_with<T, V, W>(
        &self,
        deps: &[&Sealed<V, W>],
        build: impl for<'b> FnOnce(&'b F::Region, Vec<V::At<'b>>) -> T::At<'b>,
    ) -> Witnessed<T, W>
    where
        T: Reattachable,
        V: Reattachable,
        V::At<'static>: Copy,
        W: UnionWitness + SetWitness<Rc<F>> + Clone,
        F::Region: 'static,
    {
        let acc0: Witnessed<AllocViews<V, F::Region>, W> =
            Witnessed::<AllocViews<V, F::Region>, Rc<F>>::yoke(Rc::clone(&self.frame), |region| {
                (region, Vec::with_capacity(deps.len()))
            })
            .into_set::<W>();
        let acc = deps.iter().fold(acc0, |acc, dep| {
            dep.transfer_into::<AllocViews<V, F::Region>, AllocViews<V, F::Region>>(
                acc,
                fold_dep_view::<V, F::Region>(),
            )
        });
        acc.map::<T>(finalize_alloc_with::<F, T, V>(build))
    }

    /// [`Self::alloc`] for a frame owning a library [`Region`]: the build closure receives the
    /// region's [`RegionHandle`] instead of the bare region.
    pub fn alloc_handle<P, T, W>(
        &self,
        build: impl for<'b> FnOnce(RegionHandle<'b, P>) -> T::At<'b>,
    ) -> Witnessed<T, W>
    where
        P: StorageProfile + 'static,
        F: RegionOwner<Region = Region<P>>,
        T: Reattachable,
        W: SetWitness<Rc<F>>,
    {
        self.alloc::<T, W>(|region| build(RegionHandle::new(region)))
    }

    /// [`Self::alloc_with`] for a frame owning a library [`Region`]: same dep folding, build closure
    /// receives the [`RegionHandle`].
    pub fn alloc_with_handle<P, T, V, W>(
        &self,
        deps: &[&Sealed<V, W>],
        build: impl for<'b> FnOnce(RegionHandle<'b, P>, Vec<V::At<'b>>) -> T::At<'b>,
    ) -> Witnessed<T, W>
    where
        P: StorageProfile + 'static,
        F: RegionOwner<Region = Region<P>>,
        T: Reattachable,
        V: Reattachable,
        V::At<'static>: Copy,
        W: UnionWitness + SetWitness<Rc<F>> + Clone,
    {
        self.alloc_with::<T, V, W>(deps, |region, views| {
            build(RegionHandle::new(region), views)
        })
    }
}

/// [`Self::alloc_with`]'s per-dep fold step, factored into its own generic function that carries no
/// `V::At<'static>: Copy` bound. Binding a `V::At<'b>` view directly inside a scope that *also*
/// carries that bound (as `alloc_with` must, to call [`Sealed::transfer_into`]) trips a rustc region-
/// inference gap over GAT projections — a fresh, non-`'static` instantiation gets spuriously required
/// to outlive `'static`. Building the closure here, where no such bound is in scope, and handing back
/// only the finished opaque `impl for<'b> FnOnce(..)` value sidesteps it: `alloc_with` itself never
/// binds a `V::At<'b>` value, only moves this closure around.
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
