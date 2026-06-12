//! The dispatch write-harness — the peer of
//! [`run_action`](super::super::harness::run_action) for the dispatcher.
//!
//! [`apply_dispatch_outcome`] is the one place that turns a decided [`DispatchOutcome`]
//! into the scheduler graph writes it implies and the terminal [`NodeStep`]. A shape
//! handler decides; this applies. During the incremental migration it threads the writes
//! through the still-`&mut` [`DispatchCtx`] shim; once every handler returns an outcome it
//! becomes the sole `&mut Scheduler` user on the dispatch side.

use super::super::nodes::NodeStep;
use super::outcome::DispatchOutcome;
use super::DispatchCtx;

/// Interpret a handler's [`DispatchOutcome`] into the scheduler effect it names and return
/// the slot's [`NodeStep`]. Grows one arm per migrated handler.
pub(in crate::machine::execute) fn apply_dispatch_outcome<'run>(
    _ctx: &mut DispatchCtx<'run, '_>,
    outcome: DispatchOutcome<'run>,
    _idx: usize,
) -> NodeStep<'run> {
    match outcome {
        DispatchOutcome::Terminal(output) => NodeStep::Done(output),
    }
}
