//! Koan's steps and the state they run over, above the scheduler: a loaded program as one owning
//! value, and the body runner that performs it.
//!
//! [`CellSubstrate`] owns program storage and the symbol interner, and beside them the state that
//! borrows them at `'graph`: the cell graph, the type registry, the builtin table and the
//! [`Program`] record. The self-reference is `self_cell`'s; koan writes no `unsafe` for it. Nothing
//! outside names `'graph`: the running state is reached through [`CellSubstrate::with`], whose
//! closure is quantified over a fresh lifetime, and the scheduler is a view made per call over the
//! graph. [`KBundle`] is the step bundle the scheduler runs, and [`run`] is the body runner — the
//! top level's root work, and every called body's frame. The layer above supplies a [`Language`]:
//! the builtin table, and the step every evaluation runs.
//!
//! **Imports.** Outside `#[cfg(test)]` this module names `crate::elaborate`, `crate::knot`,
//! `crate::memory`, `crate::parse`, `crate::scheduler`, `crate::scope`, `crate::symbols`,
//! `crate::type_lattice` and `crate::values`; [`tests::boundary`] reads the source to hold it
//! there.
//!
//! See [program/README.md](program/README.md).

mod body;
mod bundle;
mod record;
mod substrate;

#[cfg(test)]
mod tests;

pub use body::{Runner, call, placement_of, run};
pub use bundle::{KBirth, KBirthFamily, KBundle, KScratchFamily, KState, KStateFamily};
pub use record::{Evaluated, Language, LoadError, Program};
pub use substrate::{CellSubstrate, Running};
