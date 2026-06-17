//! `Reattachable` / `Erased<T>` — the single audited owner of the erase-to-`'static` /
//! reattach-to-`'run` lifetime discipline every lifetime-free carrier shares.
//!
//! A handful of carriers store a borrow-carrying value on a structure the borrow checker can't
//! lifetime-track — a scheduler node's slot, a per-call `TraceFrame`, the arena's lifetime-free
//! `CallArena`. Each forgets the value's lifetime to `'static` for storage and re-anchors it at a
//! caller-chosen lifetime on read, witnessed by a held `Rc` (a frame cart, the run arena) that
//! pins the pointee. That erase/reattach pair, and the layout-invariance argument that makes it
//! sound, live here once: [`Erased<T>`] for a stored value, the [`reattach_value`] /
//! [`reattach_ref`] / [`reattach_slice`] helpers for a transient re-exposure that is not stored.
//! [`ScopePtr`](super::scope_ptr::ScopePtr) is the same discipline specialized to a `Scope`
//! pointer with an invariance brand.
//!
//! The carrier families ([`Reattachable`] impls) live beside their own types — `Carried`,
//! `KObject`, `ReturnContract`, the `NodeCont` continuation — so this module names no concrete
//! Koan type and the `core → execute` layering is not inverted.
//!
//! A sibling concern lives here too: [`pin_deref`] re-borrows a raw `*const T` whose pointee a heap
//! pin (a frame `Rc`, the owning arena) holds fixed — the self-referential arena-pointer derefs that
//! erase/reattach can't express, because the pointer, not its lifetime, is what's being recovered.

use std::mem::ManuallyDrop;

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

/// The single lifetime-retype primitive: move an `A` out as a `B`, where the caller guarantees `A`
/// and `B` are one type up to a lifetime. Private to this module and reached only through the
/// `Reattachable`-bounded wrappers, so `A` / `B` are always `T::At<_>` for one family — the trait's
/// layout-invariance contract is what makes the bitwise move sound.
///
/// `transmute` can't prove `size_of::<T::At<'a>>() == size_of::<T::At<'b>>()` for an opaque GAT
/// projection, so this goes through `transmute_copy` (which assumes the size equality the contract
/// guarantees) behind a `ManuallyDrop` so the source is not dropped after the move. A `const` assert
/// restores the size check `transmute` would emit — mirroring
/// [`erase_store`](super::storage_frame), the sibling GAT-family erasure.
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

/// Generic owner of an erased carrier: a one-lifetime-family value with its lifetime forgotten to
/// `'static` for storage on a lifetime-free node, arena, or trace frame. [`Self::erase`] stores;
/// [`Self::reattach`] re-anchors it at a caller-chosen lifetime the held witness keeps live. The
/// single audited home for the carrier families; see the module docs.
pub struct Erased<T: Reattachable> {
    inner: T::At<'static>,
}

impl<T: Reattachable> Erased<T> {
    /// Erase a live carrier to its storable `'static` form. Safe: forgetting a lifetime for
    /// storage cannot fabricate one — the value is stored, never used at `'static`, until
    /// [`Self::reattach`] re-anchors it.
    pub(crate) fn erase(live: T::At<'_>) -> Self {
        // SAFETY: lifetime-only retype for storage of a single-lifetime family; the erased value
        // is stored, not used, until `reattach`.
        Erased {
            inner: unsafe { retype::<T::At<'_>, T::At<'static>>(live) },
        }
    }

