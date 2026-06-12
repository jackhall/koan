//! The dispatch write-harness — the peer of
//! [`run_action`](super::super::harness::run_action) for the dispatcher.
//!
//! [`apply_dispatch_outcome`] is the one place that turns a decided [`DispatchOutcome`]
//! into the scheduler graph writes it implies and the terminal [`NodeStep`]. A shape
//! handler decides; this applies. During the incremental migration it threads the writes
//! through the still-`&mut` [`DispatchCtx`] shim; once every handler returns an outcome it
//! becomes the sole `&mut Scheduler` user on the dispatch side.

use crate::machine::core::kfunction::SchedulerHandle;
use crate::machine::NodeId;

use super::super::nodes::{NodeStep, NodeWork};
use super::outcome::{DispatchDep, DispatchOutcome};
use super::DispatchCtx;

// The park edges a `ParkSelf` adds are `Notify` (sibling producers the slot waits on), never
// owned — `add_park_edge` is the right primitive.

/// Interpret a handler's [`DispatchOutcome`] into the scheduler effect it names and return
/// the slot's [`NodeStep`]. Grows one arm per migrated handler.
pub(in crate::machine::execute) fn apply_dispatch_outcome<'run>(
    ctx: &mut DispatchCtx<'run, '_>,
    outcome: DispatchOutcome<'run>,
    idx: usize,
) -> NodeStep<'run> {
    match outcome {
        DispatchOutcome::Terminal(output) => NodeStep::Done(output),
        DispatchOutcome::Combine {
            deps,
            dep_error_frame,
            finish,
        } => {
            // Submit each fresh dep (an `Existing` is already in the graph), install it as an
            // owned edge so it cascade-frees on resolve, and park the slot on the lot as a
            // `DispatchCombine`. Submission order is preserved, so `finish` reads `results[k]`
            // for the k-th declared dep.
            let dep_ids: Vec<NodeId> = deps
                .into_iter()
                .map(|dep| {
                    let id = match dep {
                        DispatchDep::Dispatch(expr) => ctx.scheduler_mut().add_dispatch_here(expr),
                        DispatchDep::ListLit(items) => ctx.schedule_list_literal(items),
                        DispatchDep::DictLit(pairs) => ctx.schedule_dict_literal(pairs),
                        DispatchDep::RecordLit(fields) => ctx.schedule_record_literal(fields),
                        DispatchDep::Existing(id) => id,
                    };
                    ctx.add_owned_edge(id, NodeId(idx));
                    id
                })
                .collect();
            NodeStep::Replace {
                work: NodeWork::DispatchCombine {
                    deps: dep_ids,
                    park_count: 0,
                    finish,
                    dep_error_frame,
                },
                frame: None,
                function: None,
                block_entry: None,
                body_index: 0,
            }
        }
        DispatchOutcome::ParkSelf { producers, state } => {
            for producer in producers {
                ctx.add_park_edge(producer, NodeId(idx));
            }
            ctx.replace_with_parked_dispatch(state)
        }
    }
}
