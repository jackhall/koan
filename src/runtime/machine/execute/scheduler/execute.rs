use crate::runtime::machine::{Frame, KError, KErrorKind, NodeId};

use super::super::lift::lift_kobject;
use super::super::nodes::{LiftState, Node, NodeOutput, NodeStep, NodeWork};
use super::Scheduler;

impl<'a> Scheduler<'a> {
    /// Drain pending work via [`WorkQueues::pop_next`]: in-flight slots feed first,
    /// then fresh top-level dispatches in submission order.
    ///
    /// `NodeStep::Replace` is the tail-call path: the slot's work is rewritten in place and
    /// re-enqueued via [`WorkQueues::push_after_replace`]. `Replace { frame: Some(f) }`
    /// installs `f` on the slot and drops the previous frame; the new frame's scope
    /// becomes the slot's scope and its arena owns the per-call allocations.
    ///
    /// On `Done` with a frame: the return `Value` references memory in the per-call arena
    /// that's about to drop, so it must be lifted into the captured scope's arena before
    /// the frame is released. See design/memory-model.md.
    pub fn execute(&mut self) -> Result<(), KError> {
        while let Some(idx) = self.queues.pop_next() {
            let id = NodeId(idx);
            let node = self.store.take_for_run(id);
            let scope = node.scope;
            let work = node.work;
            let prev_frame = node.frame;
            let prev_function = node.function;
            // Expose the slot's frame to builtins via `SchedulerHandle::current_frame` for
            // the duration of this slot's run; restored on exit.
            let prev_active = self.active_frame.take();
            self.active_frame = prev_frame.clone();
            let step = match work {
                NodeWork::Dispatch(expr) => self.run_dispatch(expr, scope, idx)?,
                NodeWork::Bind { expr, subs } => self.run_bind(expr, subs, scope, idx)?,
                NodeWork::Combine { deps, finish } => self.run_combine(deps, finish, scope, idx),
                NodeWork::Lift(state) => NodeStep::Done(Self::run_lift(state)),
            };
            self.active_frame = prev_active;
            // Drain pending re-entrant writes while `scope` is still guaranteed live —
            // match arms below may drop the frame `scope` is anchored to. See
            // design/memory-model.md § Re-entrant `Scope::add`.
            scope.drain_pending();
            match step {
                NodeStep::Done(output) => {
                    match (output, prev_frame) {
                        (NodeOutput::Value(v), Some(frame)) => {
                            // Lift into the captured arena (per-call scope's `outer` by
                            // lexical scoping) before the frame drops. See
                            // design/memory-model.md.
                            let dest = scope
                                .outer
                                .expect("per-call scope must have an outer (its captured scope)")
                                .arena;
                            let lifted_obj = lift_kobject(v, &frame);
                            if let Some(f) = prev_function {
                                // Module-system functor-params Stage B: only run the lift-
                                // time slot check for `Resolved(_)` return types. `Deferred`
                                // returns route their per-call check through the Combine
                                // finish that joins the body's lifted value with the
                                // per-call elaboration's `KType` — the static carrier
                                // here can't see the per-call resolution. `ReturnType::
                                // matches_value` returns `true` for `Deferred(_)` to keep
                                // the structural surface consistent, but skipping the
                                // call entirely keeps the diagnostic from misattributing
                                // a body-internal mismatch.
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
                            // `frame` drops here; if the lifted value cloned an Rc the
                            // arena lives on, otherwise it frees.
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
                            // Fresh per-call frame: drop the previous one. Lexical scoping
                            // means the new frame's child scope's `outer` is the captured
                            // scope, not the previous frame's. The `'a`-anchoring of
                            // `f.scope()` lives inside `reinstall_with_frame` — see its
                            // SAFETY docstring.
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
                    // Replace return sites either install their own edges via
                    // `add_owned_edge` / `add_park_edge` before returning (run_dispatch
                    // bare-name and replay-park branches, defer_to_lift) or have nothing
                    // to install (BodyResult::Tail rewrites to a Dispatch whose
                    // work_owned_edges is empty, and reclaim_deps cleared dep_edges[idx]
                    // beforehand). So pending_count(idx) is authoritative here.
                    if self.deps.pending_count(idx) == 0 {
                        self.queues.push_after_replace(idx);
                    }
                }
            }
        }
        Ok(())
    }

    /// Terminal write + notify-walk for slot `idx`. The single entry point for
    /// landing a `NodeOutput` and waking parked consumers — pairs
    /// `NodeStore::finalize` with `DepGraph::drain_notify` so the two halves
    /// of the terminal step happen in one method body. Each woken consumer
    /// whose work is `Lift(Pending(idx))` is stamped to
    /// `Lift(Ready(producer_output))` before enqueue, so the matching
    /// `run_lift` pop has the terminal in hand and never reads
    /// `results[idx]`.
    ///
    /// Invariant: every consumer drained here is parked with a non-zero
    /// counter. Freed slots are scrubbed from every producer's `notify_list`
    /// before the producer drains (see the
    /// `freed_slot_does_not_appear_in_other_notify_lists` test).
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

    /// Reclaim slot `idx` and the sub-tree it owns. Walks `dep_edges` recursively but
    /// recurses only into `DepEdge::Owned` entries (via `DepGraph::owned_children`),
    /// invoking `NodeStore::free_one` per reclaimed index. `DepEdge::Notify` entries are
    /// dropped on the floor: they point at sibling producers this slot merely parked on,
    /// and reclaiming a consumer must not reach across a park edge into the producer's
    /// subtree.
    ///
    /// Idempotent and safe to call on a still-live slot: the guards early-continue when
    /// the slot is still live (`NodeStore::is_live`) or was already reclaimed
    /// (`NodeStore::is_reclaimed` paired with `DepGraph::is_dep_edges_empty`).
    ///
    /// `&'a KObject` references handed out by `read` survive `free` because the underlying
    /// value lives in an arena; clearing the slot's result only drops the enum wrapper.
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
    /// `bind_id` was just spawned by this slot's `run_dispatch`, so it lands in
    /// `dep_edges[idx]` as `Owned`: the Lift owns its underlying Bind/Combine and
    /// must cascade-free it. When a Dispatch slot first parked via replay-park and
    /// then re-dispatched here, the resulting `dep_edges[idx]` is the mixed shape
    /// `[Notify(producer), …, Owned(bind_id)]` — exactly the case `free`'s
    /// `Owned`-only recursion handles correctly.
    pub(super) fn defer_to_lift(&mut self, idx: usize, bind_id: NodeId) -> NodeStep<'a> {
        // `bind_id` was just spawned by this slot — fresh slot, terminal not yet
        // computed, so the producer-not-terminal precondition for `add_owned_edge`
        // holds. Atomic +1 across the three vectors closes the deferred-fixup gap.
        self.deps.add_owned_edge(bind_id, NodeId(idx));
        NodeStep::Replace {
            work: NodeWork::Lift(LiftState::Pending(bind_id)),
            frame: None,
            function: None,
        }
    }
}
