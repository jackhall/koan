//! `Witnessed<T, W>` and the lifetime-erasure substrate it is built on — the single audited owner
//! of the erase-to-`'static` / reattach-to-`'r` discipline every lifetime-free inter-node carrier
//! shares. It sits below both an embedder's value layer and [`scheduler`](crate::scheduler) and
//! names no concrete workload type, so each depends on it for the machinery, not the reverse.
//! See [design/witnessed-memory.md](../design/witnessed-memory.md) and
//! [design/reach.md § The carrier states](../design/reach.md#the-carrier-states).
//!
//! A node's slot stores a borrow-carrying value the borrow checker can't lifetime-track: it forgets
//! the borrow's lifetime to `'static` for storage and re-anchors it at a caller-chosen lifetime on
//! read. The re-anchor is sound only while a *liveness witness* — the producer frame `Rc` that pins
//! the pointee — is held. [`Witnessed<T, W>`] bundles the erased value with that witness `W` in one
//! value, so "the witness keeps the value alive" is a type invariant, not a comment. Its accessors
//! are rank-2 (`for<'b>`) branded so a fabricated content lifetime cannot escape the witness pin:
//! [`Witnessed::with`] (borrow + read) and [`Witnessed::map`] (consume + transform) re-anchor an
//! already-bundled carrier, [`Witnessed::yoke`] *sources* one from the witness's own region so
//! co-location holds by construction, and the envelope-bearing
//! [`Delivered::merge_into`](delivered::Delivered::merge_into) /
//! [`transfer_into`](delivered::Delivered::transfer_into) combine two under one brand and re-seal
//! under their composed witness.
//!
//! Between accesses a carrier rests either in a [`Sealed`] — opaque, and branded by the region
//! hosting it, which is what lets its reads take no pin — or in a [`Retained`], whose home liveness
//! is a refcount no lifetime can express. During a step it is an [`Opened`], whose content lifetime
//! rides the frame borrow so the step reads freely and [`reseal`](Opened::reseal)s at step end; in
//! transit it is a [`Delivered`](delivered::Delivered). A holder that has a carrier and its coverage
//! before it knows which frame will own them keeps them fused as an
//! [`Unhosted`](delivered::Unhosted) — the envelope minus its home pin, whose only exit
//! ([`Unhosted::host`](delivered::Unhosted::host)) supplies that pin.
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
pub use region::{Region, RegionHandle, RegionHandleFamily, StorageProfile};

mod reach;
pub(crate) use reach::PinBundle;
pub use reach::{PinsRegion, ReachDescription, StepCoverage};

mod host;
pub use host::RegionHost;
/// The pin-ring detector's whole embedder surface — present in any debug build, with no feature
/// wiring, and compiled out of a release one along with the detector itself.
#[cfg(debug_assertions)]
pub use host::{PinCycleReport, pin_cycle_reports, reset_pin_cycle_reports};
#[cfg(any(test, feature = "test-hooks"))]
pub use host::{RegionMetrics, region_metrics, reset_region_metrics};

mod carrier;
pub use carrier::{Carrier, HasRegionHandle};

mod bump;
pub use bump::{BumpAllocator, BumpBackedMap};

mod sectioned;
pub use sectioned::{CellInput, CellReach, CellRef, Sectioned};

mod delivered;
pub use delivered::{Delivered, Unhosted};

mod dormant;
use dormant::Dormant;
pub use dormant::{SealedPinned, Within};

mod step_ctx;
pub use step_ctx::StepContext;

/// Fixture types the doctests and `compile_fail` guards name. Doctests compile as external crates,
/// so the fixture must be `pub` — and gated behind `test-hooks`, or it would be production surface
/// an embedder could build against. Every configuration that runs a doctest passes that gate.
#[doc(hidden)]
#[cfg(any(test, feature = "test-hooks"))]
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
    /// The family's form at `'r`. Bounded `: 'r` because a single-lifetime family's value borrows
    /// only through `'r`, which is what lets a family's form be held behind a `&'r` (a region-bumped
    /// slice of views, say).
    type At<'r>: 'r;
}

/// Marker: this family's `At<'static>` has no drop glue, so it may rest in the Copy tier's
/// glue-free dormant slot ([`Erased`] and everything built over it). A family whose erased form
/// *does* need drop rests on the owned tier, [`SealedPinned`], instead.
///
/// Safe to implement: a false impl leaks the value's owned contents (the glue-free slot never runs
/// drop) and cannot cause UB. The intended route is the default [`reattachable!`] arm, whose const
/// backstop rejects a false declaration at compile time.
///
/// Resting a droppable family in the Copy tier is an ordinary trait error at type-check time:
///
/// ```compile_fail
/// use workgraph::witnessed::{reattachable, SealedExtern};
/// struct BoxedFamily;
/// reattachable!(droppable BoxedFamily => Box<&'r u32>);
/// // The `droppable` arm emits no `DropFree`, so the Copy tier rejects the family.
/// let _: Option<SealedExtern<BoxedFamily>> = None;
/// ```
///
/// ```
/// use workgraph::witnessed::{reattachable, SealedExtern};
/// struct RefFamily;
/// reattachable!(RefFamily => &'r u32);
/// // The default arm certifies `DropFree` — the compiling twin of the guard above.
/// let _: Option<SealedExtern<RefFamily>> = None;
/// ```
pub trait DropFree {}

