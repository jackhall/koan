//! `Witnessed<T, W>` and the lifetime-erasure substrate it is built on — the single audited owner
//! of the erase-to-`'static` / reattach-to-`'r` discipline every lifetime-free inter-node carrier
//! shares. It sits below both an embedder's value layer and [`scheduler`](crate::scheduler) and
//! names no concrete workload type, so each depends on it for the machinery, not the reverse.
//!
//! A node's slot stores a borrow-carrying value the borrow checker can't lifetime-track: it forgets
//! the borrow's lifetime to `'static` for storage and re-anchors it at a caller-chosen lifetime on
//! read. The re-anchor is sound only while a *liveness witness* — the producer frame `Rc` that pins
//! the pointee — is held. [`Witnessed<T, W>`] bundles the erased value with that witness `W` in one
//! value, so "the witness keeps the value alive" is a type invariant, not a comment. Its accessors
//! are rank-2 (`for<'b>`) branded so a fabricated content lifetime cannot escape the witness pin:
//! [`Witnessed::with`] (borrow + read) and [`Witnessed::map`] (consume + transform) re-anchor an
//! already-bundled carrier, [`Witnessed::yoke`] *sources* one from the witness's own region so
//! co-location holds by construction, and [`Witnessed::merge_pinned`] combines two under one brand
//! and re-seals under their composed witness. For storage *between* accesses a carrier rests in a
//! [`Sealed`], the opaque dormant form that hides every transform and re-anchors only through the
//! rank-2 [`Sealed::open`].
//!
//! The layout machinery underneath — the [`Reattachable`] family contract, the private [`retype`]
//! primitive, [`erase_to_static`] and the storable [`Erased<T>`] — is the same single-lifetime
//! retype every carrier family routes. The carrier families ([`Reattachable`] impls) live in the
//! workload beside their own types, so this module stays workload-independent.

use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::rc::Rc;

use stable_deref_trait::StableDeref;

mod region;
pub use region::{
    AuditedStored, FamilyArena, Region, RegionHandle, RegionHandleFamily, StorageOf,
    StorageProfile, Stored,
};

mod region_set;
pub use region_set::{PinsRegion, RegionSet};

mod host;
pub use host::RegionHost;
#[cfg(any(test, feature = "test-hooks"))]
pub use host::{region_metrics, reset_region_metrics, RegionMetrics};

mod carrier;
pub use carrier::{Carrier, HasRegionHandle, Residence};

mod delivered;
pub use delivered::Delivered;

mod step_ctx;
pub use step_ctx::StepContext;

#[doc(hidden)]
pub mod doctest_fixture;

#[cfg(test)]
mod tests;

/// A type generic over exactly one lifetime whose representation is identical across every choice
/// of that lifetime — a lifetime parameter never changes layout. Implementing it lets the family
/// route the single audited lifetime-retype below.
///
/// # Safety
///
/// An implementor asserts that `At<'x>` and `At<'y>` are the *same type up to the lifetime
/// parameter* — identical size, alignment, and validity — for all `'x`, `'y`. Every well-formed
/// `type At<'r> = Foo<'r>;` where `Foo` is generic only in that lifetime satisfies this. Do not
/// implement it for a family whose layout depends on the lifetime.
pub unsafe trait Reattachable {
    type At<'r>;
}

/// Generate `unsafe impl Reattachable` for layout-invariant carrier families. Each
/// `Family => At<'r>` pair expands to the trait impl; write the GAT body with a literal `'r`
/// (`CarriedFamily => Carried<'r>`, `KObject<'static> => KObject<'r>`,
/// `OperatorGroup => OperatorGroup`).
///
/// The `unsafe` obligation — that `Family`'s `At<'r>` is one type up to the lifetime `'r` (identical
/// size, alignment, and validity for every `'r`, per [`Reattachable`]'s contract) — is discharged
/// **once** here, so the carrier sites carry no open-coded `unsafe impl`. The macro cannot *check*
/// layout-invariance, so only invoke it with families that genuinely satisfy the contract.
#[macro_export]
macro_rules! reattachable {
    ($($family:ty => $at:ty),+ $(,)?) => {$(
        // SAFETY: see the macro docs — `$family`'s `At<'r>` is layout-invariant in `'r`.
        unsafe impl $crate::witnessed::Reattachable for $family {
            type At<'r> = $at;
        }
    )+};
}
pub use reattachable;

/// The single lifetime-retype primitive: move an `A` out as a `B`, where the caller guarantees `A`
/// and `B` are one type up to a lifetime. Private to this module and reached only through the
/// `Reattachable`-bounded wrappers, so `A` / `B` are always `T::At<_>` for one family — the trait's
/// layout-invariance contract is what makes the bitwise move sound.
///
/// `transmute` can't prove `size_of::<T::At<'a>>() == size_of::<T::At<'b>>()` for an opaque GAT
/// projection, so this goes through `transmute_copy` (which assumes the size equality the contract
/// guarantees) behind a `ManuallyDrop` so the source is not dropped after the move. A `const` assert
/// restores the size check `transmute` would emit.
///
/// # Safety
///
/// `A` and `B` must be one type up to a lifetime (the `Reattachable` contract), so they share
/// layout and the source bytes are a valid `B`.
unsafe fn retype<A, B>(value: A) -> B {
    const { assert!(size_of::<A>() == size_of::<B>()) };
    let value = ManuallyDrop::new(value);
    // SAFETY: by the caller's contract `A` and `B` share layout (size asserted above); `ManuallyDrop`
    // keeps the source from being dropped after the bitwise move out.
    unsafe { std::mem::transmute_copy::<A, B>(&value) }
}

/// Erase a single-lifetime family value to its `'static` storage form — the **safe** half of the
/// erase/reattach pair, mirroring [`Erased::erase`] for a value stored raw rather than wrapped.
/// Forgetting a lifetime for storage cannot fabricate one (the value is stored, never used at
/// `'static`, until a witnessed re-anchor), so this is sound to call without `unsafe`. The
/// run-lifetime storage substrate routes its region writes through here instead of carrying its own
/// transmute, so [`retype`] is the single audited home for value lifetime-erasure.
pub fn erase_to_static<T: Reattachable>(value: T::At<'_>) -> T::At<'static> {
    // SAFETY: lifetime-only retype for storage of a single-lifetime family (the `Reattachable`
    // layout-invariance contract); the erased value is stored, not used, until a re-anchor.
    unsafe { retype::<T::At<'_>, T::At<'static>>(value) }
}

/// Read a `'static`-erased single-lifetime-family value behind a **rank-2** (`for<'b>`) brand: hand
/// `f` a reference re-anchored to a fresh existential `'b` it cannot leak (`R` cannot name `'b`), so
/// a fabricated content lifetime never escapes the read. The single home for the
/// `&T::At<'static> -> &'b T::At<'b>` retype — [`Witnessed::with`] reads its bundled carrier through
/// it, and the region allocator hands `project` its freshly-stored value through it, sound by the
/// same `for<'b>` quantifier as [`Sealed::open`].
///
/// The **signature is safe**: the caller keeps the pointee's storage live across the call (a `&self`
/// borrow over a bundled witness, or the region being alloc'd into), and the brand keeps the view from
/// outliving it — so call sites carry no `unsafe` of their own.
pub(crate) fn with_branded_ref<T: Reattachable, R>(
    stored: &T::At<'static>,
    f: impl for<'b> FnOnce(&'b T::At<'b>) -> R,
) -> R {
    // SAFETY: lifetime-only retype of a single-lifetime family (the `Reattachable` contract);
    // `&T::At<'static>` and `&T::At<'_>` share layout (a thin/fat pointer). The reattached view is
    // handed to a `for<'b>` closure whose `R` cannot name `'b`, so the fabricated content lifetime
    // cannot escape the call — the generativity trick `Witnessed::with` / `Sealed::open` share. The
    // pointee outlives the synchronous `f` call: the caller pins its storage for the whole call.
    let branded: &T::At<'_> = unsafe { retype::<&T::At<'static>, &T::At<'_>>(stored) };
    f(branded)
}

/// Proof of being inside a fold combinator's `for<'b>` closure. Minted only by the fold engines
/// ([`Witnessed::map`] / [`Witnessed::map_pinned`] / [`Witnessed::merge_composed`] /
/// [`Delivered::transfer_into`](delivered::Delivered::transfer_into) /
/// [`StepContext::alloc_with`](step_ctx::StepContext::alloc_with)); the private field keeps an
/// embedder from forging one, and the `'b` brand keeps it from escaping the closure — so a
/// capability gated on it is usable only at a fresh fold brand.
///
/// It doubles as the `E0582` witness the fold closures need — an input mentioning `'b`, without
/// which `impl for<'b> FnOnce(..) -> P::At<'b>` is rejected — so the same argument that proves
/// fold-closure residence also anchors the brand.
///
/// `Copy` is safe: the token cannot outlive its closure (`'b` is unnameable outside), so
/// duplicating it inside the closure grants nothing new.
///
/// ```compile_fail
/// use workgraph::witnessed::FoldToken;
/// use std::marker::PhantomData;
/// // The field is private outside the crate — a fold token cannot be forged by construction.
/// let _t: FoldToken<'static> = FoldToken(PhantomData);
/// ```
///
/// ```compile_fail
/// use workgraph::witnessed::FoldToken;
/// // `mint` is crate-internal — only the fold engines mint a token.
/// let _t: FoldToken<'static> = FoldToken::mint();
/// ```
///
/// ```
/// use workgraph::witnessed::doctest_fixture::{Cart, RefFamily};
/// use workgraph::witnessed::Witnessed;
/// // A combinator hands the token to its closure — the only way to obtain one.
/// let cart = Cart(vec![5]);
/// let w: Witnessed<RefFamily, Cart> = Witnessed::yoke(cart, |region| &region[0]);
/// let mapped = w.map::<RefFamily>(|r, _token| r);
/// assert_eq!(mapped.with(|r| **r), 5);
/// ```
#[derive(Clone, Copy)]
pub struct FoldToken<'b>(PhantomData<&'b ()>);

