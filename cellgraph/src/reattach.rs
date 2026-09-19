//! The reattachable contract: a family generic over region lifetimes, beside the graph lifetime
//! `'graph` it may also borrow through; its erased form at `'graph`; and the single lifetime-retype
//! that moves those lifetimes between the two. The cell graph stores a continuation erased in a
//! slot free of every step brand and hands it back re-anchored at the step's, which is the only
//! reason this seam exists — see [../README.md](../README.md) § The contract: three embedder types.
//!
//! The contract comes in two arities. [`Reattachable`] is over one region lifetime, `'cell`, and is
//! what the storage continuation, value and delivery families carry. [`ReattachableOverBoth`] is
//! over both of a step's brands — `'here`, the executing cell's storage, and `'scratch`, its
//! scratch habitat — and is what the scratch state's family carries, so a parked form names storage
//! at `'here` and scratch at `'scratch`, each at its own brand. The two-lifetime holder,
//! [`ErasedOverBoth`], carries the crate's only two calls of the private `retype`; the one-lifetime
//! [`Erased`] is that holder seen through an adapter family whose form ignores `'scratch`, so the
//! `unsafe` argument is made once.

use std::marker::PhantomData;
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

/// A type generic over both of a step's region lifetimes — `'here`, the executing cell's storage,
/// and `'scratch`, its scratch habitat — whose representation is identical across every choice of
/// them. What a cell parks in its scratch slot is one: it names storage at `'here` and scratch at
/// `'scratch`, each at its own brand, so a `'here` reference gathered into a scratch-side run reads
/// back at `'here` and goes on into storage unchanged.
///
/// `'graph` is storage that outlives the graph, as it is for [`Reattachable`], and the retype never
/// moves it. The where-clauses are the step's own relation between its brands: `'graph: 'here`,
/// since the brand is a borrow the graph outlives, and `'here: 'scratch`, which is what lets a
/// scratch structure reference storage and nothing the other way.
///
/// # Safety
///
/// An implementor asserts that `At<'a, 'b>` and `At<'c, 'd>` are the *same type up to those two
/// lifetimes* — identical size, alignment, and validity — for every pair the where-clauses admit.
/// Every well-formed `type At<'here, 'scratch> = Foo<'graph, 'here, 'scratch>;` where `Foo` is
/// generic only in lifetimes satisfies this. Do not implement it for a family whose layout depends
/// on a lifetime.
pub unsafe trait ReattachableOverBoth<'graph> {
    /// The family's form at `'here` and `'scratch`.
    type At<'here, 'scratch>
    where
        'graph: 'here,
        'here: 'scratch;
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
/// takes the `Family<'graph> => At` arm, and implements the trait for that lifetime alone. A family
/// generic over another type takes the `Family<Inner: Bound> => At` arm, alone in its invocation:
/// the impl holds for every `Inner` meeting `Bound`, a trait over `'graph`, and the form names one
/// of that trait's region-generic associated types — `Inner::At<'cell>` — wherever it nests it.
///
/// A family of the two-lifetime contract, [`ReattachableOverBoth`], takes a `both` arm and writes
/// its form with literal `'here` and `'scratch`: `reattachable!(both Gather => Parked<'here,
/// 'scratch>)`. A generic-inner `both` arm mirrors the one-lifetime one, and the form names
/// `Inner::At<'cell>` for a one-lifetime `Inner` at whichever brand it nests it under. The inner
/// family is additionally bound `'graph`, which is what lets a form hold `Inner::At<'here>` behind
/// a `&'scratch` reference: the projection outlives `'scratch` once its own parameters do, and a
/// family marker outliving the graph is what every embedder writes anyway. Each `both` invocation
/// carries one family, so the repetition arm stays the one-lifetime contract's.
///
/// The `unsafe` obligation — that `Family`'s `At<'cell>` is one type up to `'cell`, per
/// [`Reattachable`]'s contract — is discharged **once** here, so embedder families carry no
/// open-coded `unsafe impl`. The macro cannot *check* layout-invariance, so only invoke it with
/// families that genuinely satisfy the contract. A family is named by a plain identifier. A generic
/// family's form meets the contract when every lifetime-dependent part of it is a plain lifetime or
/// an associated type of `Inner` generic in `'cell` alone: a type has one impl of a trait, and an
/// impl cannot choose a type by lifetime, so that associated type is one type up to `'cell` too.
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
///
/// struct Tagged<Inner>(PhantomData<Inner>);
/// reattachable!(Tagged<Inner: cellgraph::Reattachable<'graph>> => (u8, Inner::At<'cell>));
///
/// struct Gathered;
/// reattachable!(both Gathered => (&'here u32, &'scratch [&'here u32]));
///
/// struct Holding<Inner>(PhantomData<Inner>);
/// reattachable!(both Holding<Inner: cellgraph::Reattachable<'graph>> => &'scratch [Inner::At<'here>]);
/// ```
#[macro_export]
macro_rules! reattachable {
    (both $family:ident <$inner:ident : $bound:path> => $at:ty $(,)?) => {
        // SAFETY: see the macro docs — `$family`'s `At<'here, 'scratch>` is layout-invariant in
        // both for every `$inner`, since an impl cannot choose an associated type by lifetime.
        unsafe impl<'graph, $inner: $bound + 'graph> $crate::ReattachableOverBoth<'graph>
            for $family<$inner>
        {
            type At<'here, 'scratch> = $at where 'graph: 'here, 'here: 'scratch;
        }
    };
    (both $family:ident => $at:ty $(,)?) => {
        // SAFETY: see the macro docs — `$family`'s `At<'here, 'scratch>` is layout-invariant in
        // both lifetimes.
        unsafe impl<'graph> $crate::ReattachableOverBoth<'graph> for $family {
            type At<'here, 'scratch> = $at where 'graph: 'here, 'here: 'scratch;
        }
    };
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
    ($family:ident <$inner:ident : $bound:path> => $at:ty $(,)?) => {
        // SAFETY: see the macro docs — `$family`'s `At<'cell>` is layout-invariant in `'cell` for
        // every `$inner`, since an impl cannot choose an associated type by lifetime.
        unsafe impl<'graph, $inner: $bound> $crate::Reattachable<'graph>
            for $family<$inner>
        {
            type At<'cell> = $at where 'graph: 'cell;
        }
    };
    ($($family:ident $(<$graph:lifetime>)? => $at:ty),+ $(,)?) => {$(
        $crate::reattachable!(@family $family $(<$graph>)? => $at);
    )+};
}