/// Generate `unsafe impl Reattachable` for layout-invariant carrier families. Each
/// `Family => At<'r>` pair expands to the trait impl; write the GAT body with a literal `'r`
/// (`CarriedFamily => Carried<'r>`, `KObject<'static> => KObject<'r>`,
/// `OperatorGroup => OperatorGroup`).
///
/// The `unsafe` obligation — that `Family`'s `At<'r>` is one type up to the lifetime `'r` (identical
/// size, alignment, and validity for every `'r`, per [`Reattachable`]'s contract) — is discharged
/// **once** here, so the carrier sites carry no open-coded `unsafe impl`. The macro cannot *check*
/// layout-invariance, so only invoke it with families that genuinely satisfy the contract.
///
/// The default arm additionally certifies [`DropFree`](crate::witnessed::DropFree) — the family
/// rests in the Copy tier's glue-free slot — and backs that certification with a `needs_drop` const
/// assert, so declaring a droppable family through it is a compile error:
///
/// ```compile_fail
/// use workgraph::witnessed::reattachable;
/// struct BoxedFamily;
/// // `Box<&'r u32>` needs drop — the default arm's `needs_drop` backstop rejects it.
/// reattachable!(BoxedFamily => Box<&'r u32>);
/// ```
///
/// A family whose erased form needs drop takes the `droppable` arm instead, which emits the
/// `Reattachable` impl alone; it rests on [`SealedPinned`](crate::witnessed::SealedPinned):
///
/// ```
/// use workgraph::witnessed::reattachable;
/// struct BoxedFamily;
/// reattachable!(droppable BoxedFamily => Box<&'r u32>);
/// ```
#[macro_export]
macro_rules! reattachable {
    ($($family:ty => $at:ty),+ $(,)?) => {$(
        // SAFETY: see the macro docs — `$family`'s `At<'r>` is layout-invariant in `'r`.
        unsafe impl $crate::witnessed::Reattachable for $family {
            type At<'r> = $at;
        }
        impl $crate::witnessed::DropFree for $family {}
        // Backstop for the `DropFree` certification above: a droppable family declared through
        // this arm is a compile error, not a silent leak.
        const _: () = assert!(!::core::mem::needs_drop::<
            <$family as $crate::witnessed::Reattachable>::At<'static>,
        >());
    )+};
    (droppable $($family:ty => $at:ty),+ $(,)?) => {$(
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
/// `'static`, until a witnessed re-anchor), so this is sound to call without `unsafe`. The safe
/// erasure door onto the module-private [`retype`].
pub fn erase_to_static<T: Reattachable>(value: T::At<'_>) -> T::At<'static> {
    // SAFETY: lifetime-only retype for storage of a single-lifetime family (the `Reattachable`
    // layout-invariance contract); the erased value is stored, not used, until a re-anchor.
    unsafe { retype::<T::At<'_>, T::At<'static>>(value) }
}

/// Read a `'static`-erased single-lifetime-family value behind a **rank-2** (`for<'b>`) brand: hand
/// `f` a reference re-anchored to a fresh existential `'b` it cannot leak (`R` cannot name `'b`), so
/// a fabricated content lifetime never escapes the read. The single home for the
/// `&T::At<'static> -> &'b T::At<'b>` retype, sound by the same `for<'b>` quantifier as
/// [`Sealed::open`].
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

/// The `E0582` witness a token-form fold closure takes — an input mentioning `'b`, without which
/// `impl for<'b> FnOnce(..) -> P::At<'b>` is rejected — anchoring the closure's work to the fresh
/// fold brand. A pure brand marker: no door takes a `FoldToken` as a key — a fold that stores into
/// its destination carries a [`FoldedPlacement`] instead. Minted crate-internally and handed to the
/// closure run at the brand; the private field keeps an embedder from forging one, and the `'b`
/// brand keeps it from escaping the closure.
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
/// // `mint` is crate-internal — an embedder cannot mint a token.
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
    /// Mint a fold token — crate-internal, so no embedder can produce one.
    pub(crate) fn mint() -> Self {
        FoldToken(PhantomData)
    }
}

/// A **compile-only** capability to store into a fold destination's region without a per-value
/// audit — the fold-door counterpart of [`FoldToken`]. It privately wraps the destination
/// [`RegionHandle`] and is minted only by a fold engine that has composed the closure result's
/// witness over exactly that region, so a value the closure builds from the fold's operands is
/// covered by the result witness. [`Self::allocator`] therefore discharges the store's
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
/// use workgraph::witnessed::doctest_fixture::{fresh_cart, FixtureProfile};
/// use workgraph::witnessed::RegionHandle;
/// let cart = fresh_cart();
/// let handle = RegionHandle::from_owner(&*cart);
/// // The field is private outside the crate — a placement cannot be forged by construction.
/// let _p: FoldedPlacement<'_, FixtureProfile> = FoldedPlacement { handle };
/// ```
///
/// ```compile_fail
/// use std::rc::Rc;
/// use workgraph::witnessed::FoldedPlacement;
/// use workgraph::witnessed::doctest_fixture::{fresh_cart, FixtureProfile};
/// use workgraph::witnessed::RegionHandle;
/// let cart = fresh_cart();
/// let handle = RegionHandle::from_owner(&*cart);
/// // `mint` is crate-internal — an embedder cannot mint a placement.
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
    /// one. Gated off production, so the confinement the private field enforces holds for every
    /// build an embedder ships. The gate is on in the external-crate `compile_fail` fixtures too, so
    /// a guard must not assert that *forging* is impossible — what those guards pin down is the
    /// private field, which no configuration opens.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn forge_for_test(handle: RegionHandle<'b, W>) -> Self {
        FoldedPlacement { handle }
    }

    /// The destination handle — the identity / allocation capability the enclosing fold closure
    /// already held at will, so exposing it grants nothing new.
    pub fn handle(self) -> RegionHandle<'b, W> {
        self.handle
    }

    /// **The [`BumpAllocator`] over this fold's destination** — the store for a value the closure
    /// builds at this fold's own brand, with **no audit and no `Option`**.
    ///
    /// Sound by the rank-2 fold brand: the only inhabitants of a type at `'b` are the fold's
    /// declared operand views, the brand's own allocations, and owned `'static` data, all named by
    /// the witness the minting engine composes over this placement's own region. An
    /// ambient-lifetime capture is a compile error at that brand (`'b` has no outlives relation to
    /// any enclosing lifetime), so the always-true residence audit is discharged by the type. The
    /// private field plus crate-internal [`mint`](Self::mint) make the destination inseparable from
    /// that proof. [`fold_and_bump`](Self::fold_and_bump) remains the door for the other case —
    /// where the *operands'* reach still has to be composed and retained before the value depending
    /// on them exists.
    pub fn allocator(self) -> BumpAllocator<'b> {
        self.handle.allocator()
    }
}

/// Generic owner of an erased carrier: a one-lifetime-family value with its lifetime forgotten to
/// `'static` for storage on a lifetime-free node slot. [`Self::erase`] stores; the value is
/// re-anchored either through a [`Witnessed`] that bundles its witness, or transiently through the
/// externally-witnessed [`SealedExtern::open`] (routing [`Self::reattach`]) against a borrowed witness.
/// The single audited home for the carrier families; see the module docs.
///
/// This is the **Copy tier**'s owner, so its family is [`DropFree`]: the dormant slot is a union
/// and a union field has no drop glue. A droppable family rests on [`SealedPinned`] instead.
pub struct Erased<T: Reattachable + DropFree> {
    inner: Dormant<T::At<'static>>,
}

impl<T: Reattachable + DropFree> Erased<T> {
    /// Erase a live carrier to its storable `'static` form. Safe: forgetting a lifetime for
    /// storage cannot fabricate one — the value is stored, never used at `'static`, until a
    /// witnessed re-anchor.
    ///
    /// Crate-private, so no embedder mints a bare erased carrier: an embedder's own erasures go
    /// through a seal that bundles the value with its reach in the same act
    /// ([`RegionHandle::seal_reaching`], [`SealedExtern::erase`]).
    pub(crate) fn erase(live: T::At<'_>) -> Self {
        Erased {
            inner: Dormant::new(erase_to_static::<T>(live)),
        }
    }

    /// Wrap an **already-erased** value in the dormant slot — the module-internal constructor for
    /// a site that rebuilds an `Erased` out of erased parts (the [`SealedExtern::zip`] product,
    /// [`seal_option`]'s `Option` fold). Safe: the value is already erased, so wrapping it
    /// fabricates nothing.
    pub(in crate::witnessed) fn from_static(value: T::At<'static>) -> Self {
        Erased {
            inner: Dormant::new(value),
        }
    }

    /// Move the erased value out of the dormant slot, still erased — the inverse of
    /// [`from_static`](Self::from_static), for the same rebuild sites. Safe: no re-anchor happens,
    /// so no lifetime is fabricated.
    pub(in crate::witnessed) fn into_static(self) -> T::At<'static> {
        self.inner.into_inner()
    }

    /// Re-anchor the carrier to a caller-chosen `'r` without a bundled witness — the raw fabrication
    /// the externally-witnessed [`SealedExtern::open`] wraps behind its rank-2 brand, supplying the pin
    /// at the access. The bundled-witness accessors ([`Witnessed::map`], the composition engine under
    /// the envelope merge, the borrow-bounded reads behind [`Sealed::open`]) route their re-anchor
    /// through here, each discharging this contract with its held witness; [`Witnessed::with`] reads
    /// through [`with_branded_ref`] instead.
    ///
    /// # Safety
    ///
    /// The caller holds a liveness witness — the carrier's frame `Rc`, or the run region — that pins
    /// the pointee for all of `'r`, and re-anchors only transiently while that witness is held, so
    /// the fabricated `'r` cannot outlive the pointee. `'r` is driven by the return-type annotation.
    pub unsafe fn reattach<'r>(self) -> T::At<'r> {
        // SAFETY: see the method contract; lifetime-only retype of a single-lifetime family.
        unsafe { retype::<T::At<'static>, T::At<'r>>(self.inner.into_inner()) }
    }

    /// The `'static`-erased inner value, for a crate-internal re-anchor via [`with_branded_ref`] —
    /// the route for a carrier that stores an erased reference *alongside* (not inside) its own
    /// witness, so it re-anchors under a pin other than the one bundled with it. The returned
    /// `&T::At<'static>` interior is `Copy`, so a caller must re-anchor it under a held pin
    /// immediately and never let the `'static` form escape the re-anchor expression.
    pub(in crate::witnessed) fn as_static(&self) -> &T::At<'static> {
        self.inner.get()
    }
}

