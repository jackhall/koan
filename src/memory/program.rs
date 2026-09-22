//! **Program storage**: the tier where program text, the raw AST and what a loaded program lays
//! down at `'graph` live — outside the graph, in two stores: a bump the parser writes the AST into,
//! and `cellgraph`'s [`Storage`], written through the same [`Writer`] a region is. Its own module
//! because the parser depends on it and on nothing else here, so `parse` names [`ProgramBrand`] and
//! never a cell.

use std::marker::PhantomData;

use super::bump::{Bump, BumpAllocator};
use super::substrate::{Storage, Writer};

/// Stand up a fresh program storage.
pub fn program_storage() -> ProgramStorage {
    ProgramStorage {
        bump: Bump::new(),
        store: Storage::new(),
    }
}

/// The owner of the stores an AST and a loaded program's tables borrow. It never enters the graph:
/// both stores are private and [`brand`](ProgramStorage::brand) is the only capability the type
/// exposes, and [`program_storage`] is its only constructor.
pub struct ProgramStorage {
    bump: Bump,
    store: Storage,
}

impl ProgramStorage {
    /// Mint this storage's [`ProgramBrand`] — the allocation capability the parse entry points take.
    pub fn brand(&self) -> ProgramBrand<'_> {
        ProgramBrand(&self.bump, self.store.writer(), PhantomData)
    }
}

/// A [`BumpAllocator`] carrying the proof that its bump is [`ProgramStorage`]'s. The parse entry
/// points take this rather than a bare allocator, so a parsed AST's storage tier is checked at
/// every call site rather than held by the discipline of one. The channel it keys admits only a
/// `parse`'s `ProgramExpression`, which this brand alone mints.
///
/// Its lifetime is `'graph`: program storage outlives every cell graph that runs the program, so
/// what the AST lends a value is borrowed at the graph's lifetime and never at a cell's.
///
/// The distinction needs a type because `KExpression` is covariant: a node borrowing a shorter-lived
/// bump coerces to any shorter lifetime, so the borrow checker sees nothing to object to. Widening
/// through [`ProgramBrand::allocator`] is free; the reverse does not exist.
///
/// The brand is **invariant** in `'graph`, so a held brand never shortens either. A door call
/// therefore pins its `parts` at the storage's own lifetime rather than at whatever shorter lifetime the
/// caller happens to run at — which is what carries the storage-tier obligation in the parameter
/// types instead of in prose. The doors' *products* stay covariant, so program-hosted AST still
/// reaches its readers by ordinary subtyping.
#[derive(Clone, Copy)]
pub struct ProgramBrand<'graph>(
    BumpAllocator<'graph>,
    Writer<'graph>,
    PhantomData<fn(&'graph ()) -> &'graph ()>,
);

impl<'graph> ProgramBrand<'graph> {
    /// The plain allocation capability underneath — for the parser's own allocations, which need
    /// no more than a bump to allocate into.
    pub fn allocator(self) -> BumpAllocator<'graph> {
        self.0
    }

    /// A writer into the store `cellgraph` owns, at `'graph` — where a loaded program lays down
    /// what every cell names at no price: the builtin table and the program's record.
    pub fn writer(self) -> Writer<'graph> {
        self.1
    }
}
