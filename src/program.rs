//! A loaded program as one owning value.
//!
//! [`CellSubstrate`] owns program storage and the symbol interner, and beside them the state that
//! borrows them at `'graph`: the cell graph, the type registry, and the parsed program. The
//! self-reference is `self_cell`'s; koan writes no `unsafe` for it. Nothing outside names
//! `'graph`: the running state is reached through [`CellSubstrate::with`], whose closure is
//! quantified over a fresh lifetime, and the scheduler is a view made per call over the graph.
//!
//! **Imports.** Outside `#[cfg(test)]` this module names `crate::memory`, `crate::parse`,
//! `crate::scheduler`, `crate::symbols` and `crate::type_lattice`; [`tests::boundary`] reads the
//! source to hold it there.

mod substrate;

#[cfg(test)]
mod tests;

pub use substrate::{CellSubstrate, Running};