impl<'b> FoldToken<'b> {
    /// Mint a fold token — crate-internal, so only the fold engines can produce one.
    pub(crate) fn mint() -> Self {
        FoldToken(PhantomData)
    }
}

/// A forged fold token for the embedder's white-box tests, gated off production (like
/// [`region_metrics`]). An embedder unit test that drives a folded-placement hook in isolation —
/// outside a real fold combinator — sources its token here. Compiled out of production **and** out
/// of the external-crate `compile_fail` fixtures (which do not enable `test-hooks`), so it never
/// weakens the confinement the private field enforces.
#[cfg(any(test, feature = "test-hooks"))]
impl<'b> FoldToken<'b> {
    /// Forge a fold token for a test that has no enclosing fold engine to mint one.
    pub fn forge_for_test() -> Self {
        FoldToken::mint()
    }
}

/// A **compile-only** capability to store into a fold destination's region without a per-value
/// audit — the fold-door counterpart of [`FoldToken`]. It privately wraps the destination
/// [`RegionHandle`] and is minted only by a fold engine that has composed the closure result's
/// witness over exactly that region, so a value the closure builds from the fold's operands is
/// covered by the result witness. [`Self::alloc_resident_folded`] therefore discharges the store's
/// residence obligation at compile time, with **no runtime check** at all.
///
/// Like [`FoldToken`], `Copy` is safe (the placement cannot outlive its closure — `'b` is
/// unnameable outside — so duplicating it inside grants nothing new), the private field keeps an
/// embedder from forging one, and the crate-internal [`mint`](Self::mint) confines minting to the
/// fold engines. It doubles as the `E0582` witness a placement-bearing fold closure needs — an
/// input mentioning `'b`, without which `impl for<'b> FnOnce(..) -> P::At<'b>` is rejected.
///
/// ```compile_fail
/// use std::rc::Rc;
/// use workgraph::witnessed::FoldedPlacement;
/// use workgraph::witnessed::doctest_fixture::{fresh_region, FixtureProfile, RegionCart};
/// use workgraph::witnessed::RegionHandle;
/// let cart = Rc::new(RegionCart(fresh_region()));
/// let handle = RegionHandle::from_owner(&*cart);
/// // The field is private outside the crate — a placement cannot be forged by construction.
/// let _p: FoldedPlacement<'_, FixtureProfile> = FoldedPlacement { handle };
/// ```
///
/// ```compile_fail
/// use std::rc::Rc;
/// use workgraph::witnessed::FoldedPlacement;
/// use workgraph::witnessed::doctest_fixture::{fresh_region, FixtureProfile, RegionCart};
/// use workgraph::witnessed::RegionHandle;
/// let cart = Rc::new(RegionCart(fresh_region()));
/// let handle = RegionHandle::from_owner(&*cart);
/// // `mint` is crate-internal — only the fold engines mint a placement.
/// let _p = FoldedPlacement::mint(handle);
/// ```
pub struct FoldedPlacement<'b, W: StorageProfile> {
    handle: RegionHandle<'b, W>,
}

// Manual impls: a derive would bound `W: Clone` / `W: Copy`, which the `Copy` handle field does not
// need — mirroring [`RegionHandle`]'s own manual `Clone` / `Copy`.
impl<W: StorageProfile> Clone for FoldedPlacement<'_, W> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<W: StorageProfile> Copy for FoldedPlacement<'_, W> {}

impl<'b, W: StorageProfile> FoldedPlacement<'b, W> {
    /// Mint a placement over `handle` — crate-internal, so only a fold engine that has composed the
    /// result witness over `handle`'s region can produce one.
    pub(crate) fn mint(handle: RegionHandle<'b, W>) -> Self {
        FoldedPlacement { handle }
    }

    /// Forge a placement for an embedder white-box test that has no enclosing fold engine to mint
    /// one — gated off production and out of the external-crate `compile_fail` fixtures, mirroring
    /// [`FoldToken::forge_for_test`].
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn forge_for_test(handle: RegionHandle<'b, W>) -> Self {
        FoldedPlacement { handle }
    }

    /// The destination handle — the identity / allocation capability the enclosing fold closure
    /// already held at will, so exposing it grants nothing new.
    pub fn handle(self) -> RegionHandle<'b, W> {
        self.handle
    }

    /// Store a value built at this fold's own brand into the destination region — **no audit, no
    /// `Option`**.
    ///
    /// Sound by the rank-2 fold brand: the only inhabitants of `K::At<'b>` are the fold's declared
    /// operand views, the brand's own allocations, and owned `'static` data, all named by the
    /// witness the minting engine composes over this placement's own region. An ambient-lifetime
    /// capture is a compile error at this signature (`'b` has no outlives relation to any enclosing
    /// lifetime), so the always-true audit is discharged by the type. The private field plus
    /// crate-internal [`mint`](Self::mint) make the destination inseparable from that proof.
    pub fn alloc_resident_folded<K: Stored<W>>(self, value: K::At<'b>) -> &'b K::At<'b> {
        self.handle.region().alloc_resident::<K>(value)
    }
}

/// Generic owner of an erased carrier: a one-lifetime-family value with its lifetime forgotten to
/// `'static` for storage on a lifetime-free node slot. [`Self::erase`] stores; the value is
/// re-anchored either through a [`Witnessed`] that bundles its witness, or transiently through the
/// externally-witnessed [`SealedExtern::open`] (routing [`Self::reattach`]) against a borrowed witness.
/// The single audited home for the carrier families; see the module docs.
pub struct Erased<T: Reattachable> {
    inner: T::At<'static>,
}

impl<T: Reattachable> Erased<T> {
    /// Erase a live carrier to its storable `'static` form. Safe: forgetting a lifetime for
    /// storage cannot fabricate one — the value is stored, never used at `'static`, until a
    /// witnessed re-anchor.
    pub fn erase(live: T::At<'_>) -> Self {
        Erased {
            inner: erase_to_static::<T>(live),
        }
    }

    /// Re-anchor the carrier to a caller-chosen `'r` without a bundled witness — the raw fabrication
    /// the externally-witnessed [`SealedExtern::open`] wraps behind its rank-2 brand, supplying the pin
    /// at the access. The bundled-witness accessors ([`Witnessed::map`], [`Witnessed::merge_pinned`],
    /// the borrow-bounded [`Witnessed::read`]) route their re-anchor through here, each discharging this
    /// contract with its held witness; [`Witnessed::with`] reads through [`with_branded_ref`] instead.
    ///
    /// # Safety
    ///
    /// The caller holds a liveness witness — the carrier's frame `Rc`, or the run region — that pins
    /// the pointee for all of `'r`, and re-anchors only transiently while that witness is held, so
    /// the fabricated `'r` cannot outlive the pointee. `'r` is driven by the return-type annotation.
    pub unsafe fn reattach<'r>(self) -> T::At<'r> {
        // SAFETY: see the method contract; lifetime-only retype of a single-lifetime family.
        unsafe { retype::<T::At<'static>, T::At<'r>>(self.inner) }
    }

