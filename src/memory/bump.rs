//! The **bump tier**: storage outside the graph, for the AST and the type lattice's registry and
//! scratch. A bump here has no reach and no cell; it is released whole when its owner drops.
//!
//! The tier is a collections arena — growable vectors and a hash table over `&Bump` as an
//! [`Allocator`](allocator_api2::alloc::Allocator) — which a cell's
//! [`Writer`](super::substrate::Writer) has no verbs for, so it does not pretend to be a region.
//! Callers use bumpalo's own verbs (`alloc`, `alloc_slice_copy`, `alloc_slice_fill_iter`,
//! `alloc_str`) through [`BumpAllocator`]; there is no door layer over them.
//!
//! This is the crate's only import of `bumpalo`, `allocator_api2` and `hashbrown`.
//!
//! **Nothing with drop glue goes in.** Bumpalo runs no destructor, so a value owning heap memory
//! would leak it. [`bump_table`] asserts that for a table's entries at compile time; a slice or a
//! single value is the caller's to keep `Copy`.

use std::hash::BuildHasher;

/// The arena the tier allocates from; its owner releases it whole.
pub type Bump = bumpalo::Bump;

/// The tier's allocation capability: a shared borrow of a bump, which is both bumpalo's verb
/// receiver and the [`Allocator`](allocator_api2::alloc::Allocator) the tier's collections take.
pub type BumpAllocator<'a> = &'a Bump;

/// A growable vector whose buffer lives in a bump.
pub type BumpVec<'a, T> = allocator_api2::vec::Vec<T, BumpAllocator<'a>>;

/// A hash table whose buckets live in a bump. Built through `bump_table`.
pub type BumpBackedMap<'a, K, V, S = hashbrown::DefaultHashBuilder> =
    hashbrown::HashMap<K, V, S, BumpAllocator<'a>>;

/// Build a table over `bump`, **proving at compile time** that its entries carry no drop glue. The
/// bump runs no destructor, so a `Drop`-bearing key or value would silently leak whatever it owns;
/// the assert is monomorphization-checked, so an entry field that brings glue in is a build error
/// at the declaration that admitted it rather than a leak.
pub(crate) fn bump_table<'a, K, V, S: BuildHasher + Default>(
    bump: BumpAllocator<'a>,
) -> BumpBackedMap<'a, K, V, S> {
    const {
        assert!(
            !std::mem::needs_drop::<K>() && !std::mem::needs_drop::<V>(),
            "a bump-backed table's entries must carry no drop glue: the bump runs no destructor",
        )
    };
    hashbrown::HashMap::with_hasher_in(S::default(), bump)
}
