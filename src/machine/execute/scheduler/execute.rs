use std::rc::Rc;

use crate::machine::core::{assemble_body_chain, ScopeId};
use crate::machine::{Frame, KError, KErrorKind, KFunction, LexicalFrame, NodeId};

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
            let prev_chain_carrier = node.chain;
            // Install the slot's frame + chain + reserve via the guard.
            // `enter_slot_step` mem-replaces them in (no extra clones beyond the
            // one chain Rc bump for `active_chain`) and parks the previous values
            // inside the guard. Builtins read the active frame through
            // `SchedulerHandle::current_frame`; tail-reuse takes it via
            // `try_take_reusable_frame_for_tail`. `invoke_to_step_pinned`
            // consumes `active_reserve` when present, swapping the reserve into
            // `active_frame` so the resumed body's first tail-call reuses the
            // reserve's arena instead of allocating a fresh one. On exit,
            // `exit_slot_step` swaps the originals back and returns
            // `(post_step_frame, post_step_reserve)` — the frame is `Some(_)` if
            // the step left it intact, `None` if tail-reuse consumed it; the
            // reserve is whatever's left after the step's invoke (typically
            // `None` after a reserve-consuming invoke).
            let guard = self.enter_slot_step(
                node.frame,
                node.reserve_frame,
                prev_chain_carrier.clone(),
            );
            let step = match work {
                NodeWork::Dispatch { expr, state } => {
                    self.run_dispatch(expr, state, scope, idx)?
                }
                NodeWork::Bind { expr, subs } => self.run_bind(expr, subs, scope, idx)?,
                NodeWork::Combine { deps, park_count, finish } => {
                    self.run_combine(deps, park_count, finish, scope, idx)
                }
                NodeWork::Catch { from, finish } => self.run_catch(from, finish, scope, idx),
                NodeWork::Lift(state) => NodeStep::Done(Self::run_lift(state)),
            };
            let (prev_frame, post_step_reserve) = self.exit_slot_step(guard);
            let prev_chain = prev_chain_carrier;
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
                            let mut lifted_obj = lift_kobject(v, &frame);
                            if let Some(f) = prev_function {
                                // Only run the lift-time return-type check for `Resolved`
                                // types. `Deferred` returns route their per-call check
                                // through the Combine finish that joins the lifted body
                                // value with the per-call elaboration's `KType`; the
                                // static carrier here can't see that resolution, and
                                // skipping it avoids misattributing a body-internal
                                // mismatch.
                                let rt = &f.signature.return_type;
                                if let crate::machine::model::types::ReturnType::Resolved(declared) =
                                    rt
                                {
                                    if !declared.matches_value(&lifted_obj) {
                                        let err = KError::new(KErrorKind::TypeMismatch {
                                            arg: "<return>".to_string(),
                                            expected: rt.name(),
                                            got: lifted_obj.ktype().name(),
                                        })
                                        .with_frame(Frame::bare(f.summarize(), f.summarize()));
                                        self.finalize(idx, NodeOutput::Err(err));
                                        continue;
                                    }
                                    // Phase 3 ascription stamping: re-tag the parameterized
                                    // carrier to exactly the declared return type so
                                    // downstream dispatch sees the contract, coarsening
                                    // included (`List<Number>` body through `:(List Any)`
                                    // re-tags to `List<Any>`).
                                    lifted_obj = lifted_obj.stamp_type(declared);
                                }
                            }
                            let lifted = dest.alloc(lifted_obj);
                            self.finalize(idx, NodeOutput::Value(lifted));
                        }
                        (NodeOutput::Err(e), Some(_frame)) => {
                            let with_frame = match prev_function {
                                Some(f) => e.with_frame(Frame::bare(f.summarize(), f.summarize())),
                                None => e,
                            };
                            self.finalize(idx, NodeOutput::Err(with_frame));
                        }
                        (other, None) => {
                            self.finalize(idx, other);
                        }
                    }
                }
                NodeStep::Replace {
                    work: new_work,
                    frame: new_frame,
                    function: new_function,
                    block_entry,
                    body_index,
                } => {
                    let next_function = new_function.or(prev_function);
                    let new_chain = compute_replace_chain(
                        prev_chain.clone(),
                        block_entry,
                        new_function,
                        new_frame.as_deref(),
                        body_index,
                    );
                    match new_frame {
                        Some(f) => {
                            // Rotate the ping-pong reserve. The post-step reserve
                            // (if any) is two iterations old by construction — its
                            // protector chain is long-gone, but it is also
                            // superseded by the post-step frame we're about to
                            // park, so drop it. Rotate `prev_frame` (today's
                            // post-step frame) into the new reserve; it may be
                            // `None` if the step's invoke took it via tail-reuse
                            // (the reserve will re-warm at the next new-frame
                            // Replace). The new frame `f` becomes `slot.frame`
                            // and its child scope's `outer` is the captured
                            // scope, not the previous frame's. `'a`-anchoring
                            // lives in `reinstall_with_frame`'s SAFETY.
                            drop(post_step_reserve);
                            let new_reserve = prev_frame;
                            self.store.reinstall_with_frame(
                                id,
                                f,
                                new_reserve,
                                new_work,
                                next_function,
                                new_chain,
                            );
                        }
                        None => {
                            // No new frame: the step rewrote work in place and
                            // the slot keeps its existing frame. Carry the
                            // (possibly-`None`) post-step reserve through so
                            // it's still available to the next iteration's
                            // invoke. This is the cross-shape Replace arm where
                            // a stateful Dispatch tail-replaces into a fresh
                            // Initialized Dispatch — the reserve survives until
                            // it's either consumed or rotated.
                            self.store.reinstall(id, Node {
                                work: new_work,
                                scope,
                                frame: prev_frame,
                                reserve_frame: post_step_reserve,
                                function: next_function,
                                chain: new_chain,
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
        // The queues drained. Any slot still `PreRun` is parked on a dependency that
        // can no longer fire — a cycle. Surface it cleanly rather than letting the
        // caller's top-level result read panic on the unresolved slot.
        if let Some((pending, sample)) = self.store.unresolved() {
            return Err(KError::new(KErrorKind::SchedulerDeadlock { pending, sample }));
        }
        Ok(())
    }

    /// Drains the producer's notify list and fans out per consumer:
    ///
    /// 1. Always push the producer into the consumer's `recent_wakes`
    ///    side-channel (a no-op unless the consumer is a `PreRun`
    ///    `NodeWork::Dispatch` — see `NodeStore::push_recent_wake`).
    ///    Step 2 of `roadmap/dispatch_fix/stateful-dispatch-02-recent-wakes.md`
    ///    feeds the wake signal so the stateful dispatch driver can
    ///    pick per-edge callbacks; step 3+ consumes it.
    /// 2. When the consumer's `pending_deps` hit zero, stamp a pending
    ///    `Lift` to `Ready(_)` (cloning the producer's terminal) and
    ///    enqueue the consumer. The two-pass stamp-then-push split is
    ///    preserved so every stamp lands before any queue push (a
    ///    later stamp re-reading the slot must observe the prior
    ///    transition).
    ///
    /// Invariant: every consumer drained here is parked with a non-zero
    /// counter; freed slots are scrubbed from every producer's
    /// `notify_list` before the producer drains.
    pub(super) fn finalize(&mut self, idx: usize, output: NodeOutput<'a>) {
        let id = NodeId(idx);
        self.store.finalize(id, output);
        let drained = self.deps.drain_notify(idx);
        let mut woken: Vec<usize> = Vec::new();
        for (consumer, hit_zero) in drained {
            // Side-channel append fires for every drained consumer; the
            // `NodeStore` discriminator filters out non-Dispatch slots
            // so the bookkeeping stays scoped to consumers that actually
            // need per-edge wake attribution.
            self.store.push_recent_wake(NodeId(consumer), id);
            if hit_zero {
                self.store.stamp_lift_ready(NodeId(consumer), id);
                woken.push(consumer);
            }
        }
        // Queue push happens strictly after every stamp has landed,
        // matching the previous two-loop shape.
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

    /// Frame / function are left as `None` and `block_entry: None` so the slot's
    /// existing per-call frame, function label, and chain stay attached when the
    /// Lift writes its terminal.
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
            block_entry: None,
            body_index: 0,
        }
    }
}

/// Compute the chain for a `NodeStep::Replace`. Cases by `block_entry` /
/// `new_function`:
///
/// 1. `block_entry: None` — TCO continuation in the same lexical block. Keep
///    `prev_chain` unchanged (FN-body tail-recursion, builtin tail continuations).
/// 2. `block_entry: Some(scope_id)` + `new_function: None` — block-entry without a
///    new FN body (MATCH arm, TRY arm). Prepend `(scope_id, body_index)` to
///    `prev_chain` — `body_index = 0` for single-statement arm bodies, `N` for
///    the last-stmt tail-replace path of a multi-statement arm body.
/// 3. `block_entry: Some(body_scope_id)` + `new_function: Some(_)` — FN body
///    invoke. The new body's chain is assembled from the FN's lexical `outer`
///    walk so chain depth tracks lexical nesting, not call depth (tail-recursive
///    loops produce equal-depth chains each iteration). `body_index` positions
///    the freshly-pushed body-scope frame: `0` for single-statement bodies, `N`
///    for the multi-statement tail-into-last path.
fn compute_replace_chain<'a>(
    prev_chain: Rc<LexicalFrame>,
    block_entry: Option<ScopeId>,
    new_function: Option<&'a KFunction<'a>>,
    new_frame: Option<&crate::machine::core::CallArena>,
    body_index: usize,
) -> Rc<LexicalFrame> {
    let Some(scope_id) = block_entry else {
        return prev_chain;
    };
    match (new_function, new_frame) {
        (Some(_f), Some(frame)) => assemble_body_chain(frame.scope(), prev_chain, body_index),
        _ => LexicalFrame::push(Some(prev_chain), scope_id, body_index),
    }
}