    /// The `'static`-erased inner value, for a crate-internal re-anchor via [`with_branded_ref`] —
    /// the route for a carrier that stores an erased reference *alongside* (not inside) its own
    /// witness, so it re-anchors under a pin other than the one bundled with it. The sole caller is
    /// [`Carrier`](carrier::Carrier)'s pinned reach reader. The returned `&T::At<'static>` interior is
    /// `Copy`, so a caller must re-anchor it under a held pin immediately (as `with_reach` does) and
    /// never let the `'static` form escape the re-anchor expression.
    pub(in crate::witnessed) fn as_static(&self) -> &T::At<'static> {
        &self.inner
    }
}

impl<T: Reattachable> Clone for Erased<T>
where
    T::At<'static>: Clone,
{
    fn clone(&self) -> Self {
        Erased {
            inner: self.inner.clone(),
        }
    }
}

impl<T: Reattachable> Copy for Erased<T> where T::At<'static>: Copy {}

/// A liveness witness bundled into a [`Witnessed`] (or borrowed by [`SealedExtern::open`]): holding it
/// keeps the carrier's lifetime-erased pointee at a fixed address, so a re-anchor that borrows the
/// witness cannot dangle. This is what lets [`Witnessed::with`] / [`Witnessed::map`] be **safe**
/// methods over an erased carrier — the pin is a bound the type system checks, not prose at the
/// read site.
///
/// # Safety
///
/// An implementor asserts that, for as long as a value of `Self` is held, the storage the carrier's
/// erased pointee refers to stays live and at a fixed address. A `Rc<F>` qualifies (it owns an `F`
/// at a stable heap address — a [`StableDeref`]). A witness that pins *nothing* — the empty element
/// of a set witness — also qualifies for a frameless terminal, whose pointee is backed by storage (a
/// run-global region) that outlives the carrier, so no held pin is required.
pub unsafe trait Witness {}

// SAFETY: `Rc<F>` is `StableDeref` — the `F` it owns lives at a fixed heap address for the whole
// life of the `Rc`, and cloning or moving the `Rc` does not move the `F`. The static bound below
// records that obligation as a checked fact rather than prose.
unsafe impl<F> Witness for Rc<F> {}
const _: fn() = || {
    fn assert_stable_deref<P: StableDeref>() {}
    let _ = assert_stable_deref::<Rc<()>>;
};

/// A [`Witness`] that exposes the region it pins, so a value built *solely* from that region is
/// co-located with the witness by construction. This is the seam [`Witnessed::yoke`] routes: the
/// constructor hands `Self::region` to a `for<'b>` closure, so the only references the produced
/// carrier can hold are reached through the pinned region.
///
/// # Safety
///
/// `region` returns a reference into the same storage `Self`'s [`Witness`] impl pins — i.e. a
/// reference whose referent stays live and at a fixed address for as long as the witness is held.
/// A value whose references are all derived from that reference is therefore pinned by the witness.
pub unsafe trait WitnessRegion: Witness {
    /// The region whose contents the witness pins.
    type Region: ?Sized;
    /// Borrow the pinned region.
    fn region(&self) -> &Self::Region;
}

/// What an embedder's frame-owner type (held behind an `Rc`) implements to pick up
/// [`WitnessRegion`] via the blanket impl below — the embedder's frame-owner type is foreign to
/// this crate, so it cannot itself be the target of a direct `WitnessRegion for Rc<F>` impl; this
/// trait lets the embedder supply the `region()` projection while the blanket impl carries the
/// orphan-rule-legal `Rc` wrapping.
///
/// # Safety
///
/// Same obligation as [`WitnessRegion::region`]: the returned reference must stay live and at a
/// fixed address for as long as `Self` is held.
pub unsafe trait RegionOwner {
    /// The region whose contents `Self` pins.
    type Region: ?Sized;
    /// Borrow the pinned region.
    fn region(&self) -> &Self::Region;
}

// SAFETY: `Rc<F>` is `StableDeref` (asserted for `Witness` above), so `F::region` — a reference
// into `F` — stays live and at a fixed address for as long as the `Rc` is held, satisfying
// `WitnessRegion`'s obligation given `F`'s own `RegionOwner` obligation holds.
unsafe impl<F: RegionOwner> WitnessRegion for Rc<F> {
    type Region = F::Region;
    fn region(&self) -> &Self::Region {
        RegionOwner::region(&**self)
    }
}

/// Witness composition for [`Witnessed::merge_pinned`] /
/// [`Delivered::transfer_into`](delivered::Delivered::transfer_into), run **inside** the `for<'b>`
/// brand while both operands' backings are still covered (by the bundled witnesses, or by the
/// pinned-merge caller's external pins) and the destination's live form `B::At<'b>` is in scope —
/// so an impl can *mint* into the destination rather than only computing a pure union. Total: every
/// pair of witnesses is composable against any destination, so there is no failure verdict.
///
/// Deliberately **not** `: Witness` — a reference-only witness composes too. [`RegionSet`] (a
/// pinning witness) composes by plain union, ignoring `dest`; [`Carrier`] (reference-only) composes
/// by minting both operands' reach into `dest`'s own arena and deriving the borrows-into-dest bit —
/// the pure reach mint, since it has no residence pin in hand to materialize (that is the
/// envelope-bearing [`Delivered::transfer_into`](delivered::Delivered::transfer_into)'s job).
///
/// # Safety
///
/// Holding the value [`compose`](Self::compose) returns must keep — for a pinning impl — or must
/// **name**, relative to `dest` — for a reference-only impl — every region `left`, `right`, and
/// `dest` reach, for as long as it is held, discharged with the destination's allocation
/// capability available. A reference-only composed witness must land its minted set in `dest`'s
/// own arena, so whatever covers `dest` covers it.
pub unsafe trait ComposeWitness<B: Reattachable>: Sized {
    /// Compose `left` and `right`'s witnesses into one pinning both — and, for an impl that mints,
    /// `dest`'s own region too — with `dest`'s live form available as a mint target.
    fn compose<'b>(left: &Self, right: &Self, dest: &B::At<'b>) -> Self;
}

/// An erased carrier bundled with the liveness [`Witness`] that keeps its pointee alive — the
/// consolidation of the old `(Erased<T>, witness)` pair into one value, so the witness-pins-the-value
/// relationship is structural. Reads go through [`Self::with`]; an advance/transform that may
/// re-seal the carrier goes through [`Self::map`]. Both fabricate the content lifetime behind a
/// rank-2 (`for<'b>`) brand, the generativity trick that keeps the fabricated lifetime from escaping
/// the witness pin.
pub struct Witnessed<T: Reattachable, W> {
    value: Erased<T>,
    witness: W,
}

/// Construction and storage verbs — deliberately **unbounded** in `W`: erasing a value for
/// storage, or moving/copying an already-erased one, fabricates nothing, so a reference-only
/// witness (the collapsed [`Carrier`]) stores and travels here freely. The pin obligation sits on
/// the *reattaching* verbs (the `W: Witness` block below, and the externally-pinned siblings), not
/// on construction.
impl<T: Reattachable, W> Witnessed<T, W> {
    /// Bundle an **already-erased** carrier with its witness. The `'static`-erased input carries no
    /// lifetime, so it leaves no input lifetime for inference to pick: it is the constructor for a
    /// `Result::map(Erased::erase)` pipeline, where threading the live value's lifetime through a
    /// closure would otherwise let it default to `'static`.
    ///
    /// Co-location — that the witness pins *this* value's references — is **caller-asserted** here: the
    /// value and witness arrive independently, so this is the crate-private substrate primitive, never
    /// a production construction path. Every production carrier is born co-located instead — via
    /// [`yoke`](Self::yoke) (sourced from the witness's region), [`resident`](Self::resident) (a
    /// region-pure value under the empty witness), or [`merge_pinned`](Self::merge_pinned) (folding
    /// two co-located carriers) — so no site pairs an arbitrary value with an arbitrary witness.
    pub fn from_erased(value: Erased<T>, witness: W) -> Self {
        Witnessed { value, witness }
    }

