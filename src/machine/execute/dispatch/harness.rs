//! The dispatch write-harness — the peer of
//! [`run_action`](super::super::harness::run_action) for the dispatcher.
//!
//! [`Scheduler::apply_outcome`] is the one place that turns a decided [`Outcome`] into the
//! scheduler graph writes it implies and the terminal [`NodeStep`]. A shape handler decides
//! against a read-only [`SchedulerView`](super::ctx::SchedulerView) and returns an outcome; this
//! applies it. The harness holds the sole `&mut Scheduler` on the dispatch side.

use crate::machine::core::kfunction::action::{Dep, DepPlacement, FramePlacement};
use crate::machine::core::kfunction::body::split_body_statements;
use crate::machine::{NodeId, TraceFrame};

use super::super::nodes::{NodeOutput, NodeStep, NodeWork};
use super::super::scheduler::Scheduler;
use super::super::{catch_cont, ignore_results, short_circuit};
use super::{Continuation, DispatchDep, Outcome};

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
                DepPlacement::OwnScope => self.dispatch_here(expr),
                DepPlacement::ActiveFrame => self.add_dispatch_in_frame(expr),
                DepPlacement::InScope(scope) => self
                    .enter_block(scope.id, vec![expr], scope)
                    .into_iter()
                    .next()
                    .expect("enter_block of one statement yields one node"),
            },
        }
    }

    /// Resolve a [`FramePlacement`] to the cart a [`Continue`](Outcome::Continue) installs: reuse
    /// the slot's ping-pong reserve (the TCO tail-call cart), take a builtin-minted fresh cart, or
    /// keep the current cart (`None`). The one place the placement → cart mapping lives — shared by
    /// the `Continue` body re-run and the folded invoke / re-resolve paths (which reach it through
    /// their own `Continue`).
    fn resolve_frame_placement(
        &mut self,
        placement: FramePlacement<'run>,
    ) -> Option<std::rc::Rc<crate::machine::core::CallArena>> {
        match placement {
            FramePlacement::ReuseReserve { outer } => Some(self.acquire_tail_frame(outer)),
            FramePlacement::FreshChild { frame } => Some(frame),
            FramePlacement::Inherit => None,
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
                body_index,
                free,
            } => {
                // Reclaim the Reuse producers the decide phase consumed inline before installing the
                // replacement (mirrors the `ParkThenContinue` arm).
                drain_free(self, free);
                // The body's leading statements are never dispatched here — a producer with leading
                // statements parks on them as owned `BodyBlock` deps and emits this `Continue` only
                // from the resolving finish (see `dispatch/exec.rs` and `execute/harness.rs`).
                let frame = self.resolve_frame_placement(frame);
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
                            DepPlacement::OwnScope => dep_ids.push(self.dispatch_here(expr)),
                            DepPlacement::ActiveFrame => {
                                dep_ids.push(self.add_dispatch_in_frame(expr))
                            }
                            DepPlacement::InScope(scope) => {
                                let statements = split_body_statements(expr);
                                dep_ids.extend(self.enter_block(scope.id, statements, scope))
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
                    // A dispatch finish carries its own dep-error frame (the consuming call's, or
                    // `None` frameless); an action/literal combine is labelled `<combine>` — the
                    // one place that policy lives. Both install the same `Wait` over the realized
                    // deps (edges already installed by the loop above), the short-circuit baked into
                    // the continuation by `short_circuit`.
                    Continuation::Finish(finish) => NodeWork {
                        deps: dep_ids,
                        park_count,
                        cont: short_circuit(dep_error_frame, finish),
                        carrier: None,
                    },
                    Continuation::Combine(finish) => NodeWork {
                        deps: dep_ids,
                        park_count,
                        cont: short_circuit(Some(TraceFrame::bare("<combine>", "combine")), finish),
                        carrier: None,
                    },
                    // The action-harness catch carries its single watched dep unrealized (its
                    // placement differs from a Combine body's fan-out); realize and own it here.
                    // `catch_cont` runs the finish without short-circuiting on a dep error.
                    Continuation::Catch { watched, finish } => {
                        let from = self.realize_catch_dep(watched);
                        self.add_owned_edge(from, NodeId(idx));
                        NodeWork {
                            deps: vec![from],
                            park_count: 0,
                            cont: catch_cont(finish),
                            carrier: None,
                        }
                    }
                    // The resume closure carries the evolving `working_expr` from here on; the
                    // `carrier` it travels with is only the deadlock-summary sample. A decide takes
                    // no dep values, so `ignore_results` drops the (park-only) results slice.
                    Continuation::Resume { carrier, resume } => NodeWork {
                        deps: dep_ids,
                        park_count,
                        cont: ignore_results(resume),
                        carrier,
                    },
                };
                NodeStep::Replace {
                    work,
                    frame: None,
                    function: None,
                    block_entry: None,
                    body_index: 0,
                }
            }
            Outcome::Forward(producer) => {
                // The slot's result *is* `producer`'s. If `producer` is ready, finalize the slot
                // with its terminal directly. Otherwise splice the slot out: move its consumers onto
                // `producer`'s notify list and alias the slot to `producer` — `producer` becomes the
                // sole producer of this result, with no forwarding node and no extra wake hop.
                if self.is_result_ready(producer) {
                    match self.read_result(producer) {
                        Ok(c) => NodeStep::Done(NodeOutput::Value(c)),
                        Err(e) => NodeStep::Done(NodeOutput::Err(e.clone_for_propagation())),
                    }
                } else {
                    // Not ready: `NodeStep::Alias` drives `splice_forward` (move consumers onto the
                    // producer + alias the slot) in the execute loop.
                    NodeStep::Alias(producer)
                }
            }
        }
    }
}
