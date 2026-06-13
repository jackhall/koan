//! The dispatch write-harness — the peer of
//! [`run_action`](super::super::harness::run_action) for the dispatcher.
//!
//! [`Scheduler::apply_outcome`] is the one place that turns a decided [`Outcome`] into the
//! scheduler graph writes it implies and the terminal [`NodeStep`]. A shape handler decides
//! against a read-only [`SchedulerView`](super::ctx::SchedulerView) and returns an outcome; this
//! applies it. The harness holds the sole `&mut Scheduler` on the dispatch side.

use crate::machine::core::kfunction::action::FramePlacement;
use crate::machine::core::kfunction::{BodyResult, SchedulerHandle};
use crate::machine::model::ast::KExpression;
use crate::machine::model::Carried;
use crate::machine::NodeId;

use super::super::nodes::{DispatchCombineFinish, LiftState, NodeOutput, NodeStep, NodeWork};
use super::super::scheduler::Scheduler;
use super::ctx::SchedulerView;
use super::{Continuation, DispatchDep, Outcome};

/// Run a [`NodeWork::DispatchCombine`] finish at wake: build the read-only view, decide, and
/// apply the returned outcome — the bridge `run_dispatch_combine` (the scheduler wake side) calls
/// so the `read_view` → decide → apply dance stays inside the dispatch harness. The finish sees a
/// `&SchedulerView`, so it — like every decide — issues no graph write itself.
pub(in crate::machine::execute) fn run_dispatch_combine_finish<'run>(
    sched: &mut Scheduler<'run>,
    finish: DispatchCombineFinish<'run>,
    values: &[Carried<'run>],
    idx: usize,
) -> NodeStep<'run> {
    let outcome = finish(&SchedulerView::new(sched), values, idx);
    sched.apply_outcome(outcome, idx)
}

/// Reclaim the producers a decide phase consumed inline (a ready `Reuse` spliced into a
/// `working_expr`). Deferred off the decide phase so the handler stays read-only; the harness
/// is the sole writer, so the free lands here.
fn drain_free(sched: &mut Scheduler<'_>, free: Vec<usize>) {
    for id in free {
        sched.free(id);
    }
}

impl<'run> Scheduler<'run> {
    /// Interpret an [`Outcome`] into the scheduler effect it names and return the slot's
    /// [`NodeStep`]. This is the sole graph writer the dispatch side reaches — a decide handler
    /// never holds `&mut Scheduler`.
    pub(in crate::machine::execute) fn apply_outcome(
        &mut self,
        outcome: Outcome<'run>,
        idx: usize,
    ) -> NodeStep<'run> {
        match outcome {
            Outcome::Done(output) => NodeStep::Done(output),
            Outcome::Continue {
                work,
                frame,
                contract,
                block_entry,
                body_index,
            } => {
                // Resolve the frame placement to the cart the Replace installs: reuse the slot's
                // ping-pong reserve, take a builtin-minted cart, or keep the current cart.
                let frame = match frame {
                    FramePlacement::ReuseReserve { outer } => Some(self.acquire_tail_frame(outer)),
                    FramePlacement::FreshChild { frame } => Some(frame),
                    FramePlacement::Inherit => None,
                };
                NodeStep::Replace {
                    work,
                    frame,
                    function: contract,
                    block_entry,
                    body_index,
                }
            }
            Outcome::ParkThenContinue {
                deps,
                park_count,
                cont,
                dep_error_frame,
                free,
            } => {
                // Reclaim the Reuse producers the decide phase consumed inline before declaring
                // deps.
                drain_free(self, free);
                // Submit each fresh dep (an `Existing` is already in the graph). Submission order
                // is preserved, so a finish reads `results[k]` for the k-th declared dep.
                let dep_ids: Vec<NodeId> = deps
                    .into_iter()
                    .map(|dep| match dep {
                        DispatchDep::Dispatch(expr) => self.add_dispatch_here(expr),
                        DispatchDep::ListLit(items) => self.schedule_list_literal(items),
                        DispatchDep::DictLit(pairs) => self.schedule_dict_literal(pairs),
                        DispatchDep::RecordLit(fields) => self.schedule_record_literal(fields),
                        DispatchDep::Existing(id) => id,
                    })
                    .collect();
                // Edge install: a `Finish` owns its dep suffix (`[park_count..]`, cascade-freed on
                // resolve) and notify-parks its prefix; a `Replay`/`Forward` notify-parks on every
                // producer (the slot re-decides or forwards a value — it owns nothing).
                let park_prefix = if matches!(cont, Continuation::Finish(_)) {
                    park_count
                } else {
                    dep_ids.len()
                };
                for (i, id) in dep_ids.iter().enumerate() {
                    if i < park_prefix {
                        self.add_park_edge(*id, NodeId(idx));
                    } else {
                        self.add_owned_edge(*id, NodeId(idx));
                    }
                }
                let work = match cont {
                    Continuation::Finish(finish) => NodeWork::DispatchCombine {
                        deps: dep_ids,
                        park_count,
                        finish,
                        dep_error_frame,
                    },
                    // The state carries the evolving `working_expr` from here on, so the entry
                    // expression drops to an empty placeholder.
                    Continuation::Replay(state) => NodeWork::Dispatch {
                        expr: KExpression::new(Vec::new()),
                        state,
                    },
                    Continuation::Forward(producer) => NodeWork::Lift(LiftState::Pending(producer)),
                };
                NodeStep::Replace {
                    work,
                    frame: None,
                    function: None,
                    block_entry: None,
                    body_index: 0,
                }
            }
            Outcome::Invoke {
                picked,
                working_expr,
                free,
            } => {
                // The dispatch→execution hand-off: run the resolved call against the raw
                // `&mut Scheduler` and lower its body onto the slot.
                drain_free(self, free);
                let body = super::exec::invoke(self, picked, working_expr);
                lower_body_result(self, body, idx)
            }
            Outcome::Elaborate { fields, chain } => {
                // Execution layer: the field-list elaborator holds `&mut Scheduler` and may defer
                // through a Combine; lower its body onto the slot like any resolved call.
                let body = super::field_list::elaborate_record_value(self, fields, chain);
                lower_body_result(self, body, idx)
            }
            Outcome::Redispatch { working_expr, free } => {
                // Re-resolve dispatch against the now fully-spliced `working_expr` immediately
                // (the post-eager-subs continuation with no speculatively pre-picked function).
                drain_free(self, free);
                let outcome = super::keyworded::KeywordedState::finish(
                    &SchedulerView::new(self),
                    working_expr,
                    idx,
                );
                self.apply_outcome(outcome, idx)
            }
        }
    }
}

/// Lower a resolved body's [`BodyResult`] onto the slot's [`NodeStep`] — shared by the `Invoke`
/// and `Elaborate` arms (a value/error completes the slot, a `Tail` re-dispatches, a `DeferTo`
/// parks on the named lift).
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