    /// Bundle a **region-pure** value under the default (empty / pins-nothing) witness — the honest
    /// constructor for a value built inside an alloc brand that references no foreign region. `resident`
    /// fixes the witness to `W::default()` — the empty set for a `FrameSet`-style set witness — so it
    /// **cannot** pair a value with a *wrong* non-empty witness; the only obligation it carries is that
    /// `value`'s foreign reach is genuinely empty. It is what lets the brand-confined region allocator
    /// return a witnessed carrier without an open-ended co-location assertion.
    ///
    /// Because the default witness pins nothing, the carrier depends on an **external pin** for every
    /// read: the active frame pins its region during the producing step, and once stored on a node the
    /// scheduler's retention hold (the delivery envelope's host) carries that pin — reads open under
    /// it, never bare. A value that *references* another region is the [`yoke`](Self::yoke) /
    /// [`merge_pinned`](Self::merge_pinned) path, which sources or folds that region's pin instead.
    ///
    /// Safe because the erase cannot fabricate a lifetime, and `W::default()` is the pins-nothing
    /// element of the witness type (the empty set).
    pub fn resident(value: T::At<'_>) -> Self
    where
        W: Default,
    {
        Self::from_erased(Erased::erase(value), W::default())
    }

    /// The bundled witness — the value's reach/pin description. For a pinning witness (a
    /// [`RegionSet`]) this is the set of producer frame `Rc`s that pin the carrier's pointee; for
    /// a reference-only witness (the collapsed [`Carrier`]) it names the reach without pinning it.
    pub fn witness(&self) -> &W {
        &self.witness
    }

    /// Duplicate the carrier: copy the erased value (a `Copy` carrier family — a thin/fat
    /// reference) and clone the witness. The seam [`Sealed::duplicate`] and the transfer verbs use
    /// to relocate a value out of a `&self` seal without consuming it, so the producer keeps its
    /// terminal for other consumers.
    fn duplicate(&self) -> Self
    where
        Erased<T>: Copy,
        W: Clone,
    {
        Witnessed {
            value: self.value,
            witness: self.witness.clone(),
        }
    }

    /// Swap the bundled witness for `witness`, keeping the erased value — the crate-internal
    /// witness-retype behind the reference-only construction surfaces ([`StepContext`]'s and the
    /// embedder-facing yoke veneers): a value built region-derived under a pinning witness (the
    /// `yoke` brand proves purity) is re-bundled under the reference-only default carrier, its
    /// liveness owned externally (the ambient frame during the step, retention after finalize).
    /// Crate-private so no caller can pair a value with a *wrong pinning* witness through it: the
    /// value stays erased throughout (no reattach), and every crate-internal use swaps toward a
    /// witness that pins nothing.
    pub(in crate::witnessed) fn rewitness<W2>(self, witness: W2) -> Witnessed<T, W2> {
        Witnessed {
            value: self.value,
            witness,
        }
    }

    /// Re-anchor the carrier bounded by the `&self` borrow **under an externally supplied pin** —
    /// the unbounded sibling of the `W: Witness`-gated [`Self::read`], for a carrier whose bundled
    /// witness pins nothing (the reference-only [`Carrier`]). Module-private: the sole callers are
    /// [`Sealed::open_with`] and the pinned merge, each of which holds the external pin for the
    /// whole call — that pin, not the bundle, is what keeps the pointee live across the borrow.
    fn read_pinned(&self) -> T::At<'_>
    where
        T::At<'static>: Copy,
    {
        // SAFETY: `reattach`'s contract — the wrapping externally-pinned verb borrows a pin that
        // keeps the pointee live and fixed-address for the whole call, and the returned carrier is
        // bounded by the `&self` borrow nested inside it, so it cannot outlive the pin. The `Copy`
        // bound copies the erased carrier out of `&self` before the consuming re-anchor.
        unsafe { self.value.reattach() }
    }

    /// [`Witnessed::with`] under an **externally supplied pin** — the borrow-and-read verb for a
    /// carrier whose bundled witness pins nothing (the reference-only [`Carrier`]), mirroring
    /// [`Sealed::open_with`]. `pin` is held for the whole call and keeps the pointee live; the
    /// `for<'b>` brand confines the re-anchored view exactly as `with` does.
    pub fn with_pinned<Pin: Witness, R>(
        &self,
        pin: &Pin,
        f: impl for<'b> FnOnce(&'b T::At<'b>) -> R,
    ) -> R {
        // The borrowed `pin` keeps the pointee live for the whole call — the same role the bundled
        // witness plays in `with`, supplied externally here; `with_branded_ref` confines the view.
        let _ = pin;
        with_branded_ref::<T, R>(&self.value.inner, f)
    }

    /// [`Witnessed::map`] under an **externally supplied pin** — consume, re-anchor at a `for<'b>`
    /// brand, transform, and re-seal under the same witness, for a carrier whose bundled witness
    /// pins nothing (the reference-only [`Carrier`]). `pin` is held for the whole call and keeps
    /// the carrier's pointee live; the [`FoldToken<'b>`] argument is load-bearing exactly as in
    /// `map` (`E0582`).
    pub fn map_pinned<P: Reattachable, Pin: Witness>(
        self,
        pin: &Pin,
        f: impl for<'b> FnOnce(T::At<'b>, FoldToken<'b>) -> P::At<'b>,
    ) -> Witnessed<P, W> {
        let Witnessed { value, witness } = self;
        // SAFETY: `reattach`'s contract — the borrowed `pin` keeps the carrier's pointee live and
        // fixed-address for the whole call (the `Witness` contract, supplied externally as in
        // `Sealed::open_with`); the re-anchor is transient (the fresh existential brand the
        // `for<'b>` closure cannot leak), and the projection is immediately re-erased to `'static`
        // for storage under the carried witness.
        let live: T::At<'_> = unsafe { value.reattach() };
        let projected = f(live, FoldToken::mint());
        let _ = pin;
        Witnessed {
            value: Erased::erase(projected),
            witness,
        }
    }

    /// [`Self::map_pinned`] that hands the closure a [`FoldedPlacement`] over the operand's **own
    /// head handle** (its [`HasRegionHandle`] projection) instead of a bare [`FoldToken`]. The
    /// placement is minted at the same fresh `for<'b>` brand the operand is re-anchored under, so a
    /// value the closure folds from the operand's views stores through the placement with no
    /// per-value audit. Because the handle comes from the operand itself — the handle its witness
    /// was yoked over at birth, never a caller argument or a closure capture — the placement's
    /// destination cannot be substituted by caller code; the covariance a bare handle would admit
    /// is closed.
    pub fn map_pinned_placing<P: Reattachable, PP: StorageProfile, Pin: Witness>(
        self,
        pin: &Pin,
        f: impl for<'b> FnOnce(T::At<'b>, FoldedPlacement<'b, PP>) -> P::At<'b>,
    ) -> Witnessed<P, W>
    where
        for<'b> T::At<'b>: HasRegionHandle<'b, PP>,
    {
        let Witnessed { value, witness } = self;
        // SAFETY: `reattach`'s contract, exactly as in `map_pinned` — the borrowed `pin` keeps the
        // carrier's pointee live and fixed-address for the whole call, the re-anchor is transient (the
        // fresh `for<'b>` brand the closure cannot leak), and the projection is immediately re-erased
        // to `'static` for storage under the carried witness. The placement is minted over the
        // operand's own projected handle at that same brand, so the destination is exactly the region
        // the operand carries — the one its witness covers. The mint is co-located with the re-anchor
        // because the placement carries the re-anchored operand's lifetime: routing it through a
        // shared reattach helper universally quantifies the fold brand and rejects the region
        // coercion (and a brand-free helper closure trips `E0582`, since `T::At<'b>` alone does not
        // witness `'b`).
        let live: T::At<'_> = unsafe { value.reattach() };
        let placement = FoldedPlacement::mint(live.region_handle());
        let projected = f(live, placement);
        let _ = pin;
        Witnessed {
            value: Erased::erase(projected),
            witness,
        }
    }

    /// Combine two witnessed carriers under an **externally supplied pin** rather than bundled
    /// pinning witnesses — the pinned-merge verb for reference-only-witnessed carriers (the
    /// collapsed [`Carrier`]), mirroring [`Sealed::open_with`]'s relationship to [`Sealed::open`].
    /// The composed witness
    /// still comes from [`ComposeWitness::compose`], so a caller cannot forge coverage; what the
    /// caller supplies is liveness: `pin` covers the *source* (`self`) carrier's backing for the
    /// whole call — the delivery envelope's retained host, the retention hold — while `other` (the
    /// destination operand being built into) is covered by its own live destination, which the
    /// caller necessarily holds to be composing into it.
    pub fn merge_pinned<B: Reattachable, P: Reattachable, Pin: Witness>(
        self,
        other: Witnessed<B, W>,
        pin: &Pin,
        f: impl for<'b> FnOnce(T::At<'b>, B::At<'b>, FoldToken<'b>) -> P::At<'b>,
    ) -> Witnessed<P, W>
    where
        W: ComposeWitness<B>,
    {
        self.merge_composed(other, pin, W::compose, f)
    }

