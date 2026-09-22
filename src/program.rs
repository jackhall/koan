//! A loaded program as one owning value.
//!
//! [`CellSubstrate`] owns program storage and the symbol interner, and beside them the state that
//! borrows them at `'graph`: the cell graph, the type registry, and the parsed program. The
//! self-reference is `self_cell`'s; koan writes no `unsafe` for it. Nothing outside names
//! `'graph`: the running state is reached through [`CellSubstrate::with`], whose closure is
//! quantified over a fresh lifetime, and the scheduler is a view made per call over the graph.
//! The graph's root is taken at load, and [`Steps`] is the bundle its steps run over.
//!
//! **Imports.** Outside `#[cfg(test)]` this module names `crate::knot`, `crate::memory`,
//! `crate::parse`, `crate::scheduler`, `crate::symbols`, `crate::type_lattice` and `crate::values`;
//! [`tests::boundary`] reads the source to hold it there.
//!
//! See [program/README.md](program/README.md).

mod steps;
mod substrate;

#[cfg(test)]
mod tests;

pub use steps::Steps;
pub use substrate::{CellSubstrate, Running};
