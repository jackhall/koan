//! The reattachable contract: a family generic over exactly one lifetime, its erased `'static`
//! storage form, and the single lifetime-retype that moves a value between the two. The cell
//! graph stores a continuation erased in a lifetime-free slot and hands it back re-anchored at
//! the step's brand, which is the only reason this seam exists — see
//! [../README.md](../README.md) § The contract: two embedder types.

use std::mem::ManuallyDrop;

/// A type generic over exactly one lifetime whose representation is identical across every choice
/// of that lifetime — a lifetime parameter never changes layout. Implementing it lets the family
/// route the single audited lifetime-retype below.
///
/// # Safety
///
/// An implementor asserts that `At<'x>` and `At<'y>` are the *same type up to the lifetime
/// parameter* — identical size, alignment, and validity — for all `'x`, `'y`. Every well-formed
/// `type At<'cell> = Foo<'cell>;` where `Foo` is generic only in that lifetime satisfies this. Do not
/// implement it for a family whose layout depends on the lifetime.
pub unsafe trait Reattachable {
    /// The family's form at `'cell`. Bounded `: 'cell` because a single-lifetime family's value borrows
    /// only through `'cell`, which is what lets a family's form be held behind a `&'cell`.
    type At<'cell>: 'cell;
}

/// A family whose live form runs no destructor, so it may rest in a cell's region.
///
/// A bump releases its chunks whole and never walks a value, so anything written into a region
/// must have no drop glue — a `String` stored there would leak its heap buffer. The marker is what
/// the value doors take as their bound; the alloc site additionally asserts
/// `!needs_drop::<T::At<'static>>()`, so a wrong `impl` is a compile error at the door rather than
/// a silent leak. A continuation family carries no such bound: a continuation rests in the cell's
/// own slot, not in a region, and its glue runs when the slot reclaims.
pub trait DropFree {}

/// Generate `unsafe impl Reattachable` for layout-invariant families. Each `Family => At<'cell>` pair
/// expands to the trait impl; write the associated-type body with a literal `'cell`
/// (`Continuation => Step<'cell>`, `Owned => String`).
///
/// The `unsafe` obligation — that `Family`'s `At<'cell>` is one type up to the lifetime `'cell`, per
/// [`Reattachable`]'s contract — is discharged **once** here, so embedder families carry no
/// open-coded `unsafe impl`. The macro cannot *check* layout-invariance, so only invoke it with
/// families that genuinely satisfy the contract.
///
/// ```
/// use cellgraph::reattachable;
/// struct Borrowed;
/// reattachable!(Borrowed => &'cell u32);
/// ```
#[macro_export]
macro_rules! reattachable {
    ($($family:ty => $at:ty),+ $(,)?) => {$(
        // SAFETY: see the macro docs — `$family`'s `At<'cell>` is layout-invariant in `'cell`.
        unsafe impl $crate::Reattachable for $family {
            type At<'cell> = $at;
        }
    )+};
}

/// The single lifetime-retype primitive: move an `A` out as a `B`, where the caller guarantees `A`
/// and `B` are one type up to a lifetime. Private to this module and reached only through the
/// `Reattachable`-bounded wrappers, so `A` / `B` are always `T::At<_>` for one family — the trait's
/// layout-invariance contract is what makes the bitwise move sound.
///
/// `transmute` can't prove `size_of::<T::At<'a>>() == size_of::<T::At<'step>>()` for an opaque
/// associated-type projection, so this goes through `transmute_copy` (which assumes the size
/// equality the contract guarantees) behind a `ManuallyDrop` so the source is not dropped after
/// the move. `const` asserts restore the size check `transmute` would emit and add the alignment
/// one it would not, so a family whose layout does vary with its lifetime fails to compile at the
/// retype rather than reading misaligned.
///
/// # Safety
///
/// `A` and `B` must be one type up to a lifetime (the `Reattachable` contract), so they share
/// layout and the source bytes are a valid `B`.
unsafe fn retype<A, B>(value: A) -> B {
    const { assert!(size_of::<A>() == size_of::<B>()) };
    const { assert!(align_of::<A>() == align_of::<B>()) };
    let value = ManuallyDrop::new(value);
    // SAFETY: by the caller's contract `A` and `B` share layout (size asserted above);
    // `ManuallyDrop` keeps the source from being dropped after the bitwise move out.
    unsafe { std::mem::transmute_copy::<A, B>(&value) }
}

/// A one-lifetime family value held in its `'static` form, so it can rest in a lifetime-free slot.
/// One door puts a value in, another takes it back out at a caller-chosen `'cell`, and the crate's
/// single lifetime-retype sits between them — nothing else names it.
///
/// The type is public only so an embedder can write the `Erased<V>: Copy` bound the capture doors
/// take; nothing outside the crate constructs or opens one.
pub struct Erased<T: Reattachable> {
    inner: T::At<'static>,
}

impl<T: Reattachable> Erased<T> {
    /// Hold a family value that is already at `'static`. Safe, and no retype happens: the value
    /// is stored in the form it arrives in.
    pub(crate) fn store(value: T::At<'static>) -> Self {
        Erased { inner: value }
    }

    /// Hold a family value born at some shorter `'cell`, forgetting that lifetime for storage.
    ///
    /// The **signature is safe**: forgetting a lifetime cannot fabricate one. Nothing may be read
    /// out of the erased form without a [`reattach`](Erased::reattach), whose own contract is what
    /// carries the obligation that the value's referents are still alive at the lifetime it comes
    /// back at.
    pub(crate) fn erase(value: T::At<'_>) -> Self {
        // SAFETY: lifetime-only retype for storage of a single-lifetime family (the `Reattachable`
        // layout-invariance contract); the erased value is stored, never used, until a re-anchor.
        Erased {
            inner: unsafe { retype::<T::At<'_>, T::At<'static>>(value) },
        }
    }

    /// Re-anchor the held value at a caller-chosen `'cell`.
    ///
    /// # Safety
    ///
    /// `'cell` must be a lifetime the value's referents outlive. A value that arrived through
    /// [`store`] is at `'static`, so any `'cell` satisfies that; a value that arrived through
    /// [`erase`] came from some `'x`, and the caller must know `'x: 'cell` — this crate's callers know
    /// it because the referents are region storage the graph keeps alive for the whole step the
    /// `'cell` brand belongs to. A family that is **invariant** in its lifetime (`Cell<&'cell u32>`, say)
    /// additionally requires that nothing borrowed for `'cell` is written into the re-anchored value
    /// and then read back at a longer lifetime; the step brand this crate reattaches at is
    /// unnameable outside its own `enter` scope, which is what discharges that second condition.
    ///
    /// [`store`]: Erased::store
    /// [`erase`]: Erased::erase
    pub(crate) unsafe fn reattach<'cell>(self) -> T::At<'cell> {
        // SAFETY: see the method contract; lifetime-only retype of a single-lifetime family.
        unsafe { retype::<T::At<'static>, T::At<'cell>>(self.inner) }
    }
}

/// A family whose erased form is `Copy` makes its holder `Copy` too: the erased value names bytes
/// it does not own, so duplicating the holder duplicates no ownership. This is what lets a carrier
/// be read without being consumed.
impl<T: Reattachable> Clone for Erased<T>
where
    T::At<'static>: Copy,
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Reattachable> Copy for Erased<T> where T::At<'static>: Copy {}
