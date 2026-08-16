//! Execute — drives parsed `KExpression`s through a work-stealing scheduler to final
//! `KObject`s. A statement crosses into the scheduler as a `WorkingExpression` — the dispatcher's
//! own per-call node, whose slots the scheduler writes resolved sub-results into — and enters as a
//! `Dispatch` node against a run-root scope; a consumer parks on a producer through an edge and
//! wakes when the producer's finalize walk delivers into it.
//!
//! The layer is two phases with one door each: a *decide* ([`decide`]) runs against a read-only
//! [`DecideCtx`](decide::DecideCtx) and returns an [`Outcome`](outcome::Outcome); the *apply* half
//! ([`harness`]) is the sole holder of `&mut Scheduler`, mapping each outcome onto the
//! [`StepVerdict`](crate::scheduler::StepVerdict) the scheduler's drain applies.
//!
//! See [design/execution/README.md](../../design/execution/README.md) and
//! [design/memory-model.md](../../design/memory-model.md).

mod ambient;
mod decide;
mod finalize;
mod harness;
mod interpret;
mod lift;
mod nodes;
mod obligation;
mod outcome;
mod producer_id;
mod step_carried;
#[cfg(test)]
mod test_support;

pub use harness::KoanRuntime;
pub(crate) use interpret::seed_run_root;
pub use interpret::{interpret, interpret_with_writer, interpret_with_writer_path};
pub(in crate::machine::execute) use outcome::{
    CatchFinish, ContinuationFamily, TerminalDepFinish, WitnessedDepFinish, catch_continuation,
    ignore_results, seal_witnessed, short_circuit,
};
pub use producer_id::ProducerId;
pub(crate) use producer_id::{deps_on, extend_deps_on};
pub use step_carried::{StepCarried, drive_step_allocator};
#[cfg(test)]
pub(crate) use test_support::edge_delivered;

pub(crate) use decide::DispatchOutcome;
#[cfg(test)]
pub(crate) use decide::Resolution;
pub(crate) use decide::{BrandCompose, FieldListDeferral, build_type_operand, seal_type_identity};