impl<T: Reattachable + DropFree> Clone for Erased<T>
where
    T::At<'static>: Clone,
{
    fn clone(&self) -> Self {
        Erased {
            inner: self.inner.clone(),
        }
    }
}

impl<T: Reattachable + DropFree> Copy for Erased<T> where T::At<'static>: Copy {}

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

/// The [`Witness`] that pins **nothing** — the named form of the empty set witness the safety
/// contract above sanctions. A read takes it when the pointee's backing is kept alive by something
/// outside the read (a run-global region, a frame the caller already holds across the call, an
/// operand that carries no region content at all), so the read needs no pin of its own.
///
/// It exists so an embedder can say "no coverage of my own" without naming an owned
/// `PinBundle`: a bundle is the ownership tier, and constructing an empty one
/// purely to stand in as a witness would put pin vocabulary in embedder hands for a site that pins
/// nothing.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoPins;

// SAFETY: `NoPins` holds nothing and so pins nothing — the empty element the `Witness` safety
// contract names. Every site that passes it asserts, exactly as an empty bundle does, that the
// pointee's backing outlives the read for an external reason.
unsafe impl Witness for NoPins {}

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

/// Witness composition for the envelope merge
/// ([`Delivered::transfer_into`](delivered::Delivered::transfer_into) /
/// [`merge_into`](delivered::Delivered::merge_into)), run **inside** the `for<'b>`
/// brand while both operands' backings are still covered (by the bundled witnesses, or by the pins
/// the envelope supplies to the composition engine) and the destination's live form `B::At<'b>` is
/// in scope —
/// so an impl can *mint* into the destination rather than only computing a pure union. Total: every
/// pair of witnesses is composable against any destination, so there is no failure verdict.
///
/// Deliberately **not** `: Witness` — a reference-only witness composes too. `PinBundle` (a
/// pinning witness) composes by plain union, ignoring `dest`; [`Carrier`] (reference-only) composes
/// by minting both operands' reach into `dest`'s own arena, which is also the product's residence.
/// The carrier owns no pin, so the *owned* bundles the mint folds are threaded in by the holder
/// that does — the envelope-bearing
/// [`Delivered::transfer_into`](delivered::Delivered::transfer_into), or a resident merge's entry
/// pins.
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

/// An erased carrier bundled with the liveness [`Witness`] that keeps its pointee alive, so the
/// witness-pins-the-value relationship is structural rather than a call-site convention. Reads go
/// through [`Self::with`]; an advance/transform that may re-seal the carrier goes through
/// [`Self::map`]. Both fabricate the content lifetime behind a rank-2 (`for<'b>`) brand, the
/// generativity trick that keeps the fabricated lifetime from escaping the witness pin.
pub struct Witnessed<T: Reattachable + DropFree, W> {
    value: Erased<T>,
    witness: W,
}

/// Construction and storage verbs — deliberately **unbounded** in `W`: erasing a value for
/// storage, or moving/copying an already-erased one, fabricates nothing, so a reference-only
/// witness (the collapsed [`Carrier`]) stores and travels here freely. The pin obligation sits on
/// the *reattaching* verbs (the `W: Witness` block below, and the externally-pinned siblings), not
/// on construction.
impl<T: Reattachable + DropFree, W> Witnessed<T, W> {
    /// Bundle an **already-erased** carrier with its witness. The `'static`-erased input carries no
    /// lifetime, so it leaves no input lifetime for inference to pick: it is the constructor for a
    /// `Result::map(Erased::erase)` pipeline, where threading the live value's lifetime through a
    /// closure would otherwise let it default to `'static`.
    ///
    /// Co-location — that the witness pins *this* value's references — is **caller-asserted** here:
    /// the value and witness arrive independently, so this is the crate-private substrate
    /// primitive and its visibility is what holds the assertion in. An embedder never reaches it, so
    /// no embedder site pairs an arbitrary value with an arbitrary witness: the public doors derive
    /// one side from the other — [`yoke`](Self::yoke) sources the value from the witness's region,
    /// [`resident`](Self::resident) fixes the witness to the empty one.
    pub(crate) fn from_erased(value: Erased<T>, witness: W) -> Self {
        Witnessed { value, witness }
    }

    /// Bundle a **region-pure** value under the default (empty / pins-nothing) witness — the
    /// constructor for a value built inside an alloc brand that references no foreign region. Fixing
    /// the witness to `W::default()` means it **cannot** pair a value with a *wrong* non-empty
    /// witness; the only obligation it carries is that `value`'s foreign reach is genuinely empty.
    ///
    /// A reference-only [`Carrier`] has no default — a carrier names the region its value lives in,
    /// and no default can name one — so it takes [`RegionHandle::seal_reaching`] under a
    /// `mint_retained(&[])` description, which names that residence off the handle itself.
    ///
    /// Because the default witness pins nothing, the carrier depends on an **external pin** for every
    /// read: the active frame during the producing step, the delivery envelope's own bundle while the
    /// terminal walks, or the destination region once it comes to rest — never bare. A value that
    /// *references* another region takes [`yoke`](Self::yoke) or the envelope merge instead, which
    /// source or fold that region's pin.
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
    /// `PinBundle`) this is the set of producer frame `Rc`s that pin the carrier's pointee; for
    /// a reference-only witness (the collapsed [`Carrier`]) it names the reach without pinning it.
    pub fn witness(&self) -> &W {
        &self.witness
    }

    /// Duplicate the carrier without consuming it, so a producer keeps its terminal while a
    /// consumer takes a copy: the erased value is bit-copied and the witness cloned.
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
    /// witness-retype behind the reference-only construction surfaces: a value built region-derived
    /// under a pinning witness (the `yoke` brand proves purity) is re-bundled under a reference-only
    /// carrier, its liveness owned externally (the ambient frame during the step, retention after
    /// finalize). The value stays erased throughout (no reattach), so this fabricates nothing;
    /// crate-privacy is what keeps an embedder from pairing a value with a *wrong pinning* witness
    /// through it.
    pub(in crate::witnessed) fn rewitness<W2>(self, witness: W2) -> Witnessed<T, W2> {
        Witnessed {
            value: self.value,
            witness,
        }
    }

    /// Re-anchor the carrier bounded by the `&self` borrow **under an externally supplied pin**,
    /// for a carrier whose bundled witness pins nothing (the reference-only [`Carrier`]).
    /// Module-private, so the pin obligation rests on the wrapping verb: it holds that external pin
    /// — or the `'home` brand standing in for one — across the whole call, and that, not the bundle,
    /// keeps the pointee live for the borrow.
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
    /// carrier whose bundled witness pins nothing (the reference-only [`Carrier`]). `pin` is held
    /// for the whole call and keeps the pointee live; the `for<'b>` brand confines the re-anchored
    /// view exactly as `with` does.
    pub(in crate::witnessed) fn with_pinned<Pin: Witness, R>(
        &self,
        pin: &Pin,
        f: impl for<'b> FnOnce(&'b T::At<'b>) -> R,
    ) -> R {
        // The borrowed `pin` keeps the pointee live for the whole call — the same role the bundled
        // witness plays in `with`, supplied externally here; `with_branded_ref` confines the view.
        let _ = pin;
        with_branded_ref::<T, R>(self.value.as_static(), f)
    }