    /// [`Self::merge_pinned`] handing `f` a [`FoldedPlacement`] over the destination operand `other`'s
    /// own handle instead of a bare [`FoldToken`]. The composed witness names `other`'s region, and
    /// the placement is minted over that same handle, so a value `f` folds from the two operands
    /// stores through the engine-controlled destination — never a caller-captured handle.
    pub fn merge_pinned_placing<B, P2, Pr, Pin>(
        self,
        other: Witnessed<B, W>,
        pin: &Pin,
        f: impl for<'b> FnOnce(T::At<'b>, B::At<'b>, FoldedPlacement<'b, Pr>) -> P2::At<'b>,
    ) -> Witnessed<P2, W>
    where
        B: Reattachable,
        P2: Reattachable,
        Pin: Witness,
        Pr: StorageProfile + 'static,
        W: ComposeWitness<B>,
        for<'b> B::At<'b>: HasRegionHandle<'b, Pr>,
    {
        self.merge_composed(other, pin, W::compose, place_over_dest::<T, B, P2, Pr>(f))
    }

    /// The engine under [`Self::merge_pinned`] and the envelope-bearing
    /// [`Delivered::transfer_into`](delivered::Delivered::transfer_into): a pinned merge whose
    /// composed witness is computed by an explicit `compose` closure over both witnesses and the
    /// destination's live form. Crate-private because a caller-supplied `compose` could
    /// under-cover; the two callers pass [`ComposeWitness::compose`] or the hosted carrier
    /// composition, both of which discharge the coverage obligation.
    pub(in crate::witnessed) fn merge_composed<B: Reattachable, P: Reattachable, Pin: Witness>(
        self,
        other: Witnessed<B, W>,
        pin: &Pin,
        compose: impl for<'b> FnOnce(&W, &W, &B::At<'b>) -> W,
        f: impl for<'b> FnOnce(T::At<'b>, B::At<'b>, FoldToken<'b>) -> P::At<'b>,
    ) -> Witnessed<P, W> {
        let Witnessed {
            value: left,
            witness: left_witness,
        } = self;
        let Witnessed {
            value: right,
            witness: right_witness,
        } = other;
        // SAFETY: the borrowed `pin` covers the left (source) carrier's backing for the whole call
        // (the `Witness` contract, supplied externally as in `Sealed::open_with`), and the right
        // (destination) operand's backing is the live destination the caller holds to compose
        // into. Both carriers are re-anchored to one existential brand the `for<'b>` closure
        // cannot leak, and the projection is immediately re-erased to `'static` for storage.
        // `compose` runs before `f` consumes `live_right`, so the destination's live form is still
        // available to mint into; the composed witness names the coverage thereafter
        // (`ComposeWitness`'s obligation, or the hosted composition's).
        let live_left: T::At<'_> = unsafe { left.reattach() };
        let live_right: B::At<'_> = unsafe { right.reattach() };
        let witness = compose(&left_witness, &right_witness, &live_right);
        let projected = f(live_left, live_right, FoldToken::mint());
        let _ = pin;
        Witnessed {
            value: Erased::erase(projected),
            witness,
        }
    }
}

/// Adapt a folded-placement relocate closure into the [`FoldToken`]-shaped closure
/// [`Witnessed::merge_composed`] expects: mint a [`FoldedPlacement`] over the destination operand's
/// own handle at the fold brand, then run the caller's `relocate`. The destination handle comes from
/// the operand itself (its [`HasRegionHandle`] projection — the same handle the composed witness
/// covers), so the placement's region is the engine's own operand, never a caller-captured handle.
/// Built as a returned `impl for<'b> FnOnce` so the closure is inferred higher-ranked over the brand
/// — an inline closure is not coerced to `for<'b>` and trips a spurious `'b: 'static`, the same
/// reason the `alloc_with` fold steps are factored out.
fn place_over_dest<T, B, P, Pr>(
    relocate: impl for<'b> FnOnce(T::At<'b>, B::At<'b>, FoldedPlacement<'b, Pr>) -> P::At<'b>,
) -> impl for<'b> FnOnce(T::At<'b>, B::At<'b>, FoldToken<'b>) -> P::At<'b>
where
    T: Reattachable,
    B: Reattachable,
    P: Reattachable,
    Pr: StorageProfile + 'static,
    for<'b> B::At<'b>: HasRegionHandle<'b, Pr>,
{
    |left, right, _token| {
        let placement = FoldedPlacement::mint(right.region_handle());
        relocate(left, right, placement)
    }
}

