//! The build: the load, and the settlements a binding settles beside the run tree.
//!
//! The build arena is the scheduler's to keep: the drain resolves a frame's settlement before the
//! frame is born. This module holds what decides a settlement's identity: [`key_of`], the digest
//! a parent finds a child by; the store the load's walk keeps its records in ([`LoadStore`]); the
//! store a resumption keeps its records in over the facts it extends ([`Layer`]), with the memo of
//! what its binding wakes; and the folds over those records ([`fold`]): each callable's key and
//! contract.
//!
//! **Imports.** Outside doc comments and `#[cfg(test)]` this module names `crate::memory`,
//! `crate::scope` and `crate::values`, and nothing else in the crate; [`tests::boundary`] reads the source to hold it
//! there.
//!
//! See [build/README.md](build/README.md).

mod folds;
mod key;
mod layer;
mod load;
mod wake;

#[cfg(test)]
mod tests;

pub use folds::fold;
pub use key::key_of;
#[cfg_attr(
    not(test),
    expect(unused_imports, reason = "no settlement runs a layer yet")
)]
pub(crate) use layer::Layer;
pub use load::LoadStore;
