//! The build: the load, and the settlements a binding settles beside the run tree.
//!
//! The build depot is the scheduler's to keep: the drain resolves a frame's settlement before the
//! frame is born. This module holds what decides a settlement's identity: [`key_of_ask`], the
//! digest a parent finds a child by, a function of what the callee's frame binds; the store the
//! load's walk keeps its records in ([`LoadStore`]); the store a resumption keeps its records in
//! over the facts it extends ([`LayerStore`]), with the memo of what its binding wakes; and the
//! folds over those records ([`fold`]): each callable's key and contract, and the [`trace`] of the
//! bindings each call site of a keyed body passes.
//!
//! **Imports.** Outside doc comments and `#[cfg(test)]` this module names `crate::memory`,
//! `crate::scope` and `crate::values`, and `crate::source::SourceRef`, where a traced call is
//! written, and nothing else in the crate; [`tests::boundary`] reads the source to hold it there.
//!
//! See [build/README.md](build/README.md).

mod folds;
mod key;
mod layer;
mod load;
pub mod trace;
mod wake;

#[cfg(test)]
mod tests;

pub use folds::fold;
pub use key::{key_of, key_of_ask};
pub(crate) use layer::LayerStore;
pub use load::LoadStore;
