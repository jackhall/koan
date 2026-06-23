//! `Witnessed<T, W>` and the lifetime-erasure substrate it is built on — the single audited owner
//! of the erase-to-`'static` / reattach-to-`'r` discipline every lifetime-free inter-node carrier
//! shares. It sits below both [`machine`](crate::machine) and [`scheduler`](crate::scheduler) and
//! names no concrete workload type, so each depends on it for the machinery, not the reverse.
//!
//! A node's slot stores a borrow-carrying value the borrow checker can't lifetime-track: it forgets
//! the borrow's lifetime to `'static` for storage and re-anchors it at a caller-chosen lifetime on
//! read. The re-anchor is sound only while a *liveness witness* — the producer frame `Rc` that pins
//! the pointee — is held. [`Witnessed<T, W>`] bundles the erased value with that witness `W` in one
//! value, so "the witness keeps the value alive" is a type invariant, not a comment: its two
//! accessors, [`Witnessed::with`] (borrow + read) and [`Witnessed::map`] (consume + transform), are
//! rank-2 (`for<'b>`) branded so the fabricated content lifetime cannot escape the witness pin.
//!
//! The layout machinery underneath — the [`Reattachable`] family contract, the private [`retype`]
//! primitive, [`erase_to_static`] and the storable [`Erased<T>`] — is the same single-lifetime
//! retype every carrier family routes. The carrier families ([`Reattachable`] impls) live in the
//! workload beside their own types, so this module stays workload-independent.

use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::rc::Rc;

use stable_deref_trait::StableDeref;

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

/// Generic owner of an erased carrier: a one-lifetime-family value with its lifetime forgotten to
/// `'static` for storage on a lifetime-free node slot. [`Self::erase`] stores; the value is
/// re-anchored either through a [`Witnessed`] that bundles its witness, or transiently through
/// [`reattach_with`] against a borrowed witness. The single audited home for the carrier families;
/// see the module docs.
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
    /// the witnessed accessors wrap. Migrating off this in favour of [`Witnessed::with`] /
    /// [`reattach_with`] is what removes the open-coded reattach call sites.
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

/// A liveness witness bundled into a [`Witnessed`] (or borrowed by [`reattach_with`]): holding it
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
        // SAFETY: the bundled `witness` pins the pointee for the `&self` borrow; the reattached view
        // is handed to a `for<'b>` closure, so the fabricated content lifetime cannot escape the
        // call into `R`. Lifetime-only retype of a single-lifetime family (the `Reattachable`
        // contract); `&T::At<'static>` and `&T::At<'_>` share layout (a thin/fat pointer).
        let reattached: &T::At<'_> =
            unsafe { retype::<&T::At<'static>, &T::At<'_>>(&self.value.inner) };
        f(reattached)
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

    /// Re-anchor the carrier and hand it **out** bounded by the `&self` borrow. The borrow-bounded
    /// sibling of [`Self::with`]: where `with`'s `for<'b>` brand forbids the carrier from escaping the
    /// closure, `read` lets it escape *at the borrow lifetime itself* — the content lifetime is the
    /// `&self` borrow, not a free `'b`, so the caller cannot widen it past the witness pin.
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

    /// The bundled witness — the producer frame `Rc` (possibly wrapped in [`Option`]) that pins the
    /// carrier's pointee. Cloned out by the consumer-pull lift to keep the backing region alive
    /// while the value is copied into the consumer's frame.
    pub fn witness(&self) -> &W {
        &self.witness
    }
}