/// The scratch family of a graph whose cells park nothing in their scratch habitat. Its form is
/// `()`, so the slot holds a zero-sized value and an embedder that never parks there names no
/// family of its own — the counterpart of [`NoDelivery`](crate::NoDelivery) on the delivery side.
pub struct NoScratch;

reattachable!(both NoScratch => ());

/// The single lifetime-retype primitive: move an `A` out as a `B`, where the caller guarantees `A`
/// and `B` are one type up to a lifetime. Private to this module and called from exactly two
/// places, both on [`ErasedOverBoth`], so `A` / `B` are always `T::At<_, _>` for one family — the
/// trait's layout-invariance contract is what makes the bitwise move sound. The one-lifetime
/// [`Erased`] reaches it through that same holder, so its own doors add no call site.
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

/// A two-lifetime family's value held at `'graph` in both positions, so it can rest in a slot free
/// of every step brand. One door puts a value in, another takes it back out at a caller-chosen
/// `'here` and `'scratch`, and the crate's single lifetime-retype sits between them — the only two
/// calls of it there are. A `'graph` borrow in the form is stored and handed back as it is.
///
/// Crate-private: the scratch slot holds one, and [`Erased`] is one seen through an adapter family.
pub(crate) struct ErasedOverBoth<'graph, T: ReattachableOverBoth<'graph>> {
    inner: T::At<'graph, 'graph>,
}

