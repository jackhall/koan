//! Execute — drives parsed `KExpression`s through a work-stealing scheduler to final
//! `KObject`s. Top-level expressions enter as `Dispatch` nodes against a run-root scope;
//! producer/consumer slots park on each other via `pending_deps` and wake on terminal
//! writes.
//!
//! See [design/execution-model.md](../../design/execution-model.md) and
//! [design/memory-model.md](../../design/memory-model.md).

mod dispatch;
mod interpret;
mod lift;
mod nodes;
mod scheduler;

pub use interpret::{interpret, interpret_with_writer, interpret_with_writer_path};
pub use scheduler::Scheduler;

pub use dispatch::{NameOutcome, ResolveOutcome, ResolveTypeExprOutcome, Resolved};
pub(crate) use dispatch::{resolve_type_leaf_carrier, TypeLeafCarrier};

#[cfg(test)]
pub use lift::lift_kobject_for_test;
