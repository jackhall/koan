//! The **bump door**: the public path from an embedder's `Drop`-free value family into a
//! [`Region`]'s byte arena, and the [`BumpPlacement`] capability a constructor writes through.
//!
//! A value stored here is *not* erased. The bump allocator is lifetime-free, so `'b` enters only at
//! the allocating call — which is why a bumped value may hold an `&'b` back into the very region it
//! lives in with no [`erase_to_static`](super::erase_to_static), no
//! [`AuditedStored`](super::AuditedStored) impl, and no residence audit. The typed
//! [`FamilyArena`](super::FamilyArena) cells cannot do that: their slot type is `K::At<'static>`, so
//! a region-self-referential value reaching one has to have its borrow erased and vetted back.
//!
//! Two arguments carry the door, stated here once so the method docs point at them rather than
//! restating them.
//!
//! **Confinement is inherited, not restated.** The door is a method on [`FoldedPlacement`], the fold
//! engines' unforgeable placement capability, and `'b` is the *enclosing* fold's brand — the same one
//! [`FoldedPlacement::alloc_resident_folded`] discharges its residence obligation against. A
//! constructor cannot capture an ambient `&'x` into the value it builds, because `'x` has no outlives
//! relation to a universally-quantified `'b`; and the door's product, an
//! [`Opened<'b, …>`](super::Opened), cannot leave the enclosing fold closure for the same reason.
//! Bare inside the brand, bundled on the way out. A door that minted a *fresh* `for<'b>` brand of its
//! own could not name that brand in its return type, so its product could never escape the call at
//! all — and a value built at a fresh brand could not be written into a bump borrowed at the
//! enclosing one without a lifetime retype, which is exactly the erasure this door exists not to do.
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

use std::ptr;

use super::{
    Carrier, FoldedPlacement, Opened, PinBundle, PinsRegion, ReachDescription, Reattachable,
    Region, RegionHandle, RegionOwner, StorageProfile,
};

/// The **byte-placement** capability: the write surface a [`fold_and_bump`](FoldedPlacement::fold_and_bump)
/// constructor builds its value through. A sibling of [`FoldedPlacement`], minted **only** inside a
/// door call — which is what makes every bumped byte belong to a value whose reach the door composed,
/// and why it is a distinct type rather than three more methods on the placement that already exists.
///
/// Its primitives are std shapes only — a `Copy` value, a `Copy` slice, a `str` — so the door names
/// no workload type and grows no per-family verb. Each hands back a shared `&'b`, never the `&mut`
/// the bump itself returns: a bumped value is region state a holder names, not one it owns.
///
/// Like [`FoldedPlacement`], `Copy` is safe (the capability cannot outlive its call — `'b` is
/// unnameable outside — so duplicating it inside grants nothing new), the private field keeps an
/// embedder from forging one, and the crate-internal [`mint`](Self::mint) confines minting to the
/// door.
///
/// ```compile_fail
/// use workgraph::witnessed::doctest_fixture::{fresh_cart, FixtureProfile};
/// use workgraph::witnessed::{BumpPlacement, RegionHandle};
/// let cart = fresh_cart();
/// let handle = RegionHandle::from_owner(&*cart);
/// // The field is private outside the crate — a bump placement cannot be forged by construction.
/// let _p: BumpPlacement<'_, FixtureProfile> = BumpPlacement { handle };
/// ```
///
/// ```compile_fail
/// use workgraph::witnessed::doctest_fixture::{fresh_cart, FixtureProfile};
/// use workgraph::witnessed::{BumpPlacement, RegionHandle};
/// let cart = fresh_cart();
/// let handle = RegionHandle::from_owner(&*cart);
/// // `mint` is crate-internal — only the bump door mints a placement.
/// let _p = BumpPlacement::<FixtureProfile>::mint(handle);
/// ```
pub struct BumpPlacement<'b, W: StorageProfile> {
    handle: RegionHandle<'b, W>,
}

// Manual impls: a derive would bound `W: Clone` / `W: Copy`, which the `Copy` handle field does not
// need — mirroring [`RegionHandle`]'s and [`FoldedPlacement`]'s own manual `Clone` / `Copy`.
impl<W: StorageProfile> Clone for BumpPlacement<'_, W> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<W: StorageProfile> Copy for BumpPlacement<'_, W> {}

