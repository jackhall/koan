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
//! on is its receipt run. A frame keyed on its bindings is born borrowing its specialization, a
//! build cell the drain finds or builds before the birth ([`Seek`]), so it never waits on one.
//!
//! **Imports.** Outside doc comments and `#[cfg(test)]` this module names `crate::knot`,
//! `crate::memory`, `crate::scope`, `crate::type_lattice` and `crate::values`, and nothing else in
//! the crate; [`tests::boundary`] reads the source to hold it there. It names `scope` for the
//! header a graph keeps per habitat half ([`RegionHeader`](crate::scope::RegionHeader)), whose
//! table and home the drain keeps the books of. Neither `values`, `knot` nor `scope` names this
//! module, and `cellgraph` names neither it nor koan.
//!
//! See [scheduler/README.md](scheduler/README.md).

mod action;
mod continuation;
mod delivery;
mod drain;
mod load;
mod seek;

#[cfg(test)]
mod tests;

pub use action::{
    Action, Hold, Holding, Placement, Received, Request, Slot, Step, StepError, Taken, Use,
};
pub use continuation::{BirthAt, Born, NativeStep, StateAt, StepBundle, Work};
#[cfg(test)]
pub(crate) use drain::{
    CACHING, EVICT_BEFORE_STEP, LAID, TIERS, Tiers, WATCH_LOG, WATCHED, Watched,
};
pub use drain::{DrainStalled, Graph, Resting, Scheduler};
pub use load::Roots;
pub use seek::Seek;
