//! Execute — drives parsed `KExpression`s through a work-stealing scheduler to final
//! `KObject`s. Top-level expressions enter as `Dispatch` nodes against a run-root scope;
//! producer/consumer slots park on each other via `pending_deps` and wake on terminal
//! writes.
//!
//! See [design/execution-model.md](../../design/execution-model.md) and
//! [design/memory-model.md](../../design/memory-model.md).

mod dispatch;
// The shared action-harness for KFunction::invoke + builtins (design sketch / WIP). Hidden behind the
// `action-harness` feature so it stays off the default build until the refactor lands. See harness.rs.
#[cfg(feature = "action-harness")]
mod harness;
mod interpret;
mod lift;
mod nodes;
mod scheduler;

pub use interpret::{interpret, interpret_with_writer, interpret_with_writer_path};
pub use scheduler::Scheduler;

pub(crate) use dispatch::{
    defer_field_list_via_combine, resolve_type_leaf_carrier, TypeLeafCarrier,
};
pub use dispatch::{NameOutcome, ResolveOutcome, ResolveTypeExprOutcome, Resolved};

#[cfg(test)]
pub use lift::lift_ktype_for_test;
