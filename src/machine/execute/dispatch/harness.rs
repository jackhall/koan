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

use super::super::nodes::{LiftState, NodeStep, NodeWork};
use super::outcome::{DispatchDep, DispatchOutcome};
use super::DispatchCtx;

// The park edges a `ParkSelf` adds are `Notify` (sibling producers the slot waits on), never
// owned — `add_park_edge` is the right primitive.

/// Reclaim the producers a decide phase consumed inline (a ready `Reuse` spliced into a
/// `working_expr`). Deferred off the decide phase so the handler stays read-only; the harness
/// is the sole writer, so the free lands here.
fn drain_free(ctx: &mut DispatchCtx<'_, '_>, free: Vec<usize>) {
    for id in free {
        ctx.free(id);
    }
}

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
            free,
        } => {
            // Reclaim the Reuse producers the decide phase consumed inline before declaring deps.
            drain_free(ctx, free);
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
        DispatchOutcome::Invoke {
            picked,
            working_expr,
            free,
        } => {
            // The dispatch→execution hand-off: run the resolved call against the raw
            // `&mut Scheduler` and lower its body onto the slot.
            drain_free(ctx, free);
            let body = super::exec::invoke(ctx.scheduler_mut(), picked, working_expr);
            ctx.body_result_to_step(body, idx)
        }
        DispatchOutcome::Redispatch { working_expr, free } => {
            drain_free(ctx, free);
            let outcome = super::keyworded::KeywordedState::finish(&ctx.read_view(), working_expr, idx);
            apply_dispatch_outcome(ctx, outcome, idx)
        }
        DispatchOutcome::ParkLift { producer } => {
            // Notify edge, not Owned: the producer is a sibling slot we only wait on. The slot
            // then becomes a pending `Lift`, which adopts the producer's resolved value directly.
            ctx.add_park_edge(producer, NodeId(idx));
            NodeStep::Replace {
                work: NodeWork::Lift(LiftState::Pending(producer)),
                frame: None,
                function: None,
                block_entry: None,
                body_index: 0,
            }
        }
        DispatchOutcome::BecomeDispatch(inner) => NodeStep::Replace {
            work: NodeWork::dispatch(inner),
            frame: None,
            function: None,
            block_entry: None,
            body_index: 0,
        },
        DispatchOutcome::ElaborateRecordType { fields, chain } => {
            // Execution layer: the field-list elaborator holds `&mut Scheduler` and may defer
            // through a Combine; lower its body onto the slot like any resolved call.
            let body = super::field_list::elaborate_record_value(ctx.scheduler_mut(), fields, chain);
            ctx.body_result_to_step(body, idx)
        }
    }
}
