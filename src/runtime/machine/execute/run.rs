//! Planner-side `Scheduler` methods, organized by node kind:
//!
//! - `dispatch` — `run_dispatch`: bare-name short-circuit, auto-wrap, replay-park
//!   placeholder routing, sub-node spawning.
//! - `literal` — list/dict-literal `Combine` planners.
//! - `finish` — `run_bind`, `run_combine`, `run_lift`, `invoke_to_step`: consume dep
//!   results and decode `BodyResult` into the next `NodeStep`.
//!
//! Only `defer_to_lift` is shared across the three groups, so it lives here.

use crate::runtime::machine::NodeId;

use super::nodes::{DepEdge, NodeStep, NodeWork};
use super::scheduler::Scheduler;

mod dispatch;
mod finish;
mod literal;
#[cfg(test)]
mod tests;

impl<'a> Scheduler<'a> {
    /// Frame / function are left as `None` so the slot's existing per-call frame and
    /// function label stay attached when the Lift writes its terminal.
    ///
    /// `bind_id` was just spawned by this slot's `run_dispatch`, so it lands in
    /// `dep_edges[idx]` as `Owned`: the Lift owns its underlying Bind/Combine and
    /// must cascade-free it. When a Dispatch slot first parked via replay-park and
    /// then re-dispatched here, the resulting `dep_edges[idx]` is the mixed shape
    /// `[Notify(producer), …, Owned(bind_id)]` — exactly the case `free`'s
    /// `Owned`-only recursion handles correctly.
    pub(super) fn defer_to_lift(&mut self, idx: usize, bind_id: NodeId) -> NodeStep<'a> {
        self.dep_edges[idx].push(DepEdge::Owned(bind_id));
        NodeStep::Replace {
            work: NodeWork::Lift { from: bind_id },
            frame: None,
            function: None,
        }
    }
}