impl<T: Reattachable, W: Witness> Witnessed<T, W> {
    /// Bundle a carrier **sourced from the witness's own region** — the co-location-enforcing
    /// constructor, the build-time twin of [`Self::map`]. Where [`Self::new`] pairs an *arbitrary*
    /// value with an *arbitrary* witness (co-location asserted in prose at the call site), `yoke`
    /// hands the witness's pinned region to a **rank-2** (`for<'b>`) closure and bundles whatever it
    /// builds: the only references the produced carrier can hold are ones reached through that region,
    /// so the witness-pins-the-value invariant holds **by construction**.
    ///
    /// The `for<'b>` brand is what enforces it: a closure that tried to return a reference captured
    /// from its environment (`&'x`) would need `'x: 'b` for every `'b`, which only `'static` borrows
    /// satisfy — so the carrier's references are region-derived or owned / `'static`, never a smuggled
    /// foreign borrow. The [`compile_fail`] guard below pins this, mirroring [`Self::with`] / [`Self::map`].
    ///
    /// Safe: the closure's result is erased to `'static` (forgetting the borrow of the region) before
    /// `witness` moves into the bundle, and the [`WitnessRegion`] / [`Witness`] contracts guarantee the
    /// region stays live and fixed-address under the held witness — so the later re-anchor cannot dangle.
    ///
    /// ```
    /// use workgraph::witnessed::doctest_fixture::{Cart, RefFamily};
    /// use workgraph::witnessed::Witnessed;
    ///
    /// let cart = Cart(vec![1, 2, 3]);
    /// // A region-derived borrow satisfies the `for<'b>` brand — the compiling twin of the guard below.
    /// let w: Witnessed<RefFamily, Cart> = Witnessed::yoke(cart, |region| &region[0]);
    /// assert_eq!(w.with(|r| **r), 1);
    /// ```
    ///
    /// ```compile_fail
    /// use workgraph::witnessed::doctest_fixture::{Cart, RefFamily};
    /// use workgraph::witnessed::Witnessed;
    ///
    /// let outside: u32 = 7;
    /// let cart = Cart(vec![1, 2, 3]);
    /// // Try to yoke a borrow of `outside` (not region-derived) — rejected by the `for<'b>` brand.
    /// let _: Witnessed<RefFamily, Cart> = Witnessed::yoke(cart, |_region| &outside);
    /// ```
    pub fn yoke<F>(witness: W, f: F) -> Self
    where
        W: WitnessRegion,
        F: for<'b> FnOnce(&'b W::Region) -> T::At<'b>,
    {
        // The borrow of `witness` (through `region`) ends inside `erase`, which forgets the carrier's
        // lifetime; `witness` is then free to move into the bundle. Safe for the same reason as
        // `new` — the erase cannot fabricate a lifetime — but here the carrier is provably built from
        // the witness's region, so co-location is structural rather than asserted.
        let value = Erased::erase(f(witness.region()));
        Self::from_erased(value, witness)
    }

    /// [`Self::yoke`] for a witness pinning a library [`Region`]: the closure receives the region's
    /// [`RegionHandle`] allocation capability instead of the bare region, so a yoked construction
    /// allocates through the same handle every other site uses. Sound for the same reason `yoke` is —
    /// the `for<'b>` quantifier admits only region-derived or owned references, and nothing
    /// handle-flavoured escapes the closure.
    pub fn yoke_handle<P, F>(witness: W, f: F) -> Self
    where
        P: StorageProfile,
        W: WitnessRegion<Region = Region<P>>,
        F: for<'b> FnOnce(RegionHandle<'b, P>) -> T::At<'b>,
    {
        Self::yoke(witness, |region| f(RegionHandle::new(region)))
    }

    /// Read the carrier: re-anchor it behind a **rank-2** (`for<'b>`) closure, so the fabricated
    /// content lifetime is universally quantified and nothing `'b`-flavoured can be captured into
    /// `R` and outlive the witness pin (the generativity / ghost-cell trick). The naive
    /// borrow-bounded / content-free form is a Miri-proven use-after-free; this signature is the fix.
    ///
    /// The brand is load-bearing: copying a branded reference out of the closure (here
    /// `Cell::get`, whose `&u32` would otherwise escape past the witness drop) fails to compile,
    /// because `R` cannot mention the universally-quantified `'b`.
    ///
    /// ```
    /// use workgraph::witnessed::doctest_fixture::{Cart, InvFamily};
    /// use workgraph::witnessed::Witnessed;
    /// use std::cell::Cell;
    ///
    /// let cart = Cart(vec![42]);
    /// let w: Witnessed<InvFamily, Cart> = Witnessed::yoke(cart, |region| Cell::new(&region[0]));
    /// // Copy a brand-free scalar out — the compiling twin of the guard below.
    /// let value: u32 = w.with(|c| *c.get());
    /// assert_eq!(value, 42);
    /// ```
    ///
    /// ```compile_fail
    /// use workgraph::witnessed::doctest_fixture::{Cart, InvFamily};
    /// use workgraph::witnessed::Witnessed;
    /// use std::cell::Cell;
    ///
    /// let cart = Cart(vec![42]);
    /// let w: Witnessed<InvFamily, Cart> = Witnessed::yoke(cart, |region| Cell::new(&region[0]));
    /// // Try to smuggle a long-lived `&u32` OUT of `with` — rejected by the `for<'b>` brand.
    /// let escaped: &u32 = w.with(|c| c.get());
    /// drop(w);
    /// println!("{}", *escaped);
    /// ```
    pub fn with<R>(&self, f: impl for<'b> FnOnce(&'b T::At<'b>) -> R) -> R {
        // The bundled `witness` pins the pointee for the whole `&self` borrow; `with_branded_ref`
        // hands the reattached view to the `for<'b>` closure, so the fabricated content lifetime
        // cannot escape into `R`. Routes the single audited brand-retype home, so `with` carries no
        // `unsafe` of its own.
        with_branded_ref::<T, R>(&self.value.inner, f)
    }

    /// Transform the carrier (the `yoke::map_project` shape): consume `self`, re-anchor the carrier
    /// at a `for<'b>` brand, run `f` — which may interior-mutate the invariant carrier or bind
    /// cart-coherent `'b` values into it — then **re-seal** the projected `P::At<'b>` under the same
    /// witness. Re-sealing is what lets a *branded* value be kept, unlike [`Self::with`], which only
    /// lets a brand-free `R` out.
    ///
    /// The [`FoldToken<'b>`] argument is load-bearing, not decoration: without an input mentioning
    /// `'b`, `impl for<'b> FnOnce(..) -> P::At<'b>` is rejected (`E0582`), since the brand would
    /// appear only in the output GAT projection. This is exactly `yoke::map_project`'s shape.
    ///
    /// The brand also seals `map`: a projection cannot stash a branded reference into an outer slot
    /// to be read after the witness drops — the `for<'b>` quantifier rejects it at compile time.
    ///
    /// ```
    /// use workgraph::witnessed::doctest_fixture::{Cart, RefFamily};
    /// use workgraph::witnessed::Witnessed;
    ///
    /// let cart = Cart(vec![5]);
    /// let w: Witnessed<RefFamily, Cart> = Witnessed::yoke(cart, |region| &region[0]);
    /// // Project within the brand and re-seal — the compiling twin of the guard below.
    /// let mapped = w.map::<RefFamily>(|r, _token| r);
    /// assert_eq!(mapped.with(|r| **r), 5);
    /// ```
    ///
    /// ```compile_fail
    /// use workgraph::witnessed::doctest_fixture::{Cart, RefFamily};
    /// use workgraph::witnessed::Witnessed;
    ///
    /// let cart = Cart(vec![5]);
    /// let w: Witnessed<RefFamily, Cart> = Witnessed::yoke(cart, |region| &region[0]);
    /// let mut stolen: Option<&u32> = None;
    /// // Try to capture the branded `&'b u32` into a longer-lived slot — rejected by `for<'b>`.
    /// let _ = w.map::<RefFamily>(|r, _token| {
    ///     stolen = Some(r);
    ///     r
    /// });
    /// println!("{}", *stolen.unwrap());
    /// ```
    pub fn map<P: Reattachable>(
        self,
        f: impl for<'b> FnOnce(T::At<'b>, FoldToken<'b>) -> P::At<'b>,
    ) -> Witnessed<P, W> {
        let Witnessed { value, witness } = self;
        // SAFETY: `reattach`'s contract — the destructured `witness` is held across `f` and pins the
        // carrier's pointee; the re-anchor is transient (the fresh existential brand the `for<'b>`
        // closure cannot leak), and the projection is immediately re-erased to `'static` for storage
        // under that same witness.
        let live: T::At<'_> = unsafe { value.reattach() };
        let projected = f(live, FoldToken::mint());
        Witnessed {
            value: Erased::erase(projected),
            witness,
        }
    }

    /// Re-anchor the carrier and hand it **out** bounded by the `&self` borrow — the internal
    /// borrow-bounded reader [`Sealed::open`] copies its value through. The borrow-bounded sibling of
    /// [`Self::with`]: where `with`'s `for<'b>` brand forbids the carrier from escaping the closure,
    /// `read` lets it escape *at the borrow lifetime itself* — the content lifetime is the `&self`
    /// borrow, not a free `'b`, so the caller cannot widen it past the witness pin.
    ///
    /// This is sound for the exact reason the naive content-free reader is not: there, a free `'b`
    /// could be inferred `'static` and outlive the witness (a Miri-proven use-after-free); here the
    /// result is `T::At<'self>`, which the borrow checker keeps inside the `&self` borrow over which
    /// the bundled witness holds the pointee live. `At<'static>: Copy` copies the erased carrier out
    /// before re-anchoring.
    ///
    /// Module-private: the sole caller is [`Sealed::open`], the rank-2 access verb, so the
    /// borrow-bounded escape here is never a public surface.
    fn read(&self) -> T::At<'_>
    where
        T::At<'static>: Copy,
    {
        // SAFETY: `reattach`'s contract — the bundled `witness` pins the pointee for the whole
        // `&self` borrow (dropping it needs `&mut self`), and the returned carrier is bounded by
        // that borrow, so it cannot outlive the pin. The `Copy` bound copies the erased carrier
        // out of `&self` before the consuming re-anchor.
        unsafe { self.value.reattach() }
    }
}

impl<T: Reattachable, F: PinsRegion + 'static> Witnessed<T, Rc<F>> {
    /// Forget the bundled frame pin, re-bundling under the empty reference-only [`Carrier`] — the
    /// lift a freshly-[`yoke`](Self::yoke)d region-pure construction takes into the carrier world.
    /// The yoke brand already proved the value is derived from the frame's own region, so the
    /// carrier's empty reach is exact; what this drops is the *pin*, whose job moves to the
    /// caller's ambient liveness — the active frame during the step, the scheduler's retention
    /// hold once the value finalizes onto a node. Safe: the value stays erased throughout (no
    /// reattach); every later read names its coverage explicitly ([`Sealed::open_with`], the
    /// [`Delivered`](delivered::Delivered) envelope).
    pub fn into_reference_only(self) -> Witnessed<T, Carrier<F>> {
        self.rewitness(Carrier::default())
    }
}

/// The dormant node-storage form of a [`Witnessed`] carrier: an opaque seal the inter-node value
/// rests in between a node's steps, exposing neither construction nor transform — only the rank-2
/// destination verb [`open`](Self::open). Where [`Witnessed`] offers `with` / `map` / `yoke` /
/// `merge` directly, `Sealed` hides them, so
/// "this carrier is dormant — nothing is borrowed from it" is a type, not a convention. It wraps a
/// [`Witnessed`] rather than re-storing the erased carrier, so [`retype`] stays the single audited
/// reattach home and `Sealed` adds no `unsafe` of its own.
pub struct Sealed<T: Reattachable, W> {
    inner: Witnessed<T, W>,
}