impl<'b, W: StorageProfile> BumpPlacement<'b, W> {
    /// Mint a placement over `handle` — crate-internal, so only
    /// [`fold_and_bump`](FoldedPlacement::fold_and_bump), which has already composed and retained the
    /// product's reach over that same region, can produce one.
    pub(crate) fn mint(handle: RegionHandle<'b, W>) -> Self {
        BumpPlacement { handle }
    }

    /// Forge a placement for an embedder white-box test that has no enclosing door call to mint one —
    /// gated off production and out of the external-crate `compile_fail` fixtures, mirroring
    /// [`FoldedPlacement::forge_for_test`].
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn forge_for_test(handle: RegionHandle<'b, W>) -> Self {
        BumpPlacement { handle }
    }

    /// Bump one `Copy` value into the region and hand back the co-located `&'b`.
    ///
    /// `T: Copy` is the static proxy for "no destructor to skip": the bump releases its chunks whole
    /// and runs no `Drop`, so a `Drop`-bearing `T` would silently leak its owned contents. "`Drop`-free"
    /// has no expressible bound; `Copy` is the honest approximation, and every family queued behind
    /// this door satisfies it by construction.
    ///
    /// ```compile_fail
    /// use std::rc::Rc;
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, FixtureProfile, RefFamily, RegionCart};
    /// use workgraph::witnessed::StepContext;
    ///
    /// static SEVEN: u32 = 7;
    /// let cart = fresh_cart();
    /// let ctx: StepContext<RegionCart> = StepContext::new(Rc::clone(&cart));
    /// let _ = ctx.alloc_with_handle::<FixtureProfile, RefFamily, RefFamily>(&[], |placement, _views| {
    ///     placement.fold_and_bump::<RefFamily, RefFamily, RegionCart>(&[], |bump, _operands| {
    ///         // A `String` owns a heap buffer the bump would never free — rejected by `T: Copy`.
    ///         let _stored: &String = bump.value(String::from("leaks"));
    ///         &SEVEN
    ///     })
    ///     .value()
    /// });
    /// ```
    pub fn value<T: Copy>(self, value: T) -> &'b T {
        self.handle.region().alloc_side_value(value)
    }

    /// Copy a `Copy` slice into the region and hand back the co-located `&'b [T]` — the shape an
    /// operator group's member block or an expression's part list takes. Same `T: Copy` rationale as
    /// [`value`](Self::value).
    pub fn slice<T: Copy>(self, items: &[T]) -> &'b [T] {
        self.handle.region().alloc_side(items)
    }

    /// Copy `text` into the region and hand back the co-located `&'b str`. A `str` is a slice of
    /// `Copy` bytes under a UTF-8 invariant, so it carries no destructor either and needs no bound to
    /// say so.
    pub fn text(self, text: &str) -> &'b str {
        self.handle.region().alloc_side_str(text)
    }
}

/// The bump door itself, hung on the fold engines' placement capability — see the module docs for the
/// confinement and no-erasure arguments it rests on.
impl<'b, W: StorageProfile> FoldedPlacement<'b, W> {
    /// **Fold the operands' reach and bump the bytes, in one verb.** Compose every operand's reach
    /// into this fold's destination region, retain it there, then run `construct` — which builds the
    /// stored value through a [`BumpPlacement`] and the operands' opened views — and hand back the
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
    /// The `construct` closure exists to keep [`BumpPlacement`] door-minted and `'b`-confined: an
    /// embedder cannot hold one outside a door call. Operands are homogeneous in `V` — the same
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
    /// // `StrFamily` has no `Stored` impl and no `AuditedStored` impl: the bump needs neither.
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
        construct: impl FnOnce(BumpPlacement<'b, W>, &[V::At<'b>]) -> K::At<'b>,
    ) -> Opened<'b, K, Carrier<F>>
    where
        K: Reattachable,
        V: Reattachable,
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

        // Mint and retain BEFORE the value exists: the retention that keeps the operands' regions
        // alive has to be established before anything depending on them is built.
        let (description, bundle) = ReachDescription::mint(dest, &source_refs);
        dest.region().retain_for(description, bundle);

        let views: Vec<V::At<'b>> = operands.iter().map(|operand| operand.value()).collect();
        let built = construct(BumpPlacement::mint(dest), &views);
        // `Opened::adopted`'s precondition is discharged above: the door retained the pins covering
        // `'b` before handing the value over, so the value↔reach pairing is still library-minted.
        Opened::adopted(built, Carrier::new(description))
    }
}
