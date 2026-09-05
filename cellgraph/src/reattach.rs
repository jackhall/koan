//! The reattachable contract: a family generic over exactly one lifetime, its erased `'static`
//! storage form, and the single lifetime-retype that moves a value between the two. The cell
//! table stores a continuation erased in a lifetime-free slot and hands it back re-anchored at
//! the step's brand, which is the only reason this seam exists — see
//! [design/cellgraph.md](../design/cellgraph.md) § The contract: two embedder types.

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
    /// The family's form at `'r`. Bounded `: 'r` because a single-lifetime family's value borrows
    /// only through `'r`, which is what lets a family's form be held behind a `&'r`.
    type At<'r>: 'r;
}

/// Generate `unsafe impl Reattachable` for layout-invariant families. Each `Family => At<'r>` pair
/// expands to the trait impl; write the associated-type body with a literal `'r`
/// (`Continuation => Step<'r>`, `Owned => String`).
///
/// The `unsafe` obligation — that `Family`'s `At<'r>` is one type up to the lifetime `'r`, per
/// [`Reattachable`]'s contract — is discharged **once** here, so embedder families carry no
/// open-coded `unsafe impl`. The macro cannot *check* layout-invariance, so only invoke it with
/// families that genuinely satisfy the contract.
///
/// ```
/// use cellgraph::reattachable;
/// struct Borrowed;
/// reattachable!(Borrowed => &'r u32);
/// ```
#[macro_export]
macro_rules! reattachable {
    ($($family:ty => $at:ty),+ $(,)?) => {$(
        // SAFETY: see the macro docs — `$family`'s `At<'r>` is layout-invariant in `'r`.
        unsafe impl $crate::reattach::Reattachable for $family {
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
/// `transmute` can't prove `size_of::<T::At<'a>>() == size_of::<T::At<'b>>()` for an opaque
/// associated-type projection, so this goes through `transmute_copy` (which assumes the size
/// equality the contract guarantees) behind a `ManuallyDrop` so the source is not dropped after
/// the move. A `const` assert restores the size check `transmute` would emit.
///
/// # Safety
///
/// `A` and `B` must be one type up to a lifetime (the `Reattachable` contract), so they share
/// layout and the source bytes are a valid `B`.
unsafe fn retype<A, B>(value: A) -> B {
    const { assert!(size_of::<A>() == size_of::<B>()) };
    let value = ManuallyDrop::new(value);
    // SAFETY: by the caller's contract `A` and `B` share layout (size asserted above);
    // `ManuallyDrop` keeps the source from being dropped after the bitwise move out.
    unsafe { std::mem::transmute_copy::<A, B>(&value) }
}

/// A one-lifetime family value held in its `'static` form, so it can rest in a lifetime-free slot.
/// [`Erased::store`] puts one in; [`Erased::reattach`] takes it back out at a caller-chosen `'r`.
/// The single home for the retype in this crate: nothing else names [`retype`].
pub struct Erased<T: Reattachable> {
    inner: T::At<'static>,
}

impl<T: Reattachable> Erased<T> {
    /// Hold a family value that is already at `'static`. Safe, and no retype happens: the value
    /// is stored in the form it arrives in.
    pub fn store(value: T::At<'static>) -> Self {
        Erased { inner: value }
    }

    /// Re-anchor the held value at a caller-chosen `'r`.
    ///
    /// # Safety
    ///
    /// `'r` must be a lifetime the value's referents outlive. Every value reaching [`store`] is
    /// already at `'static`, so any `'r` satisfies that for a **covariant** family; a family that
    /// is invariant in its lifetime (`Cell<&'r u32>`, say) additionally requires that nothing
    /// borrowed for `'r` is written into the re-anchored value and then read back at `'static`.
    /// The step brand `'b` this crate reattaches at is unnameable outside its own `enter` scope,
    /// which is what discharges that second condition.
    ///
    /// [`store`]: Erased::store
    pub unsafe fn reattach<'r>(self) -> T::At<'r> {
        // SAFETY: see the method contract; lifetime-only retype of a single-lifetime family.
        unsafe { retype::<T::At<'static>, T::At<'r>>(self.inner) }
    }
}