impl<'graph, T: ReattachableOverBoth<'graph>> ErasedOverBoth<'graph, T> {
    /// Hold a family value that is already at `'graph` in both positions. Safe, and no retype
    /// happens: the value is stored in the form it arrives in.
    pub(crate) fn store(value: T::At<'graph, 'graph>) -> Self {
        ErasedOverBoth { inner: value }
    }

    /// Hold a family value born at some shorter pair, forgetting both lifetimes for storage.
    ///
    /// The **signature is safe**: forgetting a lifetime cannot fabricate one. Nothing may be read
    /// out of the erased form without a [`reattach`](ErasedOverBoth::reattach), whose own contract
    /// is what carries the obligation that the value's referents are still alive at the lifetimes
    /// it comes back at.
    pub(crate) fn erase<'here, 'scratch>(value: T::At<'here, 'scratch>) -> Self
    where
        'graph: 'here,
        'here: 'scratch,
    {
        // SAFETY: a retype of the two region lifetimes alone for storage (the
        // `ReattachableOverBoth` layout-invariance contract); the erased value is stored, never
        // used, until a re-anchor.
        ErasedOverBoth {
            inner: unsafe { retype::<T::At<'here, 'scratch>, T::At<'graph, 'graph>>(value) },
        }
    }

    /// Re-anchor the held value at a caller-chosen `'here` and `'scratch`.
    ///
    /// # Safety
    ///
    /// The obligation is [`Erased::reattach`]'s, taken per position: each of `'here` and
    /// `'scratch` must be a lifetime the referents the form holds *at that position* outlive. A
    /// `'graph` referent outlives the graph and so every pair the where-clauses admit; a value that
    /// arrived through [`store`] holds no region referent at all. A value that arrived through
    /// [`erase`] came from some pair, and the caller must know each position's region referents
    /// outlive the lifetime it comes back at — this crate's caller knows it because the `'here`
    /// referents are the executing cell's storage and the `'scratch` ones are its write home's
    /// scratch bump, both kept for the whole step the brands belong to. A family **invariant** in
    /// either lifetime additionally requires that nothing borrowed for it is written into the
    /// re-anchored value and read back at a longer one; the condition is per lifetime, and both
    /// step brands are unnameable outside their own `enter` scope, which discharges it for each.
    ///
    /// [`store`]: ErasedOverBoth::store
    /// [`erase`]: ErasedOverBoth::erase
    pub(crate) unsafe fn reattach<'here, 'scratch>(self) -> T::At<'here, 'scratch>
    where
        'graph: 'here,
        'here: 'scratch,
    {
        // SAFETY: see the method contract; a retype of the two region lifetimes alone.
        unsafe { retype::<T::At<'graph, 'graph>, T::At<'here, 'scratch>>(self.inner) }
    }
}

/// A family whose erased form is `Copy` makes its holder `Copy` too: the erased value names bytes
/// it does not own, so duplicating the holder duplicates no ownership. This is what lets a carrier
/// be read without being consumed.
impl<'graph, T: ReattachableOverBoth<'graph>> Clone for ErasedOverBoth<'graph, T>
where
    T::At<'graph, 'graph>: Copy,
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<'graph, T: ReattachableOverBoth<'graph>> Copy for ErasedOverBoth<'graph, T> where
    T::At<'graph, 'graph>: Copy
{
}

/// A one-lifetime family seen as a two-lifetime one whose form ignores `'scratch`. Private: it
/// exists so [`Erased`] routes through [`ErasedOverBoth`] rather than repeating its two `retype`
/// calls, and no family outside this module is ever written in terms of it.
struct OneBrand<T>(PhantomData<T>);

// SAFETY: `At<'here, 'scratch>` is `T::At<'here>`, one type up to `'here` by `T`'s own
// `Reattachable` contract and naming `'scratch` nowhere.
unsafe impl<'graph, T: Reattachable<'graph>> ReattachableOverBoth<'graph> for OneBrand<T> {
    type At<'here, 'scratch>
        = T::At<'here>
    where
        'graph: 'here,
        'here: 'scratch;
}

/// A family value held at `'graph`, so it can rest in a slot free of every step brand. One door
/// puts a value in, another takes it back out at a caller-chosen `'cell`, and the crate's single
/// lifetime-retype sits between them — reached through [`ErasedOverBoth`], which owns both of its
/// call sites. Only `'cell` is retyped: a `'graph` borrow in the form is stored and handed back as
/// it is.
///
/// The type is public only so an embedder can write the `Erased<'graph, V>: Copy` bound the
/// capture doors take; nothing outside the crate constructs or opens one.
pub struct Erased<'graph, T: Reattachable<'graph>> {
    inner: ErasedOverBoth<'graph, OneBrand<T>>,
}

impl<'graph, T: Reattachable<'graph>> Erased<'graph, T> {
    /// Hold a family value that is already at `'graph`. Safe, and no retype happens: the value is
    /// stored in the form it arrives in.
    pub(crate) fn store(value: T::At<'graph>) -> Self {
        Erased {
            inner: ErasedOverBoth::store(value),
        }
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
        Erased {
            inner: ErasedOverBoth::erase::<'cell, 'cell>(value),
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
        // SAFETY: the contract above is `ErasedOverBoth::reattach`'s at one lifetime taken twice —
        // the adapter's form names `'scratch` nowhere, so its `'scratch` position holds no
        // referent and asks nothing of the caller.
        unsafe { self.inner.reattach::<'cell, 'cell>() }
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
