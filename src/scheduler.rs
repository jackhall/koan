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
//! on is its receipt run. A frame keyed on its bindings is born under its settlement, an arena
//! node the drain finds or builds before the birth ([`Seek`]), so it never waits on one.
//!
//! **Imports.** Outside doc comments and `#[cfg(test)]` this module names `crate::knot`,
//! `crate::memory`, `crate::scope`, `crate::type_lattice` and `crate::values`, and nothing else in
//! the crate; [`tests::boundary`] reads the source to hold it there. It names `scope` for the
//! header a graph keeps per habitat half ([`RegionHeader`](crate::scope::RegionHeader)). Neither `values`, `knot` nor `scope` names this
//! module, and `cellgraph` names neither it nor koan.
//!
//! See [scheduler/README.md](scheduler/README.md).

mod action;
mod arena;
mod continuation;
mod delivery;
mod drain;
mod kept;
mod load;
mod seek;

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use action::PASSED;
pub use action::{
    Action, Hold, Holding, MemoHome, Placement, Received, Request, Slot, Step, StepError, Taken,
    Use,
};
pub use continuation::{BirthAt, Born, NativeStep, StateAt, StepBundle, Work};
#[cfg(test)]
pub(crate) use drain::{CACHING, HOPS, HopSample, LAID, REGION_WRITTEN, SKELETONS, TIERS, Tiers};
pub use drain::{DrainStalled, Graph, Resting, Scheduler};
pub use kept::{Held, Kept, KeptPart};
pub use seek::{Ask, Asked, Replayed, Requested, Seek, Settles, Sought};
