//! The build tree: the build root the load runs in and the specializations a binding settles,
//! drained beside the run tree.
//!
//! The cells themselves are the scheduler's to create and step: the load door enters the build
//! root, and the drain resolves a frame's specialization before the frame is born. This module
//! holds what decides a specialization's identity: [`key_of`], the digest a parent's table finds a
//! child by; the seams the load's frontiers decide ([`list_seams`]); and the names each callable's
//! key reads, laid from where its facts' seams fall ([`lay_keys`]); and the store the load's walk
//! keeps its records in ([`LoadStore`]).
//!
//! **Imports.** Outside doc comments and `#[cfg(test)]` this module names `crate::memory`,
//! `crate::scope` and `crate::values`, and nothing else in the crate; [`tests::boundary`] reads the source to hold it
//! there.
//!
//! See [build/README.md](build/README.md).

mod key;
mod load;
mod seams;

#[cfg(test)]
mod tests;

pub use key::{key_of, lay_keys};
pub use load::LoadStore;
pub use seams::list_seams;
