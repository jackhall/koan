//! Koan's steps and the state they run over, above the scheduler: a loaded program as one owning
//! value, and the body runner that performs it.
//!
//! [`CellSubstrate`] owns program storage and the symbol interner, and beside them the state that
//! borrows them at `'graph`: the cell graph, the type registry, the builtin table and the
//! [`Program`] record. The self-reference is `self_cell`'s; koan writes no `unsafe` for it. Nothing
//! outside names `'graph`: the running state is reached through [`CellSubstrate::with`], whose
//! closure is quantified over a fresh lifetime, and the scheduler is a view made per call over the
//! graph. [`KBundle`] is the step bundle the scheduler runs, and [`run`] is the body runner — the
//! top level's root work, every called body's frame, the frame of the code an `EVAL` runs, and a
//! block's; an evaluation cell runs the evaluation stack, which hands each node in flight
//! to the language's evaluator. [`call`] and [`eval`] are the doors an evaluator asks for frames
//! through. The layer above supplies a [`Language`]: the builtin table, the load-time check, and
//! the evaluator; the embedder supplies the [`Output`] sinks at load, and a run reports its
//! [`Outcome`].
//!
//! **Imports.** Outside `#[cfg(test)]` this module names `crate::build`, `crate::builtin_image`,
//! `crate::knot`, `crate::memory`, `crate::parse`, `crate::scheduler`, `crate::scope`,
//! `crate::symbols`, `crate::type_lattice` and `crate::values`; [`tests::boundary`] reads the
//! source to hold it there.
//!
//! See [program/README.md](program/README.md).

mod body;
mod bundle;
mod compose;
mod exit;
mod record;
mod stack;
mod substrate;

#[cfg(test)]
pub(crate) mod tests;

#[cfg(test)]
pub(crate) use body::ADMITTED_BY_NAME;
#[cfg(test)]
pub(crate) use body::{INSTANCED, bound_of};
pub use body::{Runner, call, eval, run, under_composition};
pub use bundle::{
    ArmKept, Catching, Entered, FrameKept, FrameLink, Handed, KBirth, KBirthFamily, KBundle,
    KScratch, KScratchFamily, KState, KStateFamily, Keeps, Passing, Within,
};
pub use compose::{born_under, called_function};
pub use exit::Exit;
pub use record::{CallKind, Contract, Evaluated, Language, LoadError, Output, Owed, Program, kept};
pub use stack::{BlockEntry, Delivered, Evaluator, Next, Node};
#[cfg(test)]
pub(crate) use substrate::Ran;
#[cfg(test)]
pub(crate) use substrate::walked;
pub use substrate::{CellSubstrate, Outcome, Running, STACK_BYTES, Uncaught};