/// Storage verbs and the externally-pinned read — **unbounded** in `W`, so a reference-only
/// witness (the collapsed [`Carrier`]) seals, travels, and duplicates as plain data, and opens
/// only under an externally supplied pin. The bundled-witness read ([`Self::open`]) sits in the
/// `W: Witness` block below — a bare `open` under a witness that pins nothing does not compile.
impl<T: Reattachable, W> Sealed<T, W> {
    /// Seal a live [`Witnessed`] into its dormant storage form — the only way in. `Sealed` exposes no
    /// other constructor and no transform once sealed, so a value can re-enter circulation only
    /// through an accessor below.
    pub fn seal(witnessed: Witnessed<T, W>) -> Self {
        Sealed { inner: witnessed }
    }

    /// Recover the bundled [`Witnessed`] — the exact inverse of [`seal`](Self::seal), a field move
    /// that consumes the seal. Lets a dormant slot value re-enter circulation as its producer's own
    /// carrier (a spliced single part becoming a slot terminal) rather than being re-wrapped by
    /// pairing the read-back value with a freshly-asserted witness — strictly better witnessing.
    /// Adds no `unsafe`: the carrier stays erased through the move; only a later accessor re-anchors.
    ///
    /// ```
    /// use workgraph::witnessed::doctest_fixture::{Cart, RefFamily};
    /// use workgraph::witnessed::{Sealed, Witnessed};
    ///
    /// let cart = Cart(vec![7]);
    /// let sealed: Sealed<RefFamily, Cart> =
    ///     Sealed::seal(Witnessed::yoke(cart, |region| &region[0]));
    /// // Unseal recovers the carrier; the value reads back unchanged.
    /// let witnessed = sealed.unseal();
    /// assert_eq!(witnessed.with(|r| **r), 7);
    /// ```
    pub fn unseal(self) -> Witnessed<T, W> {
        self.inner
    }

    /// Open the sealed carrier at a **rank-2** brand pinned by an **externally supplied** witness
    /// rather than the bundled one — the retention-pinned read. Identical to [`Self::open`] except the
    /// liveness pin is `pin` (held by the caller for the whole call), so a carrier whose own bundled
    /// witness pins nothing (a reference-only reach carrier, whose value's backing is kept alive by the
    /// scheduler's frame-retention hold) still opens soundly. The `for<'b>` brand confines the
    /// re-anchored value exactly as `open` does.
    ///
    /// # Panics / soundness
    ///
    /// The caller guarantees `pin` keeps the carrier's pointee live and fixed-address for the whole
    /// call — the retained producer-frame `Rc` (pinning invariant rules 3-4). The value is read at the
    /// `&self` borrow via [`Witnessed::read`], and the `for<'b>` quantifier keeps it from escaping, so
    /// this adds no `unsafe` beyond the audited [`Witnessed`] reattach.
    pub fn open_with<Wx: Witness, R>(&self, pin: &Wx, f: impl for<'b> FnOnce(T::At<'b>) -> R) -> R
    where
        T::At<'static>: Copy,
    {
        // The borrowed `pin` keeps the pointee live for the whole call — the same role the bundled
        // witness plays in `open`, supplied externally here. `read_pinned()` re-anchors at the
        // `&self` borrow and the `for<'b>` brand forbids escape, so nothing content-branded
        // outlives `pin`.
        let _ = pin;
        f(self.inner.read_pinned())
    }

    /// Duplicate the sealed carrier — copy the erased value (a `Copy` carrier family) and clone the
    /// witness — leaving this seal intact. The consumer-pull lift hands each construction finish a
    /// duplicate of the producer slot's own carrier (so the dep arrives **witnessed**, its reach named,
    /// rather than re-wrapped by pairing the value with a separately-asserted witness); the producer keeps its terminal for
    /// other consumers. Routes [`Witnessed::duplicate`], so it adds no `unsafe`.
    pub fn duplicate(&self) -> Self
    where
        Erased<T>: Copy,
        W: Clone,
    {
        Sealed {
            inner: self.inner.duplicate(),
        }
    }

    /// The bundled witness — the set of producer frame `Rc`s that pin the carrier's pointee. Cloned
    /// out by the consumer-pull lift to keep the backing region alive while the value is copied into
    /// the consumer's frame. Hands back the witness, not the value, so it does not weaken opacity.
    pub fn witness(&self) -> &W {
        self.inner.witness()
    }

    /// The bundled `Erased<T>`, read without consuming the seal — for a caller that must feed the
    /// erased inner into a further carrier (e.g. a `SealedExtern` zip) while keeping the original
    /// seal around for a later keep-first decision. Adds no `unsafe`: the value stays erased (no
    /// reattach), so this weakens opacity no further than [`Self::witness`] does for the witness half.
    pub fn erased(&self) -> &Erased<T> {
        &self.inner.value
    }
}

/// The bundled-witness re-anchors — gated on `W: Witness`, so they exist only for a witness that
/// genuinely pins. A reference-only carrier witness has no bundled coverage: its seal opens
/// through [`Sealed::open_with`] under an external pin, or relocates through the envelope-bearing
/// verbs; a bare `open` under it is a compile error by this bound.
impl<T: Reattachable, W: Witness> Sealed<T, W> {
    /// Open the sealed carrier at a **rank-2** (`for<'b>`) brand — the destination verb. The value is
    /// re-anchored and handed *by value* to a closure whose result `R` cannot mention the
    /// universally-quantified `'b`, so nothing branded by the fabricated content lifetime escapes the
    /// witness pin (the same generativity trick as [`Witnessed::with`]). The value arrives at the
    /// `&self` borrow via [`Witnessed::read`] — witness-pinned for that borrow — and the `for<'b>`
    /// quantifier is what forbids it leaving, so `open` carries no `unsafe` beyond the audited
    /// [`Witnessed`] reattach. The `At<'static>: Copy` bound is the slot's value-channel bound, so the
    /// result-slot readers can later route through `open` without strengthening it.
    ///
    /// The brand is load-bearing: returning the branded value out of the closure (`open(|live| live)`)
    /// fails to compile, because `R` would have to name `'b`. This mirrors the [`Witnessed::with`] /
    /// [`Witnessed::map`] guards.
    ///
    /// ```
    /// use workgraph::witnessed::doctest_fixture::{Cart, RefFamily};
    /// use workgraph::witnessed::{Sealed, Witnessed};
    ///
    /// let cart = Cart(vec![42]);
    /// let sealed: Sealed<RefFamily, Cart> =
    ///     Sealed::seal(Witnessed::yoke(cart, |region| &region[0]));
    /// // Copy a brand-free scalar out — the compiling twin of the guard below.
    /// let value: u32 = sealed.open(|live| *live);
    /// assert_eq!(value, 42);
    /// ```
    ///
    /// ```compile_fail
    /// use workgraph::witnessed::doctest_fixture::{Cart, RefFamily};
    /// use workgraph::witnessed::{Sealed, Witnessed};
    ///
    /// let cart = Cart(vec![42]);
    /// let sealed: Sealed<RefFamily, Cart> =
    ///     Sealed::seal(Witnessed::yoke(cart, |region| &region[0]));
    /// // Try to smuggle the branded value OUT of `open` — rejected by the `for<'b>` brand.
    /// let escaped: &u32 = sealed.open(|live| live);
    /// drop(sealed);
    /// println!("{}", *escaped);
    /// ```
    pub fn open<R>(&self, f: impl for<'b> FnOnce(T::At<'b>) -> R) -> R
    where
        T::At<'static>: Copy,
    {
        // The value is read at the `&self` borrow via [`Witnessed::read`] — witness-pinned for its
        // whole duration — and the `for<'b>` brand on `f` keeps anything content-branded from escaping
        // into `R`. Same brand and same audited reattach as `Witnessed::with`, so `Sealed` introduces
        // no `unsafe` of its own.
        f(self.inner.read())
    }
}

