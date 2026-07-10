//! Execute — drives parsed `KExpression`s through a work-stealing scheduler to final
//! `KObject`s. Top-level expressions enter as `Dispatch` nodes against a run-root scope;
//! producer/consumer slots park on each other via `pending_deps` and wake on terminal
//! writes.
//!
//! See [design/execution/README.md](../../design/execution/README.md) and
//! [design/memory-model.md](../../design/memory-model.md).

mod ambient;
mod dispatch;
mod finalize;
mod lift;
mod nodes;
mod obligation;
mod outcome;
// The write harness (KoanRuntime, sole &mut Scheduler) + the shared action harness and the
// program entry points (interpret submodule). See runtime.rs.
mod run_loop;
mod runtime;

pub(in crate::machine::execute) use outcome::{
    catch_continuation, ignore_results, seal_witnessed, short_circuit, CatchFinish,
    ContinuationFamily, TerminalDepFinish, WitnessedDepFinish,
};
pub use runtime::{interpret, interpret_with_writer, interpret_with_writer_path, KoanRuntime};

pub(crate) use dispatch::{build_type_operand, defer_field_list_action};
pub use dispatch::{DispatchOutcome, NameOutcome, Resolved};
