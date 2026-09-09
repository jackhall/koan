//! Execute — drives parsed `KExpression`s through a work-stealing scheduler to final `KObject`s.
//! A statement crosses into the scheduler as a `WorkingExpression`, the dispatcher's own per-call
//! node; a consumer parks on a producer through an edge and wakes when the producer's finalize
//! walk delivers into it.
//!
//! The layer is two phases with one door each: a *decide* ([`decide`]) runs against a read-only
//! [`DecideCtx`](decide::DecideCtx) and returns an [`Outcome`](outcome::Outcome); the *apply* half
//! ([`harness`]) maps each outcome onto the
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
/// The reach-tightness report — the over-pinning audit at the fold chokepoint, compiled only under
/// the `region-audit` gate. Its body lives in [`audit/`](../../audit/README.md), the home for
/// measurement code that no build ships; what stays in `src/` is this declaration and the hooks
/// inside [`StepAllocator::alloc_carried_with`](step::StepAllocator), which are the fold moment
/// itself and so cannot move.
#[cfg(any(test, feature = "region-audit"))]
#[path = "../../audit/reach_audit.rs"]
pub mod reach_audit;
pub(crate) mod step;
#[cfg(test)]
mod test_support;

pub use harness::KoanRuntime;
pub(crate) use interpret::seed_run_root;
pub use interpret::{interpret, interpret_with_writer, interpret_with_writer_path};
pub(in crate::machine::execute) use outcome::{
    ContinuationCall, ContinuationFamily, NodeContinuation, decide_only, erase_bumped, gated,
    sealed_done,
};
#[cfg(test)]
pub(in crate::machine::execute) use outcome::{erase_boxed, gated_once};
pub use producer_id::ProducerId;
pub(crate) use producer_id::{deps_on, extend_deps_on};
pub use step::{StepAllocator, StepCarried, drive_step_allocator};
#[cfg(test)]
pub(crate) use test_support::edge_delivered;

pub(crate) use decide::DispatchOutcome;
#[cfg(test)]
pub(crate) use decide::Resolution;
pub(crate) use decide::{FieldListDeferral, build_type_operand, seal_type_identity};
