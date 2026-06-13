//! The dispatch write-harness — the peer of
//! [`run_action`](super::super::harness::run_action) for the dispatcher.
//!
//! [`apply_dispatch_outcome`] is the one place that turns a decided [`DispatchOutcome`]
//! into the scheduler graph writes it implies and the terminal [`NodeStep`]. A shape
//! handler decides against a read-only [`DispatchCx`](super::ctx::DispatchCx) and returns an
//! outcome; this applies it. The harness holds the sole `&mut Scheduler` on the dispatch side.

use crate::machine::core::kfunction::{BodyResult, SchedulerHandle};
use crate::machine::model::ast::KExpression;
use crate::machine::model::Carried;
use crate::machine::NodeId;

use super::super::nodes::{DispatchCombineFinish, LiftState, NodeOutput, NodeStep, NodeWork};
use super::super::scheduler::Scheduler;
use super::ctx::DispatchCx;
use super::outcome::{DispatchDep, DispatchOutcome};
use super::DispatchState;

// The park edges a `ParkSelf` adds are `Notify` (sibling producers the slot waits on), never
// owned — `add_park_edge` is the right primitive.

/// Run a [`NodeWork::DispatchCombine`] finish at wake: build the read-only view, decide, and
/// apply the returned outcome — the bridge `run_dispatch_combine` (the scheduler wake side) calls
/// so the `read_view` → decide → apply dance stays inside the dispatch harness. The finish sees a
/// `&DispatchCx`, so it — like every decide — issues no graph write itself.
pub(in crate::machine::execute) fn run_dispatch_combine_finish<'run>(
    sched: &mut Scheduler<'run>,
    finish: DispatchCombineFinish<'run>,
    values: &[Carried<'run>],
    idx: usize,
) -> NodeStep<'run> {
    let outcome = finish(&DispatchCx::new(sched), values, idx);
    apply_dispatch_outcome(sched, outcome, idx)
}

/// Reclaim the producers a decide phase consumed inline (a ready `Reuse` spliced into a
/// `working_expr`). Deferred off the decide phase so the handler stays read-only; the harness
/// is the sole writer, so the free lands here.
fn drain_free(sched: &mut Scheduler<'_>, free: Vec<usize>) {
    for id in free {
        sched.free(id);
    }
}

/// Interpret a handler's [`DispatchOutcome`] into the scheduler effect it names and return the
/// slot's [`NodeStep`]. This is the dispatch-side write owner: it holds the `&mut Scheduler`, so a
/// decide handler never does. Grows one arm per outcome variant.
pub(in crate::machine::execute) fn apply_dispatch_outcome<'run>(
    sched: &mut Scheduler<'run>,
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
            drain_free(sched, free);
            // Submit each fresh dep (an `Existing` is already in the graph), install it as an
            // owned edge so it cascade-frees on resolve, and park the slot on the lot as a
            // `DispatchCombine`. Submission order is preserved, so `finish` reads `results[k]`
            // for the k-th declared dep.
            let dep_ids: Vec<NodeId> = deps
                .into_iter()
                .map(|dep| {
                    let id = match dep {
                        DispatchDep::Dispatch(expr) => sched.add_dispatch_here(expr),
                        DispatchDep::ListLit(items) => sched.schedule_list_literal(items),
                        DispatchDep::DictLit(pairs) => sched.schedule_dict_literal(pairs),
                        DispatchDep::RecordLit(fields) => sched.schedule_record_literal(fields),
                        DispatchDep::Existing(id) => id,
                    };
                    sched.add_owned_edge(id, NodeId(idx));
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
                sched.add_park_edge(producer, NodeId(idx));
            }
            replace_with_parked_dispatch(state)
        }
        DispatchOutcome::Invoke {
            picked,
            working_expr,
            free,
        } => {
            // The dispatch→execution hand-off: run the resolved call against the raw
            // `&mut Scheduler` and lower its body onto the slot.
            drain_free(sched, free);
            let body = super::exec::invoke(sched, picked, working_expr);
            lower_body_result(sched, body, idx)
        }
        DispatchOutcome::Redispatch { working_expr, free } => {
            drain_free(sched, free);
            let outcome =
                super::keyworded::KeywordedState::finish(&DispatchCx::new(sched), working_expr, idx);
            apply_dispatch_outcome(sched, outcome, idx)
        }
        DispatchOutcome::ParkLift { producer } => {
            // Notify edge, not Owned: the producer is a sibling slot we only wait on. The slot
            // then becomes a pending `Lift`, which adopts the producer's resolved value directly.
            sched.add_park_edge(producer, NodeId(idx));
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
            let body = super::field_list::elaborate_record_value(sched, fields, chain);
            lower_body_result(sched, body, idx)
        }
    }
}

/// Lower a resolved body's [`BodyResult`] onto the slot's [`NodeStep`] — shared by the `Invoke`
/// and `ElaborateRecordType` arms (a value/error completes the slot, a `Tail` re-dispatches, a
/// `DeferTo` parks on the named lift).
fn lower_body_result<'run>(
    sched: &mut Scheduler<'run>,
    body: BodyResult<'run>,
    idx: usize,
) -> NodeStep<'run> {
    match body {
        BodyResult::Value(c) => NodeStep::Done(NodeOutput::Value(c)),
        BodyResult::Tail {
            expr,
            frame,
            function,
            block_entry,
            body_index,
        } => NodeStep::Replace {
            work: NodeWork::dispatch(expr),
            frame,
            function,
            block_entry,
            body_index,
        },
        BodyResult::DeferTo(id) => sched.defer_to_lift(idx, id),
        BodyResult::Err(e) => NodeStep::Done(NodeOutput::Err(e)),
    }
}

/// Replace a slot with a parked `Dispatch` carrying `state`: drop the entry expression to an empty
/// placeholder (the state carries the evolving `working_expr` from here on).
fn replace_with_parked_dispatch<'run>(state: DispatchState<'run>) -> NodeStep<'run> {
    NodeStep::Replace {
        work: NodeWork::Dispatch {
            expr: KExpression::new(Vec::new()),
            state,
        },
        frame: None,
        function: None,
        block_entry: None,
        body_index: 0,
    }
}
