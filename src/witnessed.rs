//! `Witnessed<T, W>` and the lifetime-erasure substrate it is built on — the single audited owner
//! of the erase-to-`'static` / reattach-to-`'r` discipline every lifetime-free inter-node carrier
//! shares. It sits below both [`machine`](crate::machine) and [`scheduler`](crate::scheduler) and
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
//! co-location holds by construction, and [`Witnessed::merge`] combines two under one brand and
//! re-seals under the witness that pins both. For storage *between* accesses a carrier rests in a
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
pub use region::{Region, StorageProfile, Stored};

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
macro_rules! reattachable {
    ($($family:ty => $at:ty),+ $(,)?) => {$(
        // SAFETY: see the macro docs — `$family`'s `At<'r>` is layout-invariant in `'r`.
        unsafe impl $crate::witnessed::Reattachable for $family {
            type At<'r> = $at;
        }
    )+};
}
pub(crate) use reattachable;

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
pub(crate) fn erase_to_static<T: Reattachable>(value: T::At<'_>) -> T::At<'static> {
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

/// Generic owner of an erased carrier: a one-lifetime-family value with its lifetime forgotten to
/// `'static` for storage on a lifetime-free node slot. [`Self::erase`] stores; the value is
/// re-anchored either through a [`Witnessed`] that bundles its witness, or transiently through the
/// externally-witnessed [`SealedExtern::open`] (routing [`Self::reattach`]) against a borrowed witness.
/// The single audited home for the carrier families; see the module docs.
pub(crate) struct Erased<T: Reattachable> {
    inner: T::At<'static>,
}

impl<T: Reattachable> Erased<T> {
    /// Erase a live carrier to its storable `'static` form. Safe: forgetting a lifetime for
    /// storage cannot fabricate one — the value is stored, never used at `'static`, until a
    /// witnessed re-anchor.
    pub(crate) fn erase(live: T::At<'_>) -> Self {
        Erased {
            inner: erase_to_static::<T>(live),
        }
    }

    /// Re-anchor the carrier to a caller-chosen `'r` without a bundled witness — the raw fabrication
    /// the externally-witnessed [`SealedExtern::open`] wraps behind its rank-2 brand, supplying the pin
    /// at the access. The bundled-witness accessors ([`Witnessed::with`] / [`Witnessed::map`]) route
    /// the same brand-retype directly instead.
    ///
    /// # Safety
    ///
    /// The caller holds a liveness witness — the carrier's frame `Rc`, or the run region — that pins
    /// the pointee for all of `'r`, and re-anchors only transiently while that witness is held, so
    /// the fabricated `'r` cannot outlive the pointee. `'r` is driven by the return-type annotation.
    pub(crate) unsafe fn reattach<'r>(self) -> T::At<'r> {
        // SAFETY: see the method contract; lifetime-only retype of a single-lifetime family.
        unsafe { retype::<T::At<'static>, T::At<'r>>(self.inner) }
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
/// at a stable heap address — a [`StableDeref`]); a frameless terminal whose pointee is backed by a
/// region that outlives the carrier qualifies via [`Option`] (`None`).
pub unsafe trait Witness {}

// SAFETY: `Rc<F>` is `StableDeref` — the `F` it owns lives at a fixed heap address for the whole
// life of the `Rc`, and cloning or moving the `Rc` does not move the `F`. The static bound below
// records that obligation as a checked fact rather than prose.
unsafe impl<F> Witness for Rc<F> {}
const _: fn() = || {
    fn assert_stable_deref<P: StableDeref>() {}
    let _ = assert_stable_deref::<Rc<()>>;
};

// SAFETY: `Some(w)` pins through the inner witness `w`; `None` carries no witness because its
// pointee is backed by a region that outlives the carrier (the frameless / run-region terminal),
// so no held pin is required. Either way the carrier's pointee outlives a read bounded by `&self`.
unsafe impl<W: Witness> Witness for Option<W> {}

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

/// A [`Witness`] whose values compose to the one that pins **both** operands' regions — the seam
/// [`Witnessed::merge`] routes to seal a combined carrier under the tightest correct witness.
///
/// The motivating shape is a *set* of region owners: a value can reach several regions, so its
/// witness is the set of frame `Rc`s pinning them, and two witnesses compose by **set union** —
/// dropping a member whose region another member's ancestor (`outer`) chain already pins
/// (subsumption). A single-region witness is the degenerate case: the union of two *related* carts
/// collapses to the descendant (whose `outer` chain pins the ancestor), while two *unrelated*
/// single-region carts have no common representable pin, so [`Self::merge`] returns `None`. A set
/// witness can always represent the union, so it never returns `None`.
///
/// # Safety
///
/// When [`Self::merge`] returns `Some(w)`, holding `w` must keep both `left`'s and `right`'s pinned
/// regions live for as long as `w` is held. `None` asserts no value of `Self` pins both — the only
/// safe verdict when this witness type cannot represent the combined pin.
pub unsafe trait MergeWitness: Witness + Sized {
    /// The witness pinning both `left`'s and `right`'s regions (set union with `outer`-chain
    /// subsumption), or `None` when this witness type cannot represent a value pinning both.
    fn merge(left: &Self, right: &Self) -> Option<Self>;
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

impl<T: Reattachable, W: Witness> Witnessed<T, W> {
    /// Bundle a live carrier with the witness that pins it, erasing the carrier for storage. Safe:
    /// the erase cannot fabricate a lifetime, and the witness records the liveness obligation the
    /// later re-anchor relies on.
    ///
    /// Co-location — that the witness pins *this* value's references — is **caller-asserted** here: the
    /// value and witness arrive independently. Reserve `new` for carriers whose co-location is already
    /// structural (lifetime-free carriers, or a value already living in a region the witness pins);
    /// prefer [`Self::yoke`], which sources the carrier from the witness's region and so discharges
    /// co-location by construction.
    pub fn new(value: T::At<'_>, witness: W) -> Self {
        Self::from_erased(Erased::erase(value), witness)
    }

    /// Bundle an **already-erased** carrier with its witness. The `'static`-erased input carries no
    /// lifetime, so unlike [`Self::new`] it leaves no input lifetime for inference to pick: it is the
    /// constructor for a `Result::map(Erased::erase)` pipeline, where threading the live value's
    /// lifetime through a closure would otherwise let it default to `'static`.
    pub(crate) fn from_erased(value: Erased<T>, witness: W) -> Self {
        Witnessed { value, witness }
    }

    /// Bundle a **region-pure** value under the default (empty / pins-nothing) witness — the honest
    /// constructor for a value built inside an alloc brand that references no foreign region. Where
    /// [`Self::new`] pairs an *arbitrary* value with an *arbitrary* witness (co-location asserted in
    /// prose), `resident` fixes the witness to `W::default()` — the empty set for a [`FrameSet`]-style
    /// set witness, `None` for an [`Option`] witness — so it **cannot** pair a value with a *wrong*
    /// non-empty witness; the only obligation it carries is that `value`'s foreign reach is genuinely
    /// empty. It is what lets the brand-confined region allocator return a witnessed carrier without
    /// reaching for `new`'s open-ended assertion.
    ///
    /// Because the default witness pins nothing, the carrier is a **within-step transient**: sound only
    /// while the active frame pins its region externally, until the producer is folded into the witness
    /// at finalize/close ([`Self::reseal_under`]) before the carrier is stored on a node. A value that
    /// *references* another region is the [`yoke`](Self::yoke) / [`merge`](Self::merge) path, which
    /// sources or folds that region's pin instead.
    ///
    /// Safe for the same reason as [`Self::new`]: the erase cannot fabricate a lifetime. `W::default()`
    /// is the pins-nothing element of the witness type (the empty set / `None`).
    pub(crate) fn resident(value: T::At<'_>) -> Self
    where
        W: Default,
    {
        Self::from_erased(Erased::erase(value), W::default())
    }

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
    /// ```compile_fail
    /// use koan::witnessed::{Reattachable, Witness, WitnessRegion, Witnessed};
    /// use std::rc::Rc;
    ///
    /// struct RefFamily;
    /// // SAFETY: `&'r u32` is one type generic only in `'r`.
    /// unsafe impl Reattachable for RefFamily {
    ///     type At<'r> = &'r u32;
    /// }
    /// struct Cart(Vec<u32>);
    /// // SAFETY: `Rc<Cart>` is `StableDeref`; its region is the owned `Vec`.
    /// unsafe impl Witness for Rc<Cart> {}
    /// unsafe impl WitnessRegion for Rc<Cart> {
    ///     type Region = [u32];
    ///     fn region(&self) -> &[u32] { &self.0 }
    /// }
    ///
    /// let outside: u32 = 7;
    /// let cart: Rc<Cart> = Rc::new(Cart(vec![1, 2, 3]));
    /// // Try to yoke a borrow of `outside` (not region-derived) — rejected by the `for<'b>` brand.
    /// let _: Witnessed<RefFamily, Rc<Cart>> = Witnessed::yoke(cart, |_region| &outside);
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

    /// Read the carrier: re-anchor it behind a **rank-2** (`for<'b>`) closure, so the fabricated
    /// content lifetime is universally quantified and nothing `'b`-flavoured can be captured into
    /// `R` and outlive the witness pin (the generativity / ghost-cell trick). The naive
    /// borrow-bounded / content-free form is a Miri-proven use-after-free; this signature is the fix.
    ///
    /// The brand is load-bearing: copying a branded reference out of the closure (here
    /// `Cell::get`, whose `&u32` would otherwise escape past the witness drop) fails to compile,
    /// because `R` cannot mention the universally-quantified `'b`.
    ///
    /// ```compile_fail
    /// use koan::witnessed::{Reattachable, Witnessed};
    /// use std::cell::Cell;
    /// use std::rc::Rc;
    ///
    /// struct InvFamily;
    /// // SAFETY: `Cell<&'r u32>` is one type generic only in `'r`.
    /// unsafe impl Reattachable for InvFamily {
    ///     type At<'r> = Cell<&'r u32>;
    /// }
    ///
    /// let backing: Rc<Vec<u32>> = Rc::new(vec![42]);
    /// let w: Witnessed<InvFamily, Rc<Vec<u32>>> =
    ///     Witnessed::new(Cell::new(&backing[0]), Rc::clone(&backing));
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
    /// The `PhantomData<&'b ()>` argument is load-bearing, not decoration: without an input
    /// mentioning `'b`, `impl for<'b> FnOnce(..) -> P::At<'b>` is rejected (`E0582`), since the brand
    /// would appear only in the output GAT projection. This is exactly `yoke::map_project`'s shape.
    ///
    /// The brand also seals `map`: a projection cannot stash a branded reference into an outer slot
    /// to be read after the witness drops — the `for<'b>` quantifier rejects it at compile time.
    ///
    /// ```compile_fail
    /// use koan::witnessed::{Reattachable, Witnessed};
    /// use std::rc::Rc;
    ///
    /// struct RefFamily;
    /// // SAFETY: `&'r u32` is one type generic only in `'r`.
    /// unsafe impl Reattachable for RefFamily {
    ///     type At<'r> = &'r u32;
    /// }
    ///
    /// let backing: Rc<Vec<u32>> = Rc::new(vec![5]);
    /// let w: Witnessed<RefFamily, Rc<Vec<u32>>> = Witnessed::new(&backing[0], Rc::clone(&backing));
    /// let mut stolen: Option<&u32> = None;
    /// // Try to capture the branded `&'b u32` into a longer-lived slot — rejected by `for<'b>`.
    /// let _ = w.map::<RefFamily>(|r, _brand| {
    ///     stolen = Some(r);
    ///     r
    /// });
    /// println!("{}", *stolen.unwrap());
    /// ```
    pub fn map<P: Reattachable>(
        self,
        f: impl for<'b> FnOnce(T::At<'b>, PhantomData<&'b ()>) -> P::At<'b>,
    ) -> Witnessed<P, W> {
        let Witnessed { value, witness } = self;
        // SAFETY: re-anchor the erased carrier at a fresh existential brand the `for<'b>` closure
        // cannot leak; the projected result is immediately re-erased to `'static` for storage under
        // the same witness, which still pins the backing. Lifetime-only retype of a single-lifetime
        // family.
        let live: T::At<'_> = unsafe { retype::<T::At<'static>, T::At<'_>>(value.inner) };
        let projected = f(live, PhantomData);
        Witnessed {
            value: Erased::erase(projected),
            witness,
        }
    }

    /// Combine two witnessed carriers under one brand and re-seal the result under the witness that
    /// pins **both** — the composition law for [`Witnessed`]. The two carriers are re-anchored at a
    /// shared `for<'b>` brand and handed to `f`, which may bind one into the other (e.g. a witnessed
    /// `KFunction` into a witnessed `Scope`); the projection is then sealed under
    /// [`MergeWitness::merge`] of the two witnesses — the set union (with `outer`-chain subsumption)
    /// that keeps every region the combined carrier reaches live.
    ///
    /// Returns `None` only when that union is **not representable** in `W` — a single-region witness
    /// whose two operands are unrelated (see [`MergeWitness::merge`]); a set witness always succeeds.
    /// The composability verdict is taken *before* `f` runs, so an unsound combination is never built.
    ///
    /// Sound for the same reason as [`Self::map`], doubled: both source witnesses are held for the
    /// whole of `f`, so re-anchoring both carriers to one brand `'b` cannot dangle; the `for<'b>`
    /// quantifier keeps either branded carrier from escaping into the result type, and the combined
    /// witness pins the sealed carrier's backing thereafter.
    ///
    /// ```compile_fail
    /// use koan::witnessed::{MergeWitness, Reattachable, Witness, Witnessed};
    /// use std::marker::PhantomData;
    /// use std::rc::Rc;
    ///
    /// struct RefFamily;
    /// // SAFETY: `&'r u32` is one type generic only in `'r`.
    /// unsafe impl Reattachable for RefFamily {
    ///     type At<'r> = &'r u32;
    /// }
    /// // SAFETY: `Rc<Vec<u32>>` is `StableDeref`; ancestry is trivial (every cart pins itself), so
    /// // the combined witness is just a clone of the left operand.
    /// unsafe impl MergeWitness for Rc<Vec<u32>> {
    ///     fn merge(left: &Self, _right: &Self) -> Option<Self> { Some(Rc::clone(left)) }
    /// }
    ///
    /// let a: Rc<Vec<u32>> = Rc::new(vec![1]);
    /// let b: Rc<Vec<u32>> = Rc::new(vec![2]);
    /// let wa: Witnessed<RefFamily, Rc<Vec<u32>>> = Witnessed::new(&a[0], Rc::clone(&a));
    /// let wb: Witnessed<RefFamily, Rc<Vec<u32>>> = Witnessed::new(&b[0], Rc::clone(&b));
    /// let mut stolen: Option<&u32> = None;
    /// // Try to capture a branded `&'b u32` into a longer-lived slot — rejected by `for<'b>`.
    /// let _ = wa.merge::<RefFamily, RefFamily>(wb, |l, _r, _brand: PhantomData<&_>| {
    ///     stolen = Some(l);
    ///     l
    /// });
    /// println!("{}", *stolen.unwrap());
    /// ```
    pub fn merge<B: Reattachable, P: Reattachable>(
        self,
        other: Witnessed<B, W>,
        f: impl for<'b> FnOnce(T::At<'b>, B::At<'b>, PhantomData<&'b ()>) -> P::At<'b>,
    ) -> Option<Witnessed<P, W>>
    where
        W: MergeWitness,
    {
        // Composability first: the combined witness must pin both regions, or there is no sound
        // result — so compute it before `f` builds a value that would reference a region no surviving
        // witness keeps live. The source witnesses below stay held across `f`.
        let witness = W::merge(&self.witness, &other.witness)?;
        let Witnessed {
            value: left,
            witness: left_witness,
        } = self;
        let Witnessed {
            value: right,
            witness: right_witness,
        } = other;
        // SAFETY: both source witnesses are held across `f`, each pinning its own carrier's backing;
        // the two carriers are re-anchored to one existential brand the `for<'b>` closure cannot leak,
        // and the projection is immediately re-erased to `'static` for storage. The combined `witness`
        // (set union with subsumption) pins both regions thereafter. Lifetime-only retypes of
        // single-lifetime families.
        let live_left: T::At<'_> = unsafe { retype::<T::At<'static>, T::At<'_>>(left.inner) };
        let live_right: B::At<'_> = unsafe { retype::<B::At<'static>, B::At<'_>>(right.inner) };
        let projected = f(live_left, live_right, PhantomData);
        // The source witnesses pinned both backings across `f`; drop them now — the combined `witness`
        // computed above carries both pins forward.
        drop(left_witness);
        drop(right_witness);
        Some(Witnessed {
            value: Erased::erase(projected),
            witness,
        })
    }

    /// Fold an extra witness set into the bundled one, re-sealing the carrier under the union that
    /// pins **both** — the witness-only counterpart to [`Self::merge`] (no value transform, no second
    /// carrier). The producer-frame fold at finalize/close routes this: a foreign-reach-only carrier
    /// (born under the empty set, its producing frame deliberately excluded) has that frame folded in
    /// here before storage, the [`MergeWitness::merge`] set union pinning the value's backing
    /// thereafter. Idempotent when `extra` is already subsumed by the bundled witness (the folded
    /// frame is one the set's `outer` chains already pin), so re-sealing a self-witnessed carrier
    /// changes nothing.
    pub fn reseal_under(self, extra: W) -> Self
    where
        W: MergeWitness,
    {
        let Witnessed { value, witness } = self;
        let witness = W::merge(&witness, &extra)
            .expect("reseal_under: a set witness always represents the union of the resealed sets");
        Witnessed { value, witness }
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
    pub fn read(&self) -> T::At<'_>
    where
        T::At<'static>: Copy,
    {
        // SAFETY: the bundled `witness` pins the pointee for the whole `&self` borrow (dropping it
        // needs `&mut self`), and the returned carrier is bounded by that borrow, so it cannot
        // outlive the pin. Lifetime-only retype of a single-lifetime family; the `Copy` bound copies
        // the erased carrier out of `&self` before re-anchoring.
        unsafe { retype::<T::At<'static>, T::At<'_>>(self.value.inner) }
    }

    /// Duplicate the carrier: copy the erased value (a `Copy` carrier family — a thin/fat reference)
    /// and clone the witness. The seam [`Sealed::transfer_into`] uses to relocate a value out of a
    /// `&self` seal without consuming it, so the producer keeps its terminal for other consumers.
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

    /// The bundled witness — the producer frame `Rc` (possibly wrapped in [`Option`]) that pins the
    /// carrier's pointee. Cloned out by the consumer-pull lift to keep the backing region alive
    /// while the value is copied into the consumer's frame.
    pub fn witness(&self) -> &W {
        &self.witness
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

impl<T: Reattachable, W: Witness> Sealed<T, W> {
    /// Seal a live [`Witnessed`] into its dormant storage form — the only way in. `Sealed` exposes no
    /// other constructor and no transform once sealed, so a value can re-enter circulation only
    /// through an accessor below.
    pub fn seal(witnessed: Witnessed<T, W>) -> Self {
        Sealed { inner: witnessed }
    }

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
    /// ```compile_fail
    /// use koan::witnessed::{Reattachable, Sealed, Witnessed};
    /// use std::rc::Rc;
    ///
    /// struct RefFamily;
    /// // SAFETY: `&'r u32` is one type generic only in `'r`.
    /// unsafe impl Reattachable for RefFamily {
    ///     type At<'r> = &'r u32;
    /// }
    ///
    /// let backing: Rc<Vec<u32>> = Rc::new(vec![42]);
    /// let sealed: Sealed<RefFamily, Rc<Vec<u32>>> =
    ///     Sealed::seal(Witnessed::new(&backing[0], Rc::clone(&backing)));
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

    /// Relocate the sealed carrier into a destination region and re-seal it under the witness that
    /// pins **both** — the borrow-checked replacement for the consumer-pull lift's one open-coded
    /// reattach. Built from [`Witnessed::merge`]: the bundled carrier is [`duplicated`](Witnessed::duplicate)
    /// (the `&self` seal is left intact for other consumers), re-anchored at a shared `for<'b>` brand
    /// with `dest`, and handed to `relocate` — the workload's structural copy, which allocs into
    /// `dest` at the brand **natively** (no fabricated lifetime); the projection is then re-sealed
    /// under [`MergeWitness::merge`] of this carrier's witness and `dest`'s — the set union of every
    /// region the relocated value reaches (its retained sources ∪ the destination).
    ///
    /// Returns `None` only when that union is not representable in `W` (see [`MergeWitness::merge`]);
    /// a set witness always succeeds. Because it routes `merge`'s already-audited retype it **adds no
    /// `unsafe`**, and because the value lands at the destination region's own lifetime there is **no
    /// fabricated lifetime** at the call site — a soundness the type system enforces, not one a
    /// hand-written reattach must assert in prose.
    pub fn transfer_into<B: Reattachable, P: Reattachable>(
        &self,
        dest: Witnessed<B, W>,
        relocate: impl for<'b> FnOnce(T::At<'b>, B::At<'b>, PhantomData<&'b ()>) -> P::At<'b>,
    ) -> Option<Witnessed<P, W>>
    where
        W: MergeWitness + Clone,
        T::At<'static>: Copy,
    {
        self.inner.duplicate().merge::<B, P>(dest, relocate)
    }

    /// Duplicate the sealed carrier — copy the erased value (a `Copy` carrier family) and clone the
    /// witness — leaving this seal intact. The consumer-pull lift hands each construction finish a
    /// duplicate of the producer slot's own carrier (so the dep arrives **witnessed**, its reach named,
    /// rather than re-wrapped via an asserted [`Witnessed::new`]); the producer keeps its terminal for
    /// other consumers. Routes [`Witnessed::duplicate`], so it adds no `unsafe`.
    pub(crate) fn duplicate(&self) -> Self
    where
        Erased<T>: Copy,
        W: Clone,
    {
        Sealed {
            inner: self.inner.duplicate(),
        }
    }

    /// The bundled witness — the producer frame `Rc` (possibly wrapped in [`Option`]) that pins the
    /// carrier's pointee. Cloned out by the consumer-pull lift to keep the backing region alive while
    /// the value is copied into the consumer's frame. Hands back the witness, not the value, so it
    /// does not weaken opacity.
    pub fn witness(&self) -> &W {
        self.inner.witness()
    }
}

/// The **externally-witnessed** dormant form: an erased carrier that bundles *no* witness, opened by
/// supplying one at the access. Where [`Sealed`] bundles `W` (and so [`Sealed::open`] reads the pin
/// from the bundle), `SealedExtern` carries the carrier alone — the holder already pins the backing
/// and hands a borrow of the witness in at [`open`](Self::open). This is the form for a carrier whose
/// witness the holder must *not* duplicate: bundling a clone of a reference-counted cart would peg the
/// holder's own `Rc::get_mut` uniqueness check (the TCO frame-reuse gate). It wraps an [`Erased`]
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
    pub(crate) fn seal(value: Erased<T>) -> Self {
        SealedExtern { value }
    }

    /// Erase a **live** carrier directly into the dormant form — the entry for a value re-anchored at
    /// the access rather than recovered from node storage (the run-loop `dest` region). Safe for the
    /// same reason as [`Erased::erase`]: forgetting a lifetime for storage cannot fabricate one.
    pub(crate) fn erase(live: T::At<'_>) -> Self {
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
    /// ```compile_fail
    /// use koan::witnessed::{Reattachable, SealedExtern};
    /// use std::rc::Rc;
    ///
    /// struct RefFamily;
    /// // SAFETY: `&'r u32` is one type generic only in `'r`.
    /// unsafe impl Reattachable for RefFamily {
    ///     type At<'r> = &'r u32;
    /// }
    ///
    /// let backing: Rc<Vec<u32>> = Rc::new(vec![42]);
    /// let sealed: SealedExtern<RefFamily> = SealedExtern::erase(&backing[0]);
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
    pub(crate) fn zip<U: Reattachable>(self, other: SealedExtern<U>) -> SealedExtern<And<T, U>> {
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
pub(crate) fn seal_option<T: Reattachable>(value: Option<Erased<T>>) -> SealedExtern<OptionOf<T>> {
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

