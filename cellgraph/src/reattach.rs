//! The reattachable contract: a family generic over one region lifetime, `'cell`, beside the graph
//! lifetime `'graph` it may also borrow through; its erased form at `'graph`; and the single
//! lifetime-retype that moves `'cell` between the two. The cell graph stores a continuation erased
//! in a slot free of every step brand and hands it back re-anchored at the step's, which is the only
//! reason this seam exists — see [../README.md](../README.md) § The contract: two embedder types.

use std::mem::ManuallyDrop;

/// A type generic over one region lifetime, `'cell`, whose representation is identical across every
/// choice of it — a lifetime parameter never changes layout. Implementing it lets the family route
/// the single audited lifetime-retype below.
///
/// `'graph` is storage that outlives the graph: a family's form may borrow through it beside
/// `'cell` — `&'cell Entry<'graph, 'cell>` — and the retype never moves it. The where-clause is
/// what a form nesting a `'graph` borrow under a `'cell` one needs to be well-formed, and it holds
/// at every lifetime the graph hands a value back at, since each of those is a borrow the graph
/// outlives.
///
/// # Safety
///
/// An implementor asserts that `At<'x>` and `At<'y>` are the *same type up to `'cell`* — identical
/// size, alignment, and validity — for all `'x`, `'y` that `'graph` outlives. Every well-formed
/// `type At<'cell> = Foo<'graph, 'cell>;` where `Foo` is generic only in lifetimes satisfies this.
/// Do not implement it for a family whose layout depends on a lifetime.
pub unsafe trait Reattachable<'graph> {
    /// The family's form at `'cell`.
    type At<'cell>
    where
        'graph: 'cell;
}

/// A family whose live form runs no destructor, so it may rest in a cell's region.
///
/// A bump releases its chunks whole and never walks a value, so anything written into a region
/// must have no drop glue — a `String` stored there would leak its heap buffer. The marker is what
/// the value doors take as their bound; the alloc site additionally asserts
/// `!needs_drop::<T::At<'graph>>()`, so a wrong `impl` is a compile error at the door rather than
/// a silent leak. A continuation family carries no such bound: a continuation rests in the cell's
/// own slot, not in a region, and its glue runs when the slot reclaims.
pub trait DropFree {}

/// Generate `unsafe impl Reattachable` for layout-invariant families. Each `Family => At` pair
/// expands to the trait impl for every `'graph`; write the form with a literal `'cell`, and name
/// `'graph` where it borrows storage outside the graph (`Continuation => Step<'cell>`,
/// `Owned => String`, `Program => &'graph str`). A family that names the graph lifetime itself
/// takes the `Family<'graph> => At` arm, and implements the trait for that lifetime alone.
///
/// The `unsafe` obligation — that `Family`'s `At<'cell>` is one type up to `'cell`, per
/// [`Reattachable`]'s contract — is discharged **once** here, so embedder families carry no
/// open-coded `unsafe impl`. The macro cannot *check* layout-invariance, so only invoke it with
/// families that genuinely satisfy the contract. A family is named by a plain identifier.
///
/// ```
/// use std::marker::PhantomData;
///
/// use cellgraph::reattachable;
/// struct Borrowed;
/// struct Program;
/// struct Script<'graph>(PhantomData<&'graph str>);
/// reattachable!(
///     Borrowed => &'cell u32,
///     Program => &'graph str,
///     Script<'graph> => &'graph str,
/// );
/// ```
#[macro_export]
macro_rules! reattachable {
    (@family $family:ident <$graph:lifetime> => $at:ty) => {
        // SAFETY: see the macro docs — `$family`'s `At<'cell>` is layout-invariant in `'cell`.
        unsafe impl<$graph> $crate::Reattachable<$graph> for $family<$graph> {
            type At<'cell> = $at where $graph: 'cell;
        }
    };
    (@family $family:ident => $at:ty) => {
        // SAFETY: see the macro docs — `$family`'s `At<'cell>` is layout-invariant in `'cell`.
        unsafe impl<'graph> $crate::Reattachable<'graph> for $family {
            type At<'cell> = $at where 'graph: 'cell;
        }
    };
    ($($family:ident $(<$graph:lifetime>)? => $at:ty),+ $(,)?) => {$(
        $crate::reattachable!(@family $family $(<$graph>)? => $at);
    )+};
}