    /// [`Witnessed::map`] under an **externally supplied pin** — consume, re-anchor at a `for<'b>`
    /// brand, transform, and re-seal under the same witness, for a carrier whose bundled witness
    /// pins nothing (the reference-only [`Carrier`]). `pin` is held for the whole call and keeps
    /// the carrier's pointee live; the [`FoldToken<'b>`] argument is load-bearing exactly as in
    /// `map` (`E0582`).
    pub(in crate::witnessed) fn map_pinned<P: Reattachable + DropFree, Pin: Witness>(
        self,
        pin: &Pin,
        f: impl for<'b> FnOnce(T::At<'b>, FoldToken<'b>) -> P::At<'b>,
    ) -> Witnessed<P, W> {
        let Witnessed { value, witness } = self;
        // SAFETY: `reattach`'s contract — the borrowed `pin` keeps the carrier's pointee live and
        // fixed-address for the whole call (the `Witness` contract, supplied externally as in
        // `Retained::open_with`); the re-anchor is transient (the fresh existential brand the
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

    /// The engine under the envelope-bearing relocation and merge verbs
    /// ([`Delivered::transfer_into`](delivered::Delivered::transfer_into),
    /// [`Delivered::merge_into`](delivered::Delivered::merge_into)): a pinned merge whose
    /// `fold` both builds the product and computes the composed witness, inside one brand. Both
    /// operand witnesses and both live forms are in scope there, so a composition that must see the
    /// *product* — the retention predicate at a relocation verb, which asks what the folded bytes
    /// still borrow — is expressible without a second re-anchor. Crate-private because a
    /// caller-supplied composition could under-cover; the composition passed in owes the coverage
    /// obligation.
    ///
    /// `fold` returns the projected value, the composed witness, and a threaded value `X` — the
    /// freshly-minted owned reach bundle for a reference-only carrier merge (threaded to the next
    /// fold step or the terminal seal), or `()` for a self-contained composed witness that owns
    /// what it names.
    pub(in crate::witnessed) fn merge_composed<
        B: Reattachable + DropFree,
        P: Reattachable + DropFree,
        Pin: Witness,
        X,
    >(
        self,
        other: Witnessed<B, W>,
        pin: &Pin,
        fold: impl for<'b> FnOnce(&W, &W, T::At<'b>, B::At<'b>, FoldToken<'b>) -> (P::At<'b>, W, X),
    ) -> (Witnessed<P, W>, X) {
        let Witnessed {
            value: left,
            witness: left_witness,
        } = self;
        let Witnessed {
            value: right,
            witness: right_witness,
        } = other;
        // SAFETY: the borrowed `pin` covers the left (source) carrier's backing for the whole call
        // (the `Witness` contract, supplied externally as in `Retained::open_with`), and the right
        // (destination) operand's backing is the live destination the caller holds to compose
        // into. Both carriers are re-anchored to one existential brand the `for<'b>` closure
        // cannot leak, and the projection is immediately re-erased to `'static` for storage. The
        // composition runs inside `fold`, where the destination's live form is available to mint
        // into; the composed witness names the coverage thereafter (`ComposeWitness`'s obligation,
        // or the hosted composition's).
        let live_left: T::At<'_> = unsafe { left.reattach() };
        let live_right: B::At<'_> = unsafe { right.reattach() };
        let (projected, witness, threaded) = fold(
            &left_witness,
            &right_witness,
            live_left,
            live_right,
            FoldToken::mint(),
        );
        let _ = pin;
        (
            Witnessed {
                value: Erased::erase(projected),
                witness,
            },
            threaded,
        )
    }

    /// [`Self::merge_composed`] with the source side **staged**: `self` is the destination operand
    /// and `staged` is a run of N erased source values, re-anchored as one [`Staged`] slice against
    /// it inside a single brand. `fold` therefore sees every source at the brand *at once* and
    /// builds the product in one pass — the N-ary shape behind
    /// [`Delivered::transfer_all_into`](delivered::Delivered::transfer_all_into), where the pairwise
    /// door would make a caller thread an accumulator that re-gathers its whole run per step.
    ///
    /// The source witnesses are not inputs. A pairwise merge hands `fold` both operands' witnesses
    /// because its composition is *between* two carriers; an N-ary relocation composes N source
    /// **bundles** instead, which the caller holds envelope-side and threads into the mint itself —
    /// so there is nothing for a per-source witness to contribute here.
    ///
    /// `pin` must cover **every** staged source's backing, not just one. It is `?Sized` so the
    /// caller's own borrowed slice of source bundles can serve as the witness directly, with no
    /// union allocation standing between the sources and the coverage they already are.
    pub(in crate::witnessed) fn merge_staged_composed<S, P, Pin, X>(
        self,
        staged: &[S::At<'static>],
        pin: &Pin,
        fold: impl for<'b> FnOnce(&W, &'b [S::At<'b>], T::At<'b>, FoldToken<'b>) -> (P::At<'b>, W, X),
    ) -> (Witnessed<P, W>, X)
    where
        S: Reattachable + DropFree,
        P: Reattachable + DropFree,
        Pin: Witness + ?Sized,
    {
        let Witnessed {
            value: dest,
            witness: dest_witness,
        } = self;
        // SAFETY: `retype`'s contract — `Staged<S>`'s form is one type up to its lifetime (the
        // family's layout-invariance, discharged at its `Reattachable` impl), so the staged run and
        // its re-anchored view share layout. The obligation is `merge_composed`'s, quantified over
        // the run: the borrowed `pin` covers every staged source's backing for the whole call (the
        // `Witness` contract, and the caller's own doc obligation above), `staged` itself is a
        // borrow the caller holds across the call, and the view is re-anchored to one existential
        // brand the `for<'b>` closure cannot leak.
        let live_staged: <Staged<S> as Reattachable>::At<'_> = unsafe { retype(staged) };
        // SAFETY: as in `merge_composed` — the destination operand's backing is the live destination
        // the caller holds to compose into, re-anchored to that same brand and immediately re-erased
        // to `'static` for storage below.
        let live_dest: T::At<'_> = unsafe { dest.reattach() };
        let (projected, witness, threaded) =
            fold(&dest_witness, live_staged, live_dest, FoldToken::mint());
        let _ = pin;
        (
            Witnessed {
                value: Erased::erase(projected),
                witness,
            },
            threaded,
        )
    }
}

/// The **staged run** family: N erased values of one family, viewed as a single slice. Its erased
/// form is `&'static [S::At<'static>]`, so a run staged by a relocation site re-anchors through the
/// same single retype one operand takes — which is what lets [`Witnessed::merge_staged_composed`]
/// hand a fold *every* source at the brand at once, rather than folding them in one at a time
/// against an accumulator that must re-bump its gathered run at every step.
///
/// Layout-invariant in `'r`: a slice of a layout-invariant family is one, the componentwise
/// discharge [`And`] takes.
pub(in crate::witnessed) struct Staged<S>(PhantomData<S>);

// SAFETY: `&'r [S::At<'r>]` is one type up to `'r` when `S` is — see the type's doc comment.
unsafe impl<S: Reattachable> Reattachable for Staged<S> {
    type At<'r> = &'r [S::At<'r>];
}

/// A shared reference needs no drop, so a staged run rests in the Copy tier like the run it views.
impl<S> DropFree for Staged<S> {}

/// A bundled carrier whose value family is a bit-copy (a thin/fat reference) and whose witness is
/// too — the reference-only [`Carrier`], never an owned `PinBundle` — is itself `Copy`, so it
/// rides inside a `Copy` embedder value (a resting cell held in an expression part) instead of
/// forcing that value to carry `Drop` glue. It grants nothing [`Self::duplicate`] does not already:
/// both copy the erased value and duplicate the witness, and a witness that *owns* pins is
/// excluded by the `W: Copy` bound, so no pin is ever silently duplicated.
///
/// Manual rather than derived: a derive would bound `T: Copy` (the family marker, which is never
/// `Copy`) instead of the erased value.
impl<T: Reattachable + DropFree, W: Copy> Clone for Witnessed<T, W>
where
    T::At<'static>: Copy,
{
    fn clone(&self) -> Self {
        *self
    }
}
impl<T: Reattachable + DropFree, W: Copy> Copy for Witnessed<T, W> where T::At<'static>: Copy {}

/// Adapt a folded-placement relocate closure into the [`FoldToken`]-shaped closure
/// [`Witnessed::merge_composed`] expects: mint a [`FoldedPlacement`] over the destination operand's
/// own handle at the fold brand, then run the caller's `relocate`. The destination handle comes from
/// the operand itself (its [`HasRegionHandle`] projection — the same handle the composed witness
/// covers), so the placement's region is the engine's own operand, never a caller-captured handle.
/// Built as a returned `impl for<'b> FnOnce` so the closure is inferred higher-ranked over the brand
/// — an inline closure is not coerced to `for<'b>` and trips a spurious `'b: 'static`, the same
/// reason `alloc_with`'s build step is factored out.
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

impl<T: Reattachable + DropFree, W: Witness> Witnessed<T, W> {
    /// Bundle a carrier **sourced from the witness's own region** — the co-location-enforcing
    /// constructor, the build-time twin of [`Self::map`]. Where the crate-private
    /// [`Self::from_erased`] pairs an *arbitrary* value with an *arbitrary* witness (co-location
    /// caller-asserted), `yoke` hands the witness's pinned region to a **rank-2** (`for<'b>`)
    /// closure and bundles whatever it builds: the only references the produced carrier can hold are
    /// ones reached through that region, so the witness-pins-the-value invariant holds **by
    /// construction**.
    ///
    /// The `for<'b>` brand is what enforces it: a closure that tried to return a reference captured
    /// from its environment (`&'x`) would need `'x: 'b` for every `'b`, which only `'static` borrows
    /// satisfy — so the carrier's references are region-derived or owned / `'static`, never a smuggled
    /// foreign borrow. The `compile_fail` guard below pins this, mirroring [`Self::with`] / [`Self::map`].
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
        // lifetime; `witness` is then free to move into the bundle. The erase cannot fabricate a
        // lifetime, and the carrier is provably built from the witness's region, so co-location is
        // structural rather than asserted.
        let value = Erased::erase(f(witness.region()));
        Self::from_erased(value, witness)
    }

    /// [`Self::yoke`] for a witness pinning a library [`Region`]: the closure receives the region's
    /// [`RegionHandle`] allocation capability instead of the bare region, so a yoked construction
    /// allocates through the region's own handle. Sound for the same reason `yoke` is —
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
    /// `R` and outlive the witness pin (the generativity / ghost-cell trick). A borrow-bounded /
    /// content-free signature here is a Miri-proven use-after-free.
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
        with_branded_ref::<T, R>(self.value.as_static(), f)
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
    pub fn map<P: Reattachable + DropFree>(
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
}

impl<T: Reattachable + DropFree, F: PinsRegion + 'static> Witnessed<T, Rc<F>> {
    /// Forget the bundled frame pin, re-bundling under a reference-only [`Carrier`] hosted in that
    /// frame's own region — the lift a freshly-[`yoke`](Self::yoke)d region-pure construction takes
    /// into the carrier world. The yoke brand already proved the value is derived from the frame's
    /// own region, so the minted description's empty members are exact and its host is where the
    /// value genuinely lives; the mint composes no source, so its retention folds an empty bundle.
    /// What this drops is the *pin*, whose job moves to the caller's
    /// ambient liveness — the active frame during the step, the destination region once the finalize
    /// walk delivers the value into it. Safe: the value stays erased throughout (no reattach); every
    /// later read names its coverage explicitly ([`Retained::open_with`], the
    /// [`Delivered`](delivered::Delivered) envelope).
    pub fn into_reference_only<P>(self) -> Witnessed<T, Carrier<F>>
    where
        P: StorageProfile<FrameOwner = F> + 'static,
        F: RegionOwner<Region = Region<P>>,
    {
        let carrier = Carrier::new(ReachDescription::mint_resident(
            RegionHandle::from_owner(&*self.witness),
            &[],
        ));
        self.rewitness(carrier)
    }
}

/// The dormant node-storage form of a [`Witnessed`] carrier: an opaque seal the inter-node value
/// rests in between a node's steps, exposing no transform at all — reads go through the rank-2
/// [`open`](Self::open) / [`open_ref`](Self::open_ref) or the borrow-tied
/// [`open_at`](Self::open_at). Where [`Witnessed`] offers `map` / `yoke` / the merge engines
/// directly, `Sealed` hides them, so "this carrier is dormant — nothing is borrowed from it" is a
/// type, not a convention. It wraps a [`Witnessed`] rather than re-storing the erased carrier, so
/// [`retype`] stays the single audited reattach home and `Sealed` adds no `unsafe` of its own.
///
/// `'home` is the **home-region brand**: the region hosting this carrier's reach description is
/// live and fixed-address for all of `'home`. Every read below rests on it, which is why none of
/// them takes a pin. The brand is covariant — a seal shortens into a narrower scope, never
/// lengthens out of the region that established it.
pub struct Sealed<'home, T: Reattachable + DropFree, W> {
    inner: Witnessed<T, W>,
    _home: PhantomData<&'home ()>,
}

/// Storage verbs and the reads — **unbounded** in `W`, so a reference-only witness (the collapsed
/// [`Carrier`]) seals, travels, and duplicates as plain data. No read here takes a pin: the
/// `'home` brand already carries the liveness a pin would otherwise assert, and the home region's
/// union bundle owns an `Rc` on every region the value reaches, so home-alive implies reach-alive.
impl<'home, T: Reattachable + DropFree, W> Sealed<'home, T, W> {
    /// Seal a live [`Witnessed`] into its dormant storage form under `home`'s brand — the entry for
    /// the reference-only tier. `home` is the region hosting the carrier's description, so the brand
    /// cannot be picked freely: a caller must already hold the handle to the region the value lives
    /// in, which is the same handle that placed it there. ([`seal_bundled`](Self::seal_bundled) is
    /// the sibling entry for a carrier whose own witness pins.)
    ///
    /// The brand is load-bearing: a seal cannot outlive the region that minted it, which is the
    /// whole reason its reads take no pin — holding one past its region's death is a borrow-check
    /// error rather than a prose obligation at every read site.
    ///
    /// ```compile_fail
    /// use std::rc::Rc;
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, FixtureProfile, RefFamily, RegionCart};
    /// use workgraph::witnessed::{Carrier, RegionHandle, Sealed, StepContext, Witnessed};
    ///
    /// static SEVEN: u32 = 7;
    /// let escaped: Sealed<'_, RefFamily, Carrier<RegionCart>> = {
    ///     let cart = fresh_cart();
    ///     let ctx: StepContext<RegionCart> = StepContext::new(Rc::clone(&cart));
    ///     let w = ctx.alloc::<FixtureProfile, RefFamily>(|_handle| &SEVEN);
    ///     // Try to smuggle the seal OUT of the region that branded it — rejected: `cart` (and the
    ///     // region the handle borrows) dies at the end of this block.
    ///     Sealed::seal(w, RegionHandle::<FixtureProfile>::from_owner(&*cart))
    /// };
    /// escaped.open(|r| *r);
    /// ```
    pub fn seal<P: StorageProfile>(
        witnessed: Witnessed<T, W>,
        home: RegionHandle<'home, P>,
    ) -> Self {
        let _ = home;
        Sealed {
            inner: witnessed,
            _home: PhantomData,
        }
    }

