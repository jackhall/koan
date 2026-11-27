//! The build: the load, and the facts it folds beside the run tree.
//!
//! This module holds the store the load's walk keeps its records in ([`LoadStore`]) and the folds
//! over those records ([`fold`]): each body's caching class, contract, and the names its entry and
//! statement seams bind.
//!
//! It also folds the load's records into the text of the program's binding graph ([`lay`]), and
//! keeps the facts the graph holds beside it.
//!
//! **Imports.** Outside doc comments and `#[cfg(test)]` this module names `crate::memory`,
//! `crate::scope`, `crate::type_lattice` and `crate::values`, and `crate::source::SourceRef`, where
//! a call is written, and nothing else in the crate; [`tests::boundary`] reads the source to hold
//! it there.
//!
//! See [build/README.md](build/README.md).

mod folds;
pub mod lay;
mod load;

#[cfg(test)]
mod tests;

pub use folds::fold;
pub use load::LoadStore;
#[cfg(test)]
pub(crate) use tests::{typed_in_all, typed_over};
