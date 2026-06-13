//! The dispatch write-harness — the peer of
//! [`run_action`](super::super::harness::run_action) for the dispatcher.
//!
//! [`Scheduler::apply_outcome`] is the one place that turns a decided [`Outcome`] into the
//! scheduler graph writes it implies and the terminal [`NodeStep`]. A shape handler decides
//! against a read-only [`SchedulerView`](super::ctx::SchedulerView) and returns an outcome; this
//! applies it. The harness holds the sole `&mut Scheduler` on the dispatch side.

use crate::machine::core::kfunction::action::{Dep, DepPlacement, FramePlacement};
use crate::machine::model::ast::KExpression;
use crate::machine::model::Carried;
use crate::machine::NodeId;

use super::super::nodes::{DispatchCombineFinish, LiftState, NodeStep, NodeWork};
use super::super::scheduler::Scheduler;
use super::super::SchedulerHandle;
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
    /// Realize a [`Catch`](Continuation::Catch)'s single watched [`Dep`] to a producer `NodeId`.
    /// Unlike a Combine body, an `InScope` watched expr enters a fresh **single-statement** block
    /// (TRY's `child_under` body scope) so an inner `LET` stays local; `Existing` is already a
    /// producer the builtin found in scope.
    fn realize_catch_dep(&mut self, dep: Dep<'run>) -> NodeId {
        match dep {
            Dep::Existing(id) => id,
            Dep::Dispatch { expr, placement } => match placement {
                DepPlacement::OwnScope => self.add_dispatch_here(expr),
                DepPlacement::ActiveFrame => self.add_dispatch_in_frame(expr),
                DepPlacement::InScope(scope) => self
                    .enter_block(scope.id, vec![expr], scope)
                    .into_iter()
                    .next()
                    .expect("enter_block of one statement yields one node"),
            },
        }
    }

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
                leading,
                body_index,
            } => {
                // Resolve the frame placement to the cart the Replace installs: reuse the slot's
                // ping-pong reserve, take a builtin-minted cart, or keep the current cart.
                let frame = match frame {
                    FramePlacement::ReuseReserve { outer } => Some(self.acquire_tail_frame(outer)),
                    FramePlacement::FreshChild { frame } => Some(frame),
                    FramePlacement::Inherit => None,
                };
                // The body's non-tail statements dispatch as siblings against the resolved cart via
                // the shared `dispatch_body_statements` primitive; the slot tail-replaces into the
                // last statement separately below. A decide that carries `leading` issues no write
                // itself — the submission lands here.
                if !leading.is_empty() {
                    let cart = frame
                        .clone()
                        .expect("a Continue with leading statements requires a frame");
                    self.dispatch_body_statements(&cart, leading);
                }
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
                // is preserved, so a finish reads `results[k]` for the k-th declared dep — except
                // an `InScope`-placed `Dispatch`, whose multi-statement body fans out to one
                // producer per statement (the only one-dep-to-many case, so this is a loop not a
                // `map`).
                let mut dep_ids: Vec<NodeId> = Vec::with_capacity(deps.len());
                for dep in deps {
                    match dep {
                        DispatchDep::Dispatch { expr, placement } => match placement {
                            DepPlacement::OwnScope => dep_ids.push(self.add_dispatch_here(expr)),
                            DepPlacement::ActiveFrame => {
                                dep_ids.push(self.add_dispatch_in_frame(expr))
                            }
                            DepPlacement::InScope(scope) => {
                                dep_ids.extend(self.enter_body_block(scope, expr))
                            }
                        },
                        DispatchDep::ListLit(items) => {
                            dep_ids.push(self.schedule_list_literal(items))
                        }
                        DispatchDep::DictLit(pairs) => {
                            dep_ids.push(self.schedule_dict_literal(pairs))
                        }
                        DispatchDep::RecordLit(fields) => {
                            dep_ids.push(self.schedule_record_literal(fields))
                        }
                        DispatchDep::BodyBlock { frame, statements } => {
                            dep_ids.extend(self.dispatch_body_statements(&frame, statements))
                        }
                        DispatchDep::Existing(id) => dep_ids.push(id),
                    }
                }
                // Edge install: the `[..park_count]` prefix is notify-parked (sibling producers
                // the slot waits on but doesn't own); the `[park_count..]` suffix is owned
                // (cascade-freed on resolve). Each continuation sets `park_count` to match: a
                // dispatch `Finish` owns all its deps (`park_count: 0`); an action `Combine` parks
                // its `Existing` prefix and owns its `Dispatch` suffix; `Replay` parks every
                // producer (`park_count: len`); a bare-name `Forward` parks its one producer
                // (`park_count: 1`) while a deferred-combine `Forward` owns it (`park_count: 0`).
                // (`Catch` declares no deps here — it realizes and owns its single watched dep in
                // the `cont` match below.)
                for (i, id) in dep_ids.iter().enumerate() {
                    if i < park_count {
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
                    // The action-harness combine: the slot becomes a `NodeWork::Combine` over the
                    // realized deps (its edges already installed by the loop above).
                    Continuation::Combine(finish) => NodeWork::Combine {
                        deps: dep_ids,
                        park_count,
                        finish,
                    },
                    // The action-harness catch carries its single watched dep unrealized (its
                    // placement differs from a Combine body's fan-out); realize and own it here.
                    Continuation::Catch { watched, finish } => {
                        let from = self.realize_catch_dep(watched);
                        self.add_owned_edge(from, NodeId(idx));
                        NodeWork::Catch { from, finish }
                    }
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
                // The dispatch→execution hand-off. A user fn runs in a freshly acquired per-call
                // frame (the harness's irreducible write — TCO reuse mutates the reserve); a
                // builtin runs in the current frame. `invoke` is a pure decide that reads that
                // frame, so the harness acquires it here and applies the outcome `invoke` returns.
                drain_free(self, free);
                let frame = match &picked.body {
                    crate::machine::core::kfunction::Body::Builtin(_) => None,
                    _ => Some(self.acquire_tail_frame(picked.captured_scope())),
                };
                let oc = super::exec::invoke(&SchedulerView::new(self), frame, picked, working_expr);
                self.apply_outcome(oc, idx)
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