    /// Re-anchor the carrier to a caller-chosen `'r`. The single fabrication for every carrier.
    ///
    /// # Safety
    ///
    /// The caller holds a liveness witness — the carrier's frame `Rc`, or the run arena — that
    /// pins the pointee for all of `'r`, and re-anchors only transiently while that witness is
    /// held, so the fabricated `'r` cannot outlive the pointee. `'r` is driven by the return-type
    /// annotation, not a turbofish argument.
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

/// Borrowed twin of [`reattach_value`]: retype a shared reference's content (and borrow) lifetime.
///
/// # Safety
///
/// As [`reattach_value`].
pub(crate) unsafe fn reattach_ref<'i, 'o, 'a, 'b, T: Reattachable>(
    reference: &'i T::At<'a>,
) -> &'o T::At<'b> {
    // SAFETY: see the function contract; a reference is a thin pointer, retyped lifetime-only.
    unsafe { retype::<&'i T::At<'a>, &'o T::At<'b>>(reference) }
}

/// Slice twin of [`reattach_ref`]: retype a shared slice's element content lifetime.
///
/// # Safety
///
/// As [`reattach_value`].
pub(crate) unsafe fn reattach_slice<'i, 'o, 'a, 'b, T: Reattachable>(
    slice: &'i [T::At<'a>],
) -> &'o [T::At<'b>] {
    // SAFETY: see the function contract; `&[_]` is a fat pointer, retyped lifetime-only.
    unsafe { retype::<&'i [T::At<'a>], &'o [T::At<'b>]>(slice) }
}

/// Materialize a `&'x T` from a raw `*const T` whose pointee a heap pin keeps fixed in place for
/// `'x` — the audited home for the self-referential `Rc<CallArena>` arena-pointer derefs (the
/// per-call arena and its escape frame) and the functor-result arena pin. Distinct from the
/// `Reattachable` retypes above: those move a *value* between lifetimes; this re-borrows a pointer
/// whose pointee an owning `Rc` (or the frame holding it) cannot relocate or drop while borrowed.
///
/// # Safety
///
/// `ptr` must be non-null, aligned, and point at a live, initialized `T` for all of `'x`; the caller
/// holds the pin (the frame `Rc`, the owning arena) across the borrow. `'x` is driven by the
/// return-type annotation, not a turbofish argument.
pub(crate) unsafe fn pin_deref<'x, T: ?Sized>(ptr: *const T) -> &'x T {
    // SAFETY: see the function contract — the caller's held pin keeps the pointee live for `'x`.
    unsafe { &*ptr }
}

#[cfg(test)]
mod tests {
    //! Miri slate (tree borrows): the single audited [`retype`] primitive every carrier family
    //! routes — exercised through [`Erased`] storage and the [`reattach_value`] / [`reattach_ref`]
    //! / [`reattach_slice`] transient helpers. The owned value here carries a real borrow, so the
    //! erase → reattach → read round-trip pins the lifetime-fabricated read under tree borrows.
    //! Fails on UB, not values.

    use super::*;

    /// A borrow-carrying one-lifetime family standing in for `Carried` / `Scope` / the contract:
    /// `At<'r>` is a `&'r u32`, whose lifetime the borrow checker can't track across `Erased`'s
    /// `'static` store.
    struct RefFamily;

    // SAFETY: `&'r u32` is one type generic only in `'r`; a reference's layout is lifetime-independent.
    unsafe impl Reattachable for RefFamily {
        type At<'r> = &'r u32;
    }

    #[test]
    fn erased_roundtrip_and_helpers() {
        let backing = [7u32, 8, 9];
        // Erase a live borrow to the `'static` store, then re-anchor it to a fresh lifetime the
        // `backing` array (held live to the end of the test) keeps valid.
        let erased: Erased<RefFamily> = Erased::erase(&backing[0]);
        let reattached: &u32 = unsafe { erased.reattach() };
        assert_eq!(*reattached, 7);

        // The transient helpers over the same primitive.
        let owned: &u32 = unsafe { reattach_value::<RefFamily>(&backing[1]) };
        assert_eq!(*owned, 8);
        let by_ref: &&u32 = &&backing[2];
        let viaref: &&u32 = unsafe { reattach_ref::<RefFamily>(by_ref) };
        assert_eq!(**viaref, 9);
        let elems: &[&u32] = &[&backing[0], &backing[1]];
        let viaslice: &[&u32] = unsafe { reattach_slice::<RefFamily>(elems) };
        assert_eq!(viaslice.iter().map(|r| **r).sum::<u32>(), 15);

        // Read again after the helper calls to catch a tree-borrows regression on the first borrow.
        assert_eq!(*reattached, 7);
    }
}
