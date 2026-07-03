//! Shared fixture for the witnessed module's `compile_fail` soundness guards: the
//! carrier families and the local region-owning witness every guard exercises, so a
//! signature change to `Witness` / `WitnessRegion` / `Reattachable` lands here once.
//! Hidden from docs and `pub` only because doctests compile as external crates and
//! must import it; it is not part of the module's real surface.

use std::cell::Cell;

use super::{MergeWitness, Reattachable, SealedExtern, Witness, WitnessRegion};

/// A shared-reference carrier family: `&'r u32`.
pub struct RefFamily;
// SAFETY: `&'r u32` is one type generic only in `'r`.
unsafe impl Reattachable for RefFamily {
    type At<'r> = &'r u32;
}

/// An invariant carrier family: `Cell<&'r u32>`.
pub struct InvFamily;
// SAFETY: `Cell<&'r u32>` is one type generic only in `'r`.
unsafe impl Reattachable for InvFamily {
    type At<'r> = Cell<&'r u32>;
}

/// A local witness owning its region — the `Vec`'s heap buffer stays at a fixed
/// address across the witness's move, so a value built from `region()` stays pinned.
pub struct Cart(pub Vec<u32>);
// SAFETY: the owned `Vec`'s buffer is fixed-address for the `Cart`'s whole life.
unsafe impl Witness for Cart {}
// SAFETY: `region` borrows the buffer the `Witness` impl above pins.
unsafe impl WitnessRegion for Cart {
    type Region = [u32];
    fn region(&self) -> &[u32] {
        &self.0
    }
}
// SAFETY: `None` is the always-safe verdict — a single-region `Cart` cannot represent
// a value pinning two unrelated regions. The merge guard only needs this impl to
// type-check; guard code never runs.
unsafe impl MergeWitness for Cart {
    fn merge(_left: &Self, _right: &Self) -> Option<Self> {
        None
    }
}

/// Build a [`SealedExtern`] from a live carrier. `SealedExtern`'s constructors are all
/// crate-private (no production caller builds one from an arbitrary borrow), but a doctest
/// compiles as an external crate, so the `SealedExtern::open` guard and its compiling twin need
/// this in-crate wrapper to construct one at all.
pub fn seal_extern<T: Reattachable>(live: T::At<'_>) -> SealedExtern<T> {
    SealedExtern::erase(live)
}
