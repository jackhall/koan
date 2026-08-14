//! Execute — drives parsed `KExpression`s through a work-stealing scheduler to final
//! `KObject`s. A statement crosses into the scheduler as a `WorkingExpression` — the dispatcher's
//! own per-call node, whose slots the scheduler writes resolved sub-results into — and enters as a
//! `Dispatch` node against a run-root scope; a consumer parks on a producer through an edge and
//! wakes when the producer's finalize walk delivers into it.
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
mod producer_id;
// The write harness (KoanRuntime, sole &mut Scheduler) + the shared action harness and the
// program entry points (interpret submodule). See runtime.rs.
mod run_loop;
mod runtime;
mod step_carried;

pub(in crate::machine::execute) use outcome::{
    CatchFinish, ContinuationFamily, TerminalDepFinish, WitnessedDepFinish, catch_continuation,
    ignore_results, seal_witnessed, short_circuit,
};
pub use producer_id::ProducerId;
pub(crate) use producer_id::park_deps;
pub(crate) use runtime::seed_run_root;
pub use runtime::{KoanRuntime, interpret, interpret_with_writer, interpret_with_writer_path};
pub use step_carried::{StepCarried, drive_step_allocator};

pub(crate) use dispatch::{
    BrandCompose, FieldListDeferral, build_type_operand, seal_type_identity,
};
pub(crate) use dispatch::{DispatchOutcome, NameOutcome};