    /// Recover the bundled [`Witnessed`] — the exact inverse of [`seal`](Self::seal), a field move
    /// that consumes the seal. Lets a dormant slot value re-enter circulation as its producer's own
    /// carrier (a spliced single part becoming a slot terminal) rather than being re-wrapped around a
    /// freshly-asserted witness. Adds no `unsafe`: the carrier stays erased through the move; only a
    /// later accessor re-anchors.
    ///
    /// ```
    /// use workgraph::witnessed::doctest_fixture::{Cart, RefFamily};
    /// use workgraph::witnessed::{Sealed, Witnessed};
    ///
    /// let cart = Cart(vec![7]);
    /// let sealed: Sealed<'_, RefFamily, Cart> =
    ///     Sealed::seal_bundled(Witnessed::yoke(cart, |region| &region[0]));
    /// // Unseal recovers the carrier; the value reads back unchanged.
    /// let witnessed = sealed.unseal();
    /// assert_eq!(witnessed.with(|r| **r), 7);
    /// ```
    pub fn unseal(self) -> Witnessed<T, W> {
        self.inner
    }

    /// The bundled carrier, for the tier crossings inside this module.
    pub(in crate::witnessed) fn into_inner(self) -> Witnessed<T, W> {
        self.inner
    }

    /// Seal a carrier whose **bundled witness** pins — the tier where liveness rides in the value
    /// rather than in a brand, so `'home` is free and harmless: every read is bounded by a `&self`
    /// borrow, and `self` holds the pin. The region-branded [`seal`](Self::seal) is for the
    /// reference-only [`Carrier`] tier, whose witness pins nothing and whose brand is therefore the
    /// only liveness evidence there is.
    pub fn seal_bundled(witnessed: Witnessed<T, W>) -> Self
    where
        W: Witness,
    {
        Sealed {
            inner: witnessed,
            _home: PhantomData,
        }
    }

