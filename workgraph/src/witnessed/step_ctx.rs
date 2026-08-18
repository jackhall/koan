//! [`StepContext`] — the step construction context: a library-owned handle a step loop hands to a
//! finish, whose two verbs are guarantees 3 and 5 of the scheduler-library design made structural.
//! [`StepContext::alloc`] builds a value reachable only through the held frame's own region (reach =
//! own region only, by the `yoke` brand); [`StepContext::alloc_with`] relocates the whole run of
//! delivered dep envelopes in one act first, so the built value's carrier names every dep's whole
//! reach — its home region among the ordinary members of the envelope's pins —
//! and a dep's payload is viewable only inside the build closure's brand — it cannot be smuggled out
//! and stored unwitnessed.

use std::rc::Rc;

use super::{
    Carrier, Delivered, DropFree, FoldToken, FoldedPlacement, PinsRegion, Reattachable, Region,
    RegionHandle, RegionHandleFamily, RegionOwner, StorageProfile, Witnessed,
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
        // One relocation over the whole dep run against a bare handle on this context's own frame:
        // the destination's coverage is that frame, every dep's reach composes into it, and `build`
        // sees all the views at the shared brand — so there is no accumulator carrying a gathered
        // run between steps, and no projection to re-anchor one afterward. The deps arrive as a
        // slice of borrows, which the engine takes directly; nothing is gathered to adapt.
        Delivered::transfer_each_into::<RegionHandleFamily<P>, T, V, P>(
            deps.iter().copied(),
            Delivered::destination(Rc::clone(&self.frame)),
            // The views ride the product un-copied, so the built value genuinely borrows into every
            // region each dep does: the predicate releases none.
            |_dep, _view, _region| true,
            finalize_alloc_with::<F, T, V, P>(build),
        )
    }

    /// Build a value reachable only through the held frame's own region: reach = own region only,
    /// so the carrier references a description with empty members hosted in that same region — its
    /// liveness is the frame the step loop holds (guarantee 4), then each destination region the
    /// finalize walk delivers into. `build` receives the region's [`RegionHandle`], the one
    /// allocation capability, and
    /// the `for<'b>` brand on it admits only region-derived or owned references — so purity is
    /// structural rather than asserted: the value is yoked from the frame's own region and only then
    /// re-bundled under the pin-free carrier.
    ///
    /// ```
    /// use std::rc::Rc;
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, FixtureProfile, RefFamily, RegionCart};
    /// use workgraph::witnessed::{Carrier, RegionHandle, Sealed, StepContext, Witnessed};
    ///
    /// static SEVEN: u32 = 7;
    /// let cart = fresh_cart();
    /// let ctx: StepContext<RegionCart> = StepContext::new(Rc::clone(&cart));
    /// let w: Witnessed<RefFamily, Carrier<RegionCart>> =
    ///     ctx.alloc::<FixtureProfile, RefFamily>(|_handle| &SEVEN);
    /// // The carrier pins nothing; the seal's `'home` brand — the frame it was yoked into — is
    /// // what covers the read, so no pin is named at the call.
    /// let home = RegionHandle::<FixtureProfile>::from_owner(&*cart);
    /// assert_eq!(Sealed::seal(w, home).open(|r| *r), 7);
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

/// [`StepContext::alloc_with`]'s build step, adapting the embedder's closure to the shape
/// [`Delivered::transfer_all_into`]'s `relocate` takes — the region read off the destination
/// operand's own handle, and the placement's brand handed on as the fold token. The views run back
/// out beside the built value as the relocation's per-source cells: each dep's product cell **is**
/// its own view, un-copied, so the staging-order pairing the door asks for is the identity.
///
/// Factored into its own generic function that carries no `V::At<'static>: Copy` bound. Binding a
/// `V::At<'b>` view directly inside a scope that *also* carries that bound (as `alloc_with` must,
/// to relocate the envelopes) trips a rustc region-inference gap over GAT projections — a fresh,
/// non-`'static` instantiation gets spuriously required to outlive `'static`. Building the closure
/// here, where no such bound is in scope, and handing back only the finished opaque
/// `impl for<'b> FnOnce(..)` value sidesteps it: `alloc_with` itself never binds a `V::At<'b>`
/// value, only moves this closure around.
///
/// The views ride the product un-copied — sound because the relocation claimed every dep's own
/// pins, so each view's producer frame is a member of the product's minted set, pinned by the
/// consumer's own arena for the built value's life.
#[allow(clippy::type_complexity)]
fn finalize_alloc_with<F, T: Reattachable, V: Reattachable, P>(
    build: impl for<'b> FnOnce(&'b F::Region, &'b [V::At<'b>], FoldToken<'b>) -> T::At<'b>,
) -> impl for<'b> FnOnce(
    &'b [V::At<'b>],
    RegionHandle<'b, P>,
    FoldedPlacement<'b, P>,
) -> (T::At<'b>, &'b [V::At<'b>])
where
    P: StorageProfile + 'static,
    F: RegionOwner<Region = Region<P>>,
{
    |views, handle, _placement| (build(handle.region(), views, FoldToken::mint()), views)
}