/// The single lifetime-retype primitive: move an `A` out as a `B`, where the caller guarantees `A`
/// and `B` are one type up to a lifetime. Private to this module and reached only through the
/// `Reattachable`-bounded wrappers, so `A` / `B` are always `T::At<_>` for one family — the trait's
/// layout-invariance contract is what makes the bitwise move sound.
///
/// `transmute` can't prove `size_of::<T::At<'x>>() == size_of::<T::At<'y>>()` for an opaque
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

/// A family value held at `'graph`, so it can rest in a slot free of every step brand. One door
/// puts a value in, another takes it back out at a caller-chosen `'cell`, and the crate's single
/// lifetime-retype sits between them — nothing else names it. Only `'cell` is retyped: a `'graph`
/// borrow in the form is stored and handed back as it is.
///
/// The type is public only so an embedder can write the `Erased<'graph, V>: Copy` bound the
/// capture doors take; nothing outside the crate constructs or opens one.
pub struct Erased<'graph, T: Reattachable<'graph>> {
    inner: T::At<'graph>,
}

impl<'graph, T: Reattachable<'graph>> Erased<'graph, T> {
    /// Hold a family value that is already at `'graph`. Safe, and no retype happens: the value is
    /// stored in the form it arrives in.
    pub(crate) fn store(value: T::At<'graph>) -> Self {
        Erased { inner: value }
    }

    /// Hold a family value born at some shorter `'cell`, forgetting that lifetime for storage.
    ///
    /// The **signature is safe**: forgetting a lifetime cannot fabricate one. Nothing may be read
    /// out of the erased form without a [`reattach`](Erased::reattach), whose own contract is what
    /// carries the obligation that the value's referents are still alive at the lifetime it comes
    /// back at.
    pub(crate) fn erase<'cell>(value: T::At<'cell>) -> Self
    where
        'graph: 'cell,
    {
        // SAFETY: a retype of `'cell` alone for storage (the `Reattachable` layout-invariance
        // contract); the erased value is stored, never used, until a re-anchor.
        Erased {
            inner: unsafe { retype::<T::At<'cell>, T::At<'graph>>(value) },
        }
    }

    /// Re-anchor the held value at a caller-chosen `'cell`.
    ///
    /// # Safety
    ///
    /// `'cell` must be a lifetime the value's *region* referents outlive. Its `'graph` referents
    /// outlive the graph, and so every `'cell` the where-clause admits. A value that arrived through
    /// [`store`] holds no region referent, so any such `'cell` satisfies that; a value that arrived
    /// through [`erase`] came from some `'x`, and the caller must know its region referents outlive
    /// `'cell` — this crate's callers know it because the referents are region storage the graph
    /// keeps alive for the whole step the `'cell` brand belongs to. A family that is **invariant**
    /// in `'cell` (`Cell<&'cell u32>`, say) additionally requires that nothing borrowed for `'cell`
    /// is written into the re-anchored value and then read back at a longer lifetime; the step
    /// brand this crate reattaches at is unnameable outside its own `enter` scope, which is what
    /// discharges that second condition.
    ///
    /// [`store`]: Erased::store
    /// [`erase`]: Erased::erase
    pub(crate) unsafe fn reattach<'cell>(self) -> T::At<'cell>
    where
        'graph: 'cell,
    {
        // SAFETY: see the method contract; a retype of `'cell` alone.
        unsafe { retype::<T::At<'graph>, T::At<'cell>>(self.inner) }
    }
}

/// A family whose erased form is `Copy` makes its holder `Copy` too: the erased value names bytes
/// it does not own, so duplicating the holder duplicates no ownership. This is what lets a carrier
/// be read without being consumed.
impl<'graph, T: Reattachable<'graph>> Clone for Erased<'graph, T>
where
    T::At<'graph>: Copy,
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<'graph, T: Reattachable<'graph>> Copy for Erased<'graph, T> where T::At<'graph>: Copy {}