    /// Re-seal a value that was opened at `'home` — the step-end return to rest. The brand rides
    /// back in from the [`Opened`] it came from, whose own `'b` was bounded by a live region (the
    /// seal's brand, or the pin borrow the retained tier opened under), so this mints no claim the
    /// open did not already carry.
    pub(in crate::witnessed) fn from_opened(witnessed: Witnessed<T, W>) -> Self {
        Sealed {
            inner: witnessed,
            _home: PhantomData,
        }
    }

    /// Open the sealed carrier at a **rank-2** (`for<'b>`) brand — the read verb. Takes no pin: the
    /// `'home` brand is the liveness proof, established by the region door that placed the value
    /// and carried by the type ever since, so the pairing a pin argument could only assert in prose
    /// ("this owner pins *this* carrier's pointee") is here a borrow-check fact.
    ///
    /// The `for<'b>` quantifier confines the re-anchored value to the call, so nothing
    /// content-branded escapes into `R`. Adds no `unsafe` beyond the audited [`Witnessed`] reattach.
    pub fn open<R>(&self, f: impl for<'b> FnOnce(T::At<'b>) -> R) -> R
    where
        T::At<'static>: Copy,
    {
        // `'home` outlives this `&self` borrow, so the pointee is live for the whole call — the
        // role an externally supplied pin plays on the retained tier. `read_pinned` re-anchors at
        // the `&self` borrow and the `for<'b>` brand forbids escape.
        f(self.inner.read_pinned())
    }

    /// [`Self::open`] handing `f` the re-anchored value **by reference**, for a value family whose
    /// views are not `Copy`. Same soundness story: `'home` covers the call and the `for<'b>` brand
    /// confines the view.
    pub fn open_ref<R>(&self, f: impl for<'b> FnOnce(&'b T::At<'b>) -> R) -> R {
        self.inner.with_pinned(&NoPins, f)
    }

    /// Open the sealed carrier into the **in-use** [`Opened`] state at the step lifetime `'b` — the
    /// borrow-tied read whose `'b` rides the `&'b self` seal borrow under the `'home` brand. Where
    /// [`Self::open`] / [`Self::open_ref`] confine the re-anchored value inside a **rank-2** closure,
    /// `open_at` hands it out as an [`Opened<'b, T, W>`] whose content lifetime **is** the borrow
    /// lifetime `'b` — so the borrow checker keeps it from escaping the pin without a `for<'b>`
    /// closure at every read site (the step opens once, reads freely, and [`Opened::reseal`]s or lifts
    /// at step end).
    ///
    /// Sound because `'b` is a concrete lifetime bounded by the
    /// `&'b self` seal borrow — not a free `'b` an inference could widen to `'static` (the
    /// Miri-proven use-after-free the rank-2 brand otherwise guards) — and `'home: 'b` follows from
    /// the seal existing, so the region the value re-anchors into is live for the whole life of the
    /// returned [`Opened`]. It is `Copy` and constructible only here, so the value↔reach pairing it
    /// carries is exactly this seal's.
    pub fn open_at<'b>(&'b self) -> Opened<'b, T, W>
    where
        'home: 'b,
        T::At<'static>: Copy,
        W: Clone,
    {
        // `'home: 'b` keeps the home region live for the whole life of the returned `Opened`;
        // `read_pinned` re-anchors the value at the `&'b self` borrow (bounded by `'b`, never a free
        // lifetime), and the witness is cloned so the value↔reach pairing rides along.
        Opened {
            value: self.inner.read_pinned(),
            witness: self.inner.witness().clone(),
        }
    }

    /// Duplicate the sealed carrier — copy the erased value (a `Copy` carrier family) and clone the
    /// witness — leaving this seal intact, so a consumer-pull lift hands each construction finish a
    /// dep that arrives **witnessed**, its reach named, while the producer keeps its terminal for
    /// other consumers. Adds no `unsafe`.
    pub fn duplicate(&self) -> Self
    where
        Erased<T>: Copy,
        W: Clone,
    {
        Sealed {
            inner: self.inner.duplicate(),
            _home: PhantomData,
        }
    }

    /// The bundled witness — the carrier's reach/pin description. Handing back the witness rather
    /// than the value is what keeps the seal opaque.
    pub fn witness(&self) -> &W {
        self.inner.witness()
    }
}

/// The **externally-held** dormant form: a carrier at rest with no home-region brand, because its
/// home's liveness is not lexical. A delivered terminal lives here — it rests in the destination
/// region its edge names, and what keeps its backing alive is that region's own life, which is a
/// refcount protocol no lifetime can express.
///
/// So `Retained` has **no pin-free read verb**: its reads are crate-internal and each takes an
/// externally supplied pin, and the way back to pin-free reads is to re-brand — against a region
/// that has taken the coverage over, or against a held pin's borrow ([`Self::brand_with`]).
/// Splitting it from [`Sealed`] is what lets `Sealed`'s reads be pin-free: a type that cannot prove
/// liveness cannot borrow a verb from one that can.
pub struct Retained<T: Reattachable + DropFree, W> {
    inner: Witnessed<T, W>,
}