/// Re-anchor a **live** single-lifetime-family value to the `'w` a borrowed [`Witness`] pins — the
/// witness-explicit replacement for a bare transient reattach. The value is erased and immediately
/// re-anchored at `'w`; the witness borrow bounds `'w`, so the caller cannot pick a `'w` outliving
/// the storage the witness pins.
///
/// The **signature is safe**: the caller supplies a witness whose region the value genuinely lives
/// in (the call-site co-location invariant), and the target `'w` is bounded by the witness borrow
/// `'b` (`'b: 'w`), so the re-anchored view cannot outrun the pin. `'w` is left free of `'b` so the
/// caller can re-anchor to a lifetime *shorter* than the witness borrow (e.g. a step lifetime under a
/// longer-held cart `Rc`). Call sites carry no `unsafe` of their own.
pub(crate) fn reattach_with<'b, 'w, T: Reattachable, W: Witness>(
    value: T::At<'_>,
    _witness: &'b W,
) -> T::At<'w>
where
    'b: 'w,
{
    // SAFETY: `'w` is bounded by the `witness` borrow `'b` (`'b: 'w`), which pins the value's region
    // (the call-site co-location invariant), so the re-anchored view cannot escape the pin. Erase for
    // storage then re-anchor at `'w`; lifetime-only retype of a single-lifetime family.
    let erased = erase_to_static::<T>(value);
    unsafe { retype::<T::At<'static>, T::At<'w>>(erased) }
}

/// Slice twin of [`reattach_with`]: re-anchor a shared slice's element content lifetime to the `'w`
/// a borrowed witness pins, preserving the borrow `'i`.
///
/// The **signature is safe** for the same reason as [`reattach_with`]: `'w` is bounded by the
/// witness borrow, so the elements the witness pins outlive the re-anchored view.
pub(crate) fn reattach_slice_with<'i, 'w, T: Reattachable, W: Witness>(
    slice: &'i [T::At<'_>],
    _witness: &'w W,
) -> &'i [T::At<'w>] {
    // SAFETY: content re-anchored to the witnessed `'w`; the borrow `'i` is preserved. `&[_]` is a
    // fat pointer, retyped lifetime-only; the elements live for `'w` (witness-pinned).
    unsafe { retype::<&'i [T::At<'_>], &'i [T::At<'w>]>(slice) }
}

/// Transient lifetime-retype of an owned single-lifetime-family value — [`Erased::reattach`] for a
/// value re-exposed at a different lifetime in place rather than recovered from storage.
///
/// # Safety
///
/// As [`Erased::reattach`]: the value genuinely lives for the target lifetime (frame-pinned) and is
/// viewed only transiently; the retype is needed because the family is invariant.
pub(crate) unsafe fn reattach_value<'a, 'b, T: Reattachable>(value: T::At<'a>) -> T::At<'b> {
    // SAFETY: see the function contract.
    unsafe { retype::<T::At<'a>, T::At<'b>>(value) }
}

/// Re-anchor a stored [`Erased`] one-shot inter-node carrier against a held frame `Rc` witness — the
/// `Erased`-carrier sibling of [`reattach_with`], for the continuation / return-contract carriers the
/// driver re-anchors once per step against the node's cart `Rc`.
///
/// The **signature is safe** — `'w` is bounded by the `witness` borrow, so the frame the witness
/// pins is live for all of `'w`, and the caller cannot pick a `'w` outliving it.
pub(crate) fn vend_carrier<'w, T: Reattachable, F>(
    erased: Erased<T>,
    _witness: &'w Rc<F>,
) -> T::At<'w> {
    // SAFETY: `'w` is pinned by the `witness` borrow (held across the vend's use); the carrier's
    // captures live in the frame it pins (the structural co-location invariant). Lifetime-only
    // retype of a single-lifetime family, per the `Reattachable` contract.
    unsafe { erased.reattach() }
}

/// Borrowed shared-reference retype: re-expose a `&T::At<'a>` at a different content (and borrow)
/// lifetime in place. The scope-pointer path ([`scope_ptr`](crate::machine::core::scope_ptr)) routes
/// its re-attach through here — that path is branded at the pointer, not bundled with an owned
/// witness, so it needs the bare reference retype rather than a [`Witnessed`] accessor.
///
/// # Safety
///
/// The referent genuinely lives for the target lifetime (frame-pinned or brand-bounded) and is
/// viewed only transiently; the retype is needed because the family is invariant.
pub(crate) unsafe fn reattach_ref<'i, 'o, 'a, 'b, T: Reattachable>(
    reference: &'i T::At<'a>,
) -> &'o T::At<'b> {
    // SAFETY: see the function contract; a reference is a thin pointer, retyped lifetime-only.
    unsafe { retype::<&'i T::At<'a>, &'o T::At<'b>>(reference) }
}
