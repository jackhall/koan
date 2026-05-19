use crate::machine::{Frame, KError, KErrorKind, NodeId};

use super::super::lift::lift_kobject;
use super::super::nodes::{LiftState, Node, NodeOutput, NodeStep, NodeWork};
use super::Scheduler;

impl<'a> Scheduler<'a> {
    /// `NodeStep::Replace` is the tail-call path: rewrite the slot's work in place and
    /// re-enqueue.
    ///
    /// On `Done` with a frame, the return `Value` references the per-call arena that's
    /// about to drop, so it must be lifted into the captured scope's arena before the
    /// frame is released. See design/memory-model.md.
    pub fn execute(&mut self) -> Result<(), KError> {
        while let Some(idx) = self.queues.pop_next() {
            let id = NodeId(idx);
            let node = self.store.take_for_run(id);
            let scope = node.scope;
            let work = node.work;
            let prev_function = node.function;
            // Move the slot's frame into `active_frame` (no clone) so the Rc lives in
            // exactly one place during the step. Builtins read it through
            // `SchedulerHandle::current_frame`; tail-reuse takes it via
            // `try_take_reusable_frame_for_tail`. After the step we mem::replace it
            // back out — if the step consumed it for reuse, the slot's frame is now
            // `None` and the new frame arrives via `NodeStep::Replace`.
            let prev_active = std::mem::replace(&mut self.active_frame, node.frame);
            let step = match work {
                NodeWork::Dispatch(expr) => self.run_dispatch(expr, scope, idx)?,
                NodeWork::Bind { expr, subs } => self.run_bind(expr, subs, scope, idx)?,
                NodeWork::Combine { deps, finish } => self.run_combine(deps, finish, scope, idx),
                NodeWork::Catch { from, finish } => self.run_catch(from, finish, scope, idx),
                NodeWork::Lift(state) => NodeStep::Done(Self::run_lift(state)),
            };
            let prev_frame = std::mem::replace(&mut self.active_frame, prev_active);
            // Drain re-entrant writes while `scope` is still live; match arms below may
            // drop the frame it's anchored to. See design/memory-model.md.
            scope.drain_pending();
            match step {
                NodeStep::Done(output) => {
                    match (output, prev_frame) {
                        (NodeOutput::Value(v), Some(frame)) => {
                            let dest = scope
                                .outer
                                .expect("per-call scope must have an outer (its captured scope)")
                                .arena;
                            let lifted_obj = lift_kobject(v, &frame);
                            if let Some(f) = prev_function {
                                // Only run the lift-time return-type check for `Resolved`
                                // types. `Deferred` returns route their per-call check
                                // through the Combine finish that joins the lifted body
                                // value with the per-call elaboration's `KType`; the
                                // static carrier here can't see that resolution, and
                                // skipping it avoids misattributing a body-internal
                                // mismatch.
                                let rt = &f.signature.return_type;
                                if rt.is_resolved() && !rt.matches_value(&lifted_obj) {
                                    let err = KError::new(KErrorKind::TypeMismatch {
                                        arg: "<return>".to_string(),
                                        expected: rt.name(),
                                        got: lifted_obj.ktype().name(),
                                    })
                                    .with_frame(Frame {
                                        function: f.summarize(),
                                        expression: f.summarize(),
                                    });
                                    self.finalize(idx, NodeOutput::Err(err));
                                    continue;
                                }
                            }
                            let lifted = dest.alloc_object(lifted_obj);
                            self.finalize(idx, NodeOutput::Value(lifted));
                        }
                        (NodeOutput::Err(e), Some(_frame)) => {
                            let with_frame = match prev_function {
                                Some(f) => e.with_frame(Frame {
                                    function: f.summarize(),
                                    expression: f.summarize(),
                                }),
                                None => e,
                            };
                            self.finalize(idx, NodeOutput::Err(with_frame));
                        }
                        (other, None) => {
                            self.finalize(idx, other);
                        }
                    }
                }
                NodeStep::Replace { work: new_work, frame: new_frame, function: new_function } => {
                    let next_function = new_function.or(prev_function);
                    match new_frame {
                        Some(f) => {
                            // Drop the previous frame; the new frame's child scope's
                            // `outer` is the captured scope, not the previous frame's.
                            // `'a`-anchoring lives in `reinstall_with_frame`'s SAFETY.
                            drop(prev_frame);
                            self.store.reinstall_with_frame(id, f, new_work, next_function);
                        }
                        None => {
                            self.store.reinstall(id, Node {
                                work: new_work,
                                scope,
                                frame: prev_frame,
                                function: next_function,
                            });
                        }
                    }
                    // Replace return sites install their own edges before returning, or
                    // have nothing to install (Tail rewrites clear `dep_edges[idx]`
                    // beforehand), so `pending_count` is authoritative.
                    if self.deps.pending_count(idx) == 0 {
                        self.queues.push_after_replace(idx);
                    }
                }
            }
        }
        Ok(())
    }

    /// Each woken consumer whose work is `Lift(Pending(idx))` is stamped to
    /// `Lift(Ready(producer_output))` before enqueue, so the matching `run_lift` pop
    /// has the terminal in hand and never reads `results[idx]`.
    ///
    /// Invariant: every consumer drained here is parked with a non-zero counter; freed
    /// slots are scrubbed from every producer's `notify_list` before the producer drains.
    pub(super) fn finalize(&mut self, idx: usize, output: NodeOutput<'a>) {
        let id = NodeId(idx);
        self.store.finalize(id, output);
        let woken = self.deps.drain_notify(idx);
        for consumer in &woken {
            self.store.stamp_lift_ready(NodeId(*consumer), id);
        }
        for consumer in woken {
            self.queues.push_woken(consumer);
        }
    }

    /// Recurses only into `DepEdge::Owned` entries; `Notify` entries point at sibling
    /// producers this slot merely parked on, and reclaiming a consumer must not reach
    /// across a park edge into the producer's subtree.
    ///
    /// Idempotent and safe to call on a still-live slot.
    ///
    /// `&'a KObject` references handed out by `read` survive `free` because the value
    /// lives in an arena; clearing the slot's result only drops the enum wrapper.
    pub(super) fn free(&mut self, idx: usize) {
        let mut stack: Vec<NodeId> = vec![NodeId(idx)];
        while let Some(id) = stack.pop() {
            if self.store.is_live(id) { continue; }
            if self.store.is_reclaimed(id) && self.deps.is_dep_edges_empty(id.index()) {
                continue;
            }
            for child in self.deps.owned_children(id.index()) {
                stack.push(child);
            }
            self.store.free_one(id);
        }
    }

    /// Frame / function are left as `None` so the slot's existing per-call frame and
    /// function label stay attached when the Lift writes its terminal.
    ///
    /// `bind_id` is a fresh slot, so the producer-not-terminal precondition for
    /// `add_owned_edge` holds, and the Owned edge ensures `free`'s Owned-only recursion
    /// cascade-frees the underlying Bind/Combine. After a replay-park, `dep_edges[idx]`
    /// can take the mixed shape `[Notify(producer), …, Owned(bind_id)]`, which `free`
    /// handles correctly.
    pub(super) fn defer_to_lift(&mut self, idx: usize, bind_id: NodeId) -> NodeStep<'a> {
        self.deps.add_owned_edge(bind_id, NodeId(idx));
        NodeStep::Replace {
            work: NodeWork::Lift(LiftState::Pending(bind_id)),
            frame: None,
            function: None,
        }
    }
}
