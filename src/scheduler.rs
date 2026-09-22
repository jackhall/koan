//! The deferred-work drain koan runs on, built directly over `cellgraph`'s cells and liveness
//! matrix.
//!
//! A unit of work **is** a cell: its region, its erased continuation, its receipt run and its holds
//! are the cell's, and this module adds only the ready stack, the drain protocol and delivery.
//! Liveness is the matrix's — no reference count, no pin bundle and no antichain fold lives here,
//! and a cell is reclaimed the instant no hold names it. The layers above reach it as one
//! [`StepBundle`], and the scheduler names no state and no step of its own.
//!
//! A step cannot create or release a cell, and names no handle and no carrier door: it hands the
//! drain an [`Action`] naming nothing but `'graph`, and the drain performs every birth and every
//! death. A request has no cell until the drain pops it, so the only thing a live cell ever waits
//! on is its receipt run.
//!
//! **Imports.** Outside doc comments and `#[cfg(test)]` this module names `crate::knot`,
//! `crate::memory` and `crate::values`, and nothing else in the crate; [`tests::boundary`] reads
//! the source to hold it there. Neither `values` nor `knot` names this module, and `cellgraph`
//! names neither it nor koan.
//!
//! See [scheduler/README.md](scheduler/README.md).

mod action;
mod continuation;
mod delivery;
mod drain;

#[cfg(test)]
mod tests;

pub use action::{
    Action, Hold, Holding, Placement, Received, Request, Slot, Step, StepError, Taken, Use,
};
pub use continuation::{NativeStep, StepBundle, Work};
pub use drain::{DrainStalled, Graph, Scheduler};