impl<T: Reattachable + DropFree, W> Retained<T, W> {
    /// Wrap a live carrier as retention-held — the finalize-side entry, where the value has left
    /// every lexical region it could be branded against and the slot's hold takes over.
    pub(crate) fn from_witnessed(witnessed: Witnessed<T, W>) -> Self {
        Retained { inner: witnessed }
    }

    /// Take a branded seal down to the retention-held tier — a capability *loss* (the brand's
    /// pin-free reads go with it), so it needs no proof of its own.
    pub fn from_sealed(sealed: Sealed<'_, T, W>) -> Self {
        Retained {
            inner: sealed.into_inner(),
        }
    }

    /// Re-brand this carrier against the region `home` names — the door back into pin-free reads.
    /// Crate-internal, and its contract is that the envelope's coverage is already lodged in that
    /// same region, so the brand it mints is backed by retention rather than by assertion.
    pub(crate) fn brand_to<'home, P: StorageProfile>(
        self,
        home: RegionHandle<'home, P>,
    ) -> Sealed<'home, T, W> {
        Sealed::seal(self.inner, home)
    }

    /// The bundled carrier, for the tier crossings inside this module.
    pub(in crate::witnessed) fn into_retained_inner(self) -> Witnessed<T, W> {
        self.inner
    }

    /// Recover the bundled [`Witnessed`] — the exact inverse of
    /// [`from_witnessed`](Self::from_witnessed), a field move that consumes the carrier. Lets a
    /// dormant slot value re-enter circulation as its producer's own carrier.
    pub fn unseal(self) -> Witnessed<T, W> {
        self.inner
    }

    /// The bundled witness — the carrier's reach description, read without consuming.
    pub fn witness(&self) -> &W {
        self.inner.witness()
    }

    /// The bundled `Erased<T>`, read without consuming — the value stays erased, so no lifetime is
    /// fabricated here.
    pub(crate) fn erased(&self) -> &Erased<T> {
        &self.inner.value
    }

    /// Duplicate the retained carrier, leaving this one intact — the consumer-pull copy.
    pub fn duplicate(&self) -> Self
    where
        Erased<T>: Copy,
        W: Clone,
    {
        Retained {
            inner: self.inner.duplicate(),
        }
    }

    /// Read under an externally supplied pin, at a **rank-2** (`for<'b>`) brand. Crate-internal by
    /// design: this is the door where "does this pin cover this carrier?" is unchecked, so its
    /// contract is that the pin is derived from the structure that owns the retention rather than
    /// accepted from outside. An embedder never reaches it, which is the whole point of the
    /// [`Sealed`]/`Retained` split.
    pub(crate) fn open_with<Wx: Witness, R>(
        &self,
        pin: &Wx,
        f: impl for<'b> FnOnce(T::At<'b>) -> R,
    ) -> R
    where
        T::At<'static>: Copy,
    {
        let _ = pin;
        f(self.inner.read_pinned())
    }

    /// [`Self::open_with`] handing `f` the re-anchored value **by reference**, for a value family
    /// whose views are not `Copy`. Same siting and same soundness story.
    pub(crate) fn open_ref_with<Wx: Witness, R>(
        &self,
        pin: &Wx,
        f: impl for<'b> FnOnce(&'b T::At<'b>) -> R,
    ) -> R {
        self.inner.with_pinned(pin, f)
    }

    /// Open into the **in-use** [`Opened`] state at the pin borrow's lifetime `'b`. Crate-internal
    /// for [`Self::open_with`]'s reason.
    pub(crate) fn open_at_with<'b, Pin: Witness>(&'b self, pin: &'b Pin) -> Opened<'b, T, W>
    where
        T::At<'static>: Copy,
        W: Clone,
    {
        let _ = pin;
        Opened {
            value: self.inner.read_pinned(),
            witness: self.inner.witness().clone(),
        }
    }

    /// **Re-brand this carrier at a live pin's borrow** — the public door back into [`Sealed`]'s
    /// pin-free reads, for a holder that has a retained cell and a pin covering it for some `'b`.
    /// Exactly a pinned open followed by [`Opened::reseal`], so it grants no capability that
    /// composition did not: the brand is the *pin's* borrow, so every read of the returned seal is
    /// covered for as long as the pin is held and no longer.
    ///
    /// Where the crate-internal `brand_to` brands against a region that has taken the value's
    /// coverage over for good, this brands against a pin the caller holds across a bounded stretch —
    /// an embedder's step, whose coverage pins every region its deps reach. The cell keeps
    /// referencing the description its producer stamped; nothing is minted and nothing moves.
    pub fn brand_with<'b, Pin: Witness>(&'b self, pin: &'b Pin) -> Sealed<'b, T, W>
    where
        T::At<'static>: Copy,
        W: Clone,
    {
        self.open_at_with(pin).reseal()
    }
}

/// A seal over a bit-copy value family and a bit-copy witness — the reference-only [`Carrier`],
/// never an owned `PinBundle` — is itself `Copy` and **`Drop`-free**, so a dormant carrier rests
/// inside an embedder's own `Copy` value (a resolved sub-result at rest in an expression part)
/// rather than making that value heap-shaped. Copying is exactly [`Self::duplicate`] — the erased
/// value bit-copied, the witness duplicated — so it grants no capability the seal did not already
/// have; a witness that owns pins does not meet `W: Copy`, so a pin is never silently duplicated.
///
/// What a copied seal does **not** carry is coverage: the witness pins nothing. Each copy keeps the
/// `'home` brand, which is what makes its reads pin-free, and the storage that keeps their pointee
/// alive is the region that brand names ([`Delivered::rest_in`](delivered::Delivered::rest_in) is
/// the door that lodges it there).
///
/// Manual rather than derived: a derive would bound `T: Copy` (the family marker, which is never
/// `Copy`) instead of the erased value.
impl<'home, T: Reattachable + DropFree, W: Copy> Clone for Sealed<'home, T, W>
where
    T::At<'static>: Copy,
{
    fn clone(&self) -> Self {
        *self
    }
}
impl<'home, T: Reattachable + DropFree, W: Copy> Copy for Sealed<'home, T, W> where
    T::At<'static>: Copy
{
}

/// The **in-use** carrier state: a value re-anchored at a step lifetime `'b`, paired with its reach
/// witness, produced by [`Sealed::open_at`] / [`Delivered::open_at`](delivered::Delivered::open_at)
/// and returned to rest by [`Self::reseal`]. It is the borrow-tied twin of the rank-2 read verbs —
/// where [`Sealed::open`] confines the re-anchored value inside a `for<'b>` closure, an `Opened`
/// carries it out at the concrete borrow lifetime `'b`, so a step opens once and reads freely for
/// the frame's duration.
///
/// It borrows at `'b` and **pins nothing** — the value's backing is kept alive by the frame the
/// opening `pin` borrowed, not by the `Opened` (`W` is the reach witness, reference-only for the
/// collapsed [`Carrier`]). Every constructor is library-internal, so the value↔reach pairing it
/// carries is never fabricated by a caller, and [`Self::reseal`] on an open taken from a seal
/// returns exactly the seal the value came from.
pub struct Opened<'b, T: Reattachable, W> {
    value: T::At<'b>,
    witness: W,
}

