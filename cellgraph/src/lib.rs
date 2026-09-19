//! A cell substrate: a capped slab of cells, each owning a bump region, over a liveness matrix
//! that records which cell holds a value homed in which other. The crate depends on nothing —
//! not on koan, not on `workgraph` — so a cell carries no embedder vocabulary; a continuation
//! family enters as a type parameter and is stored erased.
//!
//! The cell model — the slot-plus-generation handle, the `create` / `enter` / `release` verbs,
//! the three region habitats, the tenant that owns none — is [../README.md](../README.md). The matrix that
//! decides when a cell may be reclaimed, the mint OR that is the only write into it, and the
//! sealed tier a still-held cell falls into on release, are
//! [graph/README.md](graph/README.md).
//!
//! The public surface is what `tests/surface.rs` exercises.

#![deny(unsafe_op_in_unsafe_fn)]

mod carrier;
mod dormant;
mod graph;
mod handle;
mod matrix;
mod reach;
mod reattach;
mod receipt;
mod region;
mod scratch;
mod sealed;
mod slots;
mod tenant;
mod tree;

pub use carrier::{Active, Ready};
pub use dormant::Dormant;
pub use graph::{
    CellGraph, Config, CreateError, CrossedOperand, DeliverError, EnterError, Operand, Prices,
    Receipt, ReceiptError, RedeemError, RegisterError, ReleaseAbsorption, ReleaseError,
    ReleaseTenantError, ReleaseTreeError, StepContext, Verdict,
};
pub use handle::{CellHandle, SlabHandle, Stale, TenantHandle, TreeHandle};
pub use reattach::{DropFree, Erased, NoScratch, Reattachable, ReattachableOverBoth};
pub use receipt::{Delivered, Delivery, NoDelivery};
pub use region::{Prose, Run, ThinRun, Writer};
