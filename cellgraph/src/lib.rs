//! A cell substrate: a capped slab of cells, each owning a bump region, over a liveness matrix
//! that records which cell holds a value homed in which other. The crate depends on nothing —
//! not on koan, not on `workgraph` — so a cell carries no embedder vocabulary; a continuation
//! family enters as a type parameter and is stored erased.
//!
//! The cell model — the slot-plus-generation handle, the `create` / `enter` / `release` verbs,
//! the parent birth relation — is [design/cellgraph.md](../design/cellgraph.md). The matrix that
//! decides when a cell may be reclaimed, the mint OR that is the only write into it, and the
//! sealed tier a still-held cell falls into on release, are
//! [design/liveness-matrix.md](../design/liveness-matrix.md).
//!
//! The public surface is what `tests/surface.rs` exercises.

#![deny(unsafe_op_in_unsafe_fn)]

mod carrier;
mod handle;
mod mask;
mod matrix;
mod reattach;
mod region;
mod resident;
mod scratch;
mod sealed;
mod table;
mod tree;

pub use carrier::{Active, Dormant};
pub use handle::{CellHandle, Handle, Stale, TreeHandle};
pub use reattach::{DropFree, Erased, Reattachable};
pub use region::Writer;
pub use resident::Resident;
pub use table::{
    CellTable, CreateError, CrossedOperand, EnterError, Operand, Prices, RedeemError,
    ReleaseAbsorption, ReleaseError, ReleaseTreeError, StepContext, Verdict,
};