// Manual impls: a derive would over-constrain (`T: Clone/Copy` rather than `T::At<'b>: Copy`),
// mirroring the manual carrier impls elsewhere in the module.
impl<'b, T: Reattachable, W: Clone> Clone for Opened<'b, T, W>
where
    T::At<'b>: Copy,
{
    fn clone(&self) -> Self {
        Opened {
            value: self.value,
            witness: self.witness.clone(),
        }
    }
}
impl<'b, T: Reattachable, W: Copy> Copy for Opened<'b, T, W> where T::At<'b>: Copy {}

impl<'b, T: Reattachable + DropFree, W> Opened<'b, T, W> {
    /// The re-anchored value, borrowed at the step lifetime `'b`. `Copy` families hand it back by
    /// value; the borrow checker keeps it inside `'b` (the opening pin's borrow), so it cannot
    /// outlive the frame that pins its backing.
    pub fn value(&self) -> T::At<'b>
    where
        T::At<'b>: Copy,
    {
        self.value
    }

    /// The reach witness this open carries — the value's reach description, unchanged from the seal
    /// it was opened from.
    pub fn witness(&self) -> &W {
        &self.witness
    }

    /// The re-anchored value, consuming the open — the by-move twin of [`Self::value`] for a family
    /// whose live form is not `Copy`, and the tail [`Delivered::adopt_into`](delivered::Delivered::adopt_into)
    /// reads its adopted value back through.
    pub fn into_value(self) -> T::At<'b> {
        self.value
    }

    /// Bundle a value the **library itself** re-anchored at `'b` with the witness describing it,
    /// where `'b` is the destination region's own lifetime rather than a borrow of a pin. Three
    /// doors reach it: [`Delivered::open_adopted`](delivered::Delivered::open_adopted),
    /// [`Sectioned::project`] and [`FoldedPlacement::fold_and_bump`]. Crate-internal to `witnessed`,
    /// and each door retains the pins covering `'b` *before* it hands the value over, so the
    /// value↔reach pairing an `Opened` carries is still never fabricated by a caller.
    pub(in crate::witnessed) fn adopted(value: T::At<'b>, witness: W) -> Self {
        Opened { value, witness }
    }

    /// Return the value to rest as a [`Sealed`] — the step-end re-seal. Sound because an `Opened` is
    /// `Copy` and constructible only by opening a seal or delivery: re-erasing the value under the
    /// witness it was opened with reconstitutes exactly that carrier's value↔reach pairing, never a
    /// fabricated one. Safe: the value stays a lifetime-only re-erase to `'static` for storage (no
    /// reattach), so this adds no `unsafe`.
    pub fn reseal(self) -> Sealed<'b, T, W> {
        Sealed::from_opened(Witnessed::from_erased(
            Erased::erase(self.value),
            self.witness,
        ))
    }
}

/// The **externally-witnessed** dormant form: an erased carrier that bundles *no* witness, opened by
/// supplying one at the access. Where [`Sealed`] bundles `W` (and so [`Sealed::open`] reads the pin
/// from the bundle), `SealedExtern` carries the carrier alone — the holder already pins the backing
/// and hands a borrow of the witness in at [`open`](Self::open). This is the form for a carrier whose
/// witness the holder must *not* duplicate: bundling a clone of a reference-counted cart would
/// extend its lifetime beyond the holder's own drop. It wraps an [`Erased`] rather than re-storing
/// the retype, so [`retype`] stays the single audited reattach home.
///
/// Its [`open`](Self::open) is **consuming** (takes `self`), so a non-`Copy` carrier — a
/// `Box<dyn FnOnce>` continuation — passes where [`Sealed::open`]'s `At<'static>: Copy` excludes it;
/// and several can be combined under one brand with [`zip`](Self::zip) so heterogeneous carriers
/// witnessed by the same pin open together (the run-loop step's continuation / contract / region).
pub struct SealedExtern<T: Reattachable + DropFree> {
    value: Erased<T>,
}

impl<T: Reattachable + DropFree> SealedExtern<T> {
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
    pub fn zip<U: Reattachable + DropFree>(
        self,
        other: SealedExtern<U>,
    ) -> SealedExtern<And<T, U>> {
        SealedExtern {
            value: Erased::from_static((self.value.into_static(), other.value.into_static())),
        }
    }

    /// The wrapped [`Erased`] — a field move, for the module-internal opens that re-anchor an
    /// externally-witnessed operand themselves ([`SealedPinned::open`]). Adds no `unsafe`: the
    /// value stays erased through the move.
    pub(in crate::witnessed) fn into_erased(self) -> Erased<T> {
        self.value
    }
}

impl<T: Reattachable + DropFree> Clone for SealedExtern<T>
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
impl<T: Reattachable + DropFree> Copy for SealedExtern<T> where T::At<'static>: Copy {}

/// Seal an **optional** already-erased carrier into the externally-witnessed dormant form, folding the
/// `Option` *inside* the seal as an [`OptionOf`] carrier — so an optional operand (the run-loop's
/// frame-gated return contract) can [`zip`](SealedExtern::zip) into a combined open and arrive as
/// `Option<T::At<'b>>` at the brand. A pure-data rewrap of `Option<Erased<T>>` into
/// `Erased<OptionOf<T>>` (both are `'static`-erased), so it carries no `unsafe`.
pub fn seal_option<T: Reattachable + DropFree>(
    value: Option<Erased<T>>,
) -> SealedExtern<OptionOf<T>> {
    SealedExtern {
        value: Erased::from_static(value.map(Erased::into_static)),
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

/// A pair of drop-free erased forms is drop-free — the componentwise `DropFree` certification, so
/// a zipped operand rests in the Copy tier exactly when both halves do.
impl<A: DropFree, B: DropFree> DropFree for And<A, B> {}

/// `Option` of a carrier family, re-anchored as one — the family [`seal_option`] seals so an
/// **optional** operand opens to `Option<T::At<'b>>` at the brand. Layout-invariant in `'r` because
/// an `Option` of a layout-invariant family is itself layout-invariant.
pub struct OptionOf<T>(PhantomData<T>);

// SAFETY: `Option<T::At<'r>>` is one type up to `'r` when `T` is — the `Reattachable` contract,
// discharged through the inner family.
unsafe impl<T: Reattachable> Reattachable for OptionOf<T> {
    type At<'r> = Option<T::At<'r>>;
}

/// An `Option` of a drop-free erased form is drop-free — the `DropFree` certification through the
/// inner family, so an optional operand rests in the Copy tier exactly when its payload does.
impl<T: DropFree> DropFree for OptionOf<T> {}

/// A **shared reference to** a carrier family — `&'r T::At<'r>`, the co-located shape a region
/// allocation hands back. It is what lets a door erase the *reference* it just bumped without a
/// family declaration per stored family: the bump door
/// ([`RegionHandle::bump_born_with`]) builds its value at a fold brand and re-anchors the reference
/// out through this family, and an embedder whose carrier is `&'r Scope<'r>` names it instead of
/// declaring a reference family of its own beside every value family.
///
/// Note the two lifetimes move together: `At<'r>` is `&'r T::At<'r>` (borrow == content), the tight
/// shape with no free content lifetime a holder could widen past its pin.
pub struct ReferenceFamily<T>(PhantomData<T>);

// SAFETY: `&'r T::At<'r>` is a thin/fat pointer whose layout is identical for every choice of `'r`
// when `T` is layout-invariant — the `Reattachable` contract, discharged through the inner family.
unsafe impl<T: Reattachable> Reattachable for ReferenceFamily<T> {
    type At<'r> = &'r T::At<'r>;
}

/// A shared reference needs no drop, whatever it points at — so a reference family rests in the
/// Copy tier even where its pointee family does not.
impl<T> DropFree for ReferenceFamily<T> {}
