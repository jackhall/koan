//! The deferred-work drain koan runs on, built directly over `cellgraph`'s cells and liveness
//! matrix.
//!
//! A unit of work **is** a cell: its region, its erased continuation and its holds are the cell's,
//! and this module adds only the submission table, the work queue, the drain protocol and delivery.
//! Liveness is the matrix's — no reference count, no pin bundle and no antichain fold lives here,
//! and a cell is reclaimed the instant no hold names it.
//!
//! A step cannot create or release a cell: it hands the drain an [`Action`] naming nothing but
//! `'graph`, and the drain performs every birth and every death. A unit whose dependencies are
//! unmet has no cell at all — the drain holds it as a submission with a count — so the only thing a
//! live cell ever waits on is its receipt run.
//!
//! **Imports.** Outside doc comments and `#[cfg(test)]` this module names `crate::function`,
//! `crate::memory` and `crate::values`, and nothing else in the crate; [`tests::boundary`] reads
//! the source to hold it there. Neither `values` nor `function` names this module, and `cellgraph`
//! names neither it nor koan.
//!
//! See [scheduler/README.md](scheduler/README.md).

mod action;
mod continuation;
mod delivery;
mod drain;

#[cfg(test)]
mod tests;

pub use action::{Action, Hop, Placement, Request, Spawns, StepError};
pub use continuation::{
    CellPlace, Context, Continuation, ContinuationFamily, Destination, NativeStep, Provenance,
    Resume, ScratchFamily, ScratchState, State,
};
pub use delivery::KDelivery;
pub use drain::{DrainStalled, Scheduler};
