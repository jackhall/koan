//! Execute — drives parsed `KExpression`s through a work-stealing scheduler to final
//! `KObject`s. Top-level expressions enter as `Dispatch` nodes against a run-root scope;
//! producer/consumer slots park on each other via `pending_deps` and wake on terminal
//! writes.
//!
//! See [design/execution-model.md](../../design/execution-model.md) and
//! [design/memory-model.md](../../design/memory-model.md).

mod dispatch;
mod lift;
mod nodes;
mod outcome;
// The write harness (KoanRuntime, sole &mut Scheduler) + the shared action harness and the
// program entry points (interpret submodule). See runtime.rs.
mod runtime;
mod scheduler;

pub(in crate::machine::execute) use outcome::{
    catch_cont, ignore_results, short_circuit, CatchFinish, CombineFinish, NodeCont,
};
pub use runtime::{interpret, interpret_with_writer, interpret_with_writer_path, KoanRuntime};
pub use scheduler::Scheduler;

pub(crate) use dispatch::{defer_field_list_action, resolve_type_leaf_carrier, TypeLeafCarrier};
pub use dispatch::{NameOutcome, ResolveOutcome, ResolveTypeExprOutcome, Resolved};

#[cfg(test)]
pub use lift::lift_ktype_for_test;