/// The **externally-witnessed** dormant form: an erased carrier that bundles *no* witness, opened by
/// supplying one at the access. Where [`Sealed`] bundles `W` (and so [`Sealed::open`] reads the pin
/// from the bundle), `SealedExtern` carries the carrier alone — the holder already pins the backing
/// and hands a borrow of the witness in at [`open`](Self::open). This is the form for a carrier whose
/// witness the holder must *not* duplicate: bundling a clone of a reference-counted cart would extend
/// its lifetime beyond the holder's own drop. It wraps an [`Erased`]
/// rather than re-storing the retype, so [`retype`] stays the single audited reattach home.
///
/// Its [`open`](Self::open) is **consuming** (takes `self`), so a non-`Copy` carrier — a
/// `Box<dyn FnOnce>` continuation — passes where [`Sealed::open`]'s `At<'static>: Copy` excludes it;
/// and several can be combined under one brand with [`zip`](Self::zip) so heterogeneous carriers
/// witnessed by the same pin open together (the run-loop step's continuation / contract / region).
pub struct SealedExtern<T: Reattachable> {
    value: Erased<T>,
}

impl<T: Reattachable> SealedExtern<T> {
    /// Seal an **already-erased** carrier into its externally-witnessed dormant form — the entry for a
    /// carrier the node already stores erased (the continuation / contract). No witness is bundled.
    pub fn seal(value: Erased<T>) -> Self {
        SealedExtern { value }
    }

    /// Erase a **live** carrier directly into the dormant form — the entry for a value re-anchored at
    /// the access rather than recovered from node storage (the run-loop `dest` region). Safe for the
    /// same reason as [`Erased::erase`]: forgetting a lifetime for storage cannot fabricate one.
    pub fn erase(live: T::At<'_>) -> Self {
        SealedExtern {
            value: Erased::erase(live),
        }
    }

    /// Open the externally-witnessed carrier at a **rank-2** (`for<'b>`) brand — the **consuming,
    /// externally-witnessed** destination verb, the witness-supplied twin of [`Sealed::open`]. The
    /// carrier is re-anchored to a fresh existential `'b` and handed **by value** to a closure whose
    /// result `R` cannot mention `'b`, so nothing branded by the fabricated content lifetime escapes
    /// the pin (the same generativity trick as [`Witnessed::with`]). Two things distinguish it from
    /// [`Sealed::open`]: the pin is supplied **at the call** (`witness`) rather than read from a
    /// bundle, and the carrier is **consumed**, so a non-`Copy` `Box<dyn FnOnce>` passes — there is no
    /// `At<'static>: Copy` bound.
    ///
    /// Soundness rests on the witness borrow: holding `&W` for the whole call keeps the carrier's
    /// pointee live and fixed-address (the [`Witness`] contract), and the fresh `'b` lives only for
    /// the synchronous `f(live)` call nested inside that borrow — so the re-anchored view cannot
    /// outlive the pin, and the `for<'b>` quantifier keeps it from escaping into `R`. The one audited
    /// reattach is [`Erased::reattach`]; this verb adds no `unsafe` of its own beyond it.
    ///
    /// The brand is load-bearing: returning the branded value out of the closure (`open(w, |live| live)`)
    /// fails to compile, because `R` would have to name `'b`. This mirrors the [`Sealed::open`] guard
    /// but over a **consumed**, externally-witnessed carrier.
    ///
    /// ```
    /// use workgraph::witnessed::doctest_fixture::{seal_extern, RefFamily};
    /// use workgraph::witnessed::SealedExtern;
    /// use std::rc::Rc;
    ///
    /// let backing: Rc<Vec<u32>> = Rc::new(vec![42]);
    /// let sealed: SealedExtern<RefFamily> = seal_extern(&backing[0]);
    /// // Copy a brand-free scalar out — the compiling twin of the guard below.
    /// let value: u32 = sealed.open(&backing, |live| *live);
    /// assert_eq!(value, 42);
    /// ```
    ///
    /// ```compile_fail
    /// use workgraph::witnessed::doctest_fixture::{seal_extern, RefFamily};
    /// use workgraph::witnessed::SealedExtern;
    /// use std::rc::Rc;
    ///
    /// let backing: Rc<Vec<u32>> = Rc::new(vec![42]);
    /// let sealed: SealedExtern<RefFamily> = seal_extern(&backing[0]);
    /// // Try to smuggle the branded value OUT of `open` — rejected by the `for<'b>` brand.
    /// let escaped: &u32 = sealed.open(&backing, |live| live);
    /// drop(sealed);
    /// println!("{}", *escaped);
    /// ```
    pub fn open<W: Witness, R>(self, _witness: &W, f: impl for<'b> FnOnce(T::At<'b>) -> R) -> R {
        // SAFETY: the borrowed `_witness` pins the carrier's pointee for the whole call (the `Witness`
        // contract: the backing stays live and fixed-address while the witness is held — here borrowed
        // for the call). The carrier is re-anchored to a fresh existential `'b` and handed by value to
        // the `for<'b>` closure, whose result `R` cannot name `'b`, so nothing content-branded escapes
        // the pin. Lifetime-only retype of a single-lifetime family (the `Reattachable` contract).
        let live: T::At<'_> = unsafe { self.value.reattach() };
        f(live)
    }

    /// Combine two externally-witnessed carriers into one, so they open together at a **single** brand
    /// via [`open`](Self::open) — the way heterogeneous carriers pinned by the *same* witness reach one
    /// step lifetime. The combined carrier is an [`And`] product of the two families; opening it hands
    /// the closure a `(T::At<'b>, U::At<'b>)` pair at one `'b`. A pure-data combine of two already-erased
    /// carriers, so it adds no `unsafe`: both halves are re-anchored together by the eventual `open`.
    pub fn zip<U: Reattachable>(self, other: SealedExtern<U>) -> SealedExtern<And<T, U>> {
        SealedExtern {
            value: Erased {
                inner: (self.value.inner, other.value.inner),
            },
        }
    }
}

impl<T: Reattachable> Clone for SealedExtern<T>
where
    T::At<'static>: Clone,
{
    fn clone(&self) -> Self {
        SealedExtern {
            value: self.value.clone(),
        }
    }
}

/// A `SealedExtern` whose carrier value is `Copy` — a thin pointer family (a `&Scope`) — is itself
/// `Copy`, so a holder can `open` a copied-out carrier each access without disturbing the stored
/// field. The non-`Copy` carriers (a `Box<dyn FnOnce>` continuation) simply do not meet the bound.
impl<T: Reattachable> Copy for SealedExtern<T> where T::At<'static>: Copy {}

/// Seal an **optional** already-erased carrier into the externally-witnessed dormant form, folding the
/// `Option` *inside* the seal as an [`OptionOf`] carrier — so an optional operand (the run-loop's
/// frame-gated return contract) can [`zip`](SealedExtern::zip) into a combined open and arrive as
/// `Option<T::At<'b>>` at the brand. A pure-data rewrap of `Option<Erased<T>>` into
/// `Erased<OptionOf<T>>` (both are `'static`-erased), so it carries no `unsafe`.
pub fn seal_option<T: Reattachable>(value: Option<Erased<T>>) -> SealedExtern<OptionOf<T>> {
    SealedExtern {
        value: Erased {
            inner: value.map(|erased| erased.inner),
        },
    }
}

/// Product of two carrier families, re-anchored as one — the family [`SealedExtern::zip`] seals so
/// heterogeneous carriers pinned by a shared witness open at a single brand. Layout-invariant in `'r`
/// because a tuple of two layout-invariant families is itself layout-invariant.
pub struct And<A, B>(PhantomData<(A, B)>);

// SAFETY: `(A::At<'r>, B::At<'r>)` is one type up to `'r` when both `A` and `B` are (each component is
// layout-invariant, so the tuple is too) — the `Reattachable` contract, discharged componentwise.
unsafe impl<A: Reattachable, B: Reattachable> Reattachable for And<A, B> {
    type At<'r> = (A::At<'r>, B::At<'r>);
}

/// `Option` of a carrier family, re-anchored as one — the family [`seal_option`] seals so an
/// **optional** operand opens to `Option<T::At<'b>>` at the brand. Layout-invariant in `'r` because
/// an `Option` of a layout-invariant family is itself layout-invariant.
pub struct OptionOf<T>(PhantomData<T>);

// SAFETY: `Option<T::At<'r>>` is one type up to `'r` when `T` is — the `Reattachable` contract,
// discharged through the inner family.
unsafe impl<T: Reattachable> Reattachable for OptionOf<T> {
    type At<'r> = Option<T::At<'r>>;
}
