use std::rc::Rc;

use crate::machine::core::kfunction::body::{ErasedContract, ReturnContract};
use crate::machine::core::{assemble_body_chain, ScopeId};
use crate::machine::{KError, KErrorKind, LexicalFrame, NodeId};

use super::super::dispatch::reattach_node_scope;
use super::super::finalize::NodeFinalize;
use super::super::nodes::{CallFrame, Node, NodePayload, NodeScope, NodeStep, NodeWork};
use super::super::{ErasedValue, NodeCont};
use super::super::runtime::KoanRuntime;
use super::Scheduler;

impl<'run> KoanRuntime<'run> {
    /// On `Done` with a frame, the return `Value` references the per-call arena that's
    /// about to drop, so it must be lifted into the captured scope's arena before the
    /// frame is released. See design/memory-model.md.
    pub fn execute(&mut self) -> Result<(), KError> {
        while let Some(idx) = self.sched.queues.pop_next() {
            let id = NodeId(idx);
            let node = self.sched.store.take_for_run(id);
            // The step reads its scope on demand (`current_scope`), and the post-step uses below
            // re-acquire it per use, so nothing holds a scope borrow across the step's `&mut self`
            // work or the in-step TCO frame reset.
            let node_scope = node.payload.scope;
            let CallFrame {
                cart,
                reserve,
                contract: prev_contract,
            } = node.frame;
            let prev_chain_carrier = node.payload.chain;
            let NodeWork {
                deps,
                park_count,
                cont: erased_cont,
                carrier: _,
            } = node.work;
            // Re-anchor the slot's erased continuation against its own cart before that cart moves
            // into the step guard. The guard keeps the cart live across `run_wait`, so the
            // fabricated `'run` cannot outlive the continuation's captures (which live in the run
            // arena or a strict ancestor of the cart). Mirrors the contract re-anchor at the Done
            // boundary — the same erase / reattach discipline, generalized to the whole closure.
            // SAFETY: `cart` is the witness pinning the captures' home for the whole step.
            let cont: NodeCont<'run> = unsafe { erased_cont.reattach(&cart) };
            let guard = self.sched.enter_slot_step(
                cart,
                reserve,
                NodePayload {
                    scope: node_scope,
                    chain: prev_chain_carrier.clone(),
                },
            );
            // Expose to the dispatch step whether this slot is a tail call within an established
            // contract chain — a deferred-return FN dispatched here skips resolving its own return
            // type (keep-first discards it anyway).
            self.sched.active_in_contract_chain = prev_contract.is_some();
            let step = self.run_wait(deps, park_count, cont, idx);
            // The post-step token owns the slot's frame at step end and is the *only* source of
            // the step scope (via `post.step_scope()`), so the wrong-frame read that ambient
            // `active_frame` allowed is unspellable here.
            let post = self.sched.exit_slot_step(guard);
            self.sched.active_in_contract_chain = false;
            // Drain re-entrant writes against the step scope (re-anchored at the workload boundary
            // from the slot's raw handle and the authoritative post-step frame).
            reattach_node_scope(&post.payload().scope, Some(&post.prev_frame)).drain_pending();
            match step {
                NodeStep::Done(output) => {
                    let frame = (!post.prev_frame.non_dying()).then_some(&post.prev_frame);
                    // Contract layer (the `NodeFinalize` workload hook): re-anchor the slot's erased
                    // contract against `frame`, check the declared return, and — when the declared
                    // type coarsens the value (e.g. `List<Number>` through `:(LIST OF Any)`) —
                    // re-tag it into the contract's own home arena so the terminal survives to every
                    // consumer's pull-lift and the top-level read even when the producer frame is
                    // reused/freed. A non-coarsened terminal stays in the producer's own frame
                    // (pinned below); the producer does **not** lift it at Done.
                    let result = self.finalize_terminal(output, frame, prev_contract);
                    if result.is_err() {
                        reattach_node_scope(&post.payload().scope, Some(&post.prev_frame))
                            .clear_placeholders_for_producer(id);
                    }
                    // Pin the producer's per-call frame in the slot's terminal: a dying frame is
                    // held until the slot is freed (frame death Done->free), keeping the terminal
                    // readable until every consumer has pulled it; a frameless / run-frame producer
                    // pins nothing (its value already lives in the run arena).
                    self.sched
                        .finalize(idx, result.map(ErasedValue::erase), frame.cloned());
                }
                NodeStep::Replace {
                    work: new_work,
                    frame: new_frame,
                    function: new_function,
                    block_entry,
                    body_index,
                } => {
                    let prev_frame = post.prev_frame;
                    let post_step_reserve = post.post_step_reserve;
                    // Keep the **first** contract of a tail chain: once a contract is set, a nested
                    // tail call does not overwrite it, so the chain checks the original caller's
                    // declared return — not the tail-most callee's. `compute_replace_chain` reads
                    // `new_function` (still live) for the chain-shape decision before erasure.
                    let next_contract: Option<ErasedContract> =
                        prev_contract.or_else(|| new_function.map(ErasedContract::erase));
                    // The frame the body runs in: a freshly installed cart, else the slot's current
                    // one (a `FramePlacement::Inherit` FN-body re-enters the cart a prior `Continue`
                    // already installed — the folded `invoke`).
                    let body_frame: &crate::machine::core::CallArena =
                        new_frame.as_deref().unwrap_or(&prev_frame);
                    let new_chain = compute_replace_chain(
                        prev_chain_carrier,
                        block_entry,
                        new_function,
                        body_frame,
                        body_index,
                    );
                    match new_frame {
                        Some(f) => {
                            // Rotate the ping-pong reserve: the post-step reserve is
                            // superseded by today's post-step frame (which we park as
                            // the new reserve). `'run`-anchoring lives in
                            // `reinstall_with_frame`'s SAFETY.
                            drop(post_step_reserve);
                            // The non-dying run frame is not a reusable per-call arena; parking
                            // it as the ping-pong reserve would defer (and mis-time) a real
                            // frame's drop. Treat it as no reserve — the run scope is re-reached
                            // through the scheduler's `run_frame`, never a reset reserve.
                            let new_reserve = (!prev_frame.non_dying()).then_some(prev_frame);
                            // The tail-replace slot's scope is always this `f` cart's own child, so
                            // store a payload-less `NodeScope::Yoked` re-projected from the co-located
                            // cart at the read boundary — no persisted `&'run` to dangle across a TCO
                            // reset. This Yoked payload is the Koan workload's, built here off the
                            // scheduler.
                            self.sched.store.reinstall_with_frame(
                                id,
                                Node {
                                    work: new_work,
                                    payload: NodePayload {
                                        scope: NodeScope::Yoked,
                                        chain: new_chain,
                                    },
                                    frame: CallFrame {
                                        cart: f,
                                        reserve: new_reserve,
                                        contract: next_contract,
                                    },
                                },
                            );
                        }
                        None => {
                            // A frameless Replace keeps the prior cart — an invoke reuses the
                            // reserve, never the active cart, so the slot's cart is always present.
                            self.sched.store.reinstall(
                                id,
                                Node {
                                    work: new_work,
                                    payload: NodePayload {
                                        scope: node_scope,
                                        chain: new_chain,
                                    },
                                    frame: CallFrame {
                                        cart: prev_frame,
                                        reserve: post_step_reserve,
                                        contract: next_contract,
                                    },
                                },
                            );
                        }
                    }
                    // Replace return sites install their own edges (or clear
                    // `dep_edges[idx]` for tail rewrites), so `pending_count` is
                    // authoritative here.
                    if self.sched.deps.pending_count(idx) == 0 {
                        self.sched.queues.push_after_replace(idx);
                    }
                }
                NodeStep::Alias(producer) => {
                    // The slot spliced itself out as a bare-name forward: move its consumers onto
                    // `producer` and alias it for reads. The slot is not re-queued; `producer`'s
                    // fire wakes the moved consumers, and late parkers resolve the alias when they
                    // wire in. See `scheduler::splice`.
                    self.sched.splice_forward(id, producer);
                }
            }
        }
        // Any slot still `PreRun` after drain is parked on a dependency that can
        // no longer fire — surface the cycle rather than panic on the caller's
        // top-level result read.
        if let Some((pending, sample)) = self.sched.store.unresolved() {
            return Err(KError::new(KErrorKind::SchedulerDeadlock {
                pending,
                sample,
            }));
        }
        Ok(())
    }
}

impl<P, V: Copy> Scheduler<P, V> {
    /// Invariant: every consumer drained here is parked with a non-zero counter;
    /// freed slots are scrubbed from every producer's `notify_list` before the
    /// producer drains.
    ///
    /// Wakes must all land before any queue push: a later wake re-reading the
    /// slot must observe the prior transition.
    pub(in crate::machine::execute::scheduler) fn finalize(
        &mut self,
        idx: usize,
        output: Result<V, KError>,
        frame: Option<Rc<crate::machine::core::CallArena>>,
    ) {
        let id = NodeId(idx);
        self.store.finalize(id, output, frame);
        let drained = self.deps.drain_notify(idx);
        let mut woken: Vec<usize> = Vec::new();
        for (consumer, hit_zero) in drained {
            if hit_zero {
                woken.push(consumer);
            }
        }
        for consumer in woken {
            self.queues.push_woken(consumer);
        }
    }

    /// Recurses only into `DepEdge::Owned` entries; `Notify` entries point at sibling
    /// producers this slot merely parked on, and reclaiming a consumer must not reach
    /// across a park edge into the producer's subtree.
    ///
    /// Idempotent and safe to call on a still-live slot. `&'run KObject` references
    /// handed out by `read` survive because the value lives in an arena.
    pub(in crate::machine::execute) fn free(&mut self, idx: usize) {
        let mut stack: Vec<NodeId> = vec![NodeId(idx)];
        while let Some(id) = stack.pop() {
            if self.store.is_live(id) {
                continue;
            }
            if self.store.is_reclaimed(id) && self.deps.is_dep_edges_empty(id.index()) {
                continue;
            }
            for child in self.deps.owned_children(id.index()) {
                stack.push(child);
            }
            self.store.free_one(id);
        }
    }
}

/// Cases by `block_entry` / `new_function`:
///
/// - `None` — TCO in the same lexical block; chain unchanged.
/// - `Some(scope_id)` + non-`Function` contract — block-entry arm (MATCH, TRY); prepend.
/// - `Some(_)` + `Function`/`PerCall` contract — FN body invoke (a deferred FN body for
///   `PerCall`). Chain is assembled from the FN's lexical `outer` walk so depth tracks lexical
///   nesting, not call depth (tail-recursive loops produce equal-depth chains each iteration).
///
/// `body_frame` is the cart the body runs in — the freshly installed frame for a
/// `FreshChild`/`ReuseReserve` tail, or the slot's already-installed current cart for an `Inherit`
/// FN-body re-entry (the folded `invoke`). The body-chain decision keys off the **contract kind**,
/// not whether a new frame was minted, so an `Inherit` FN body assembles against the current cart
/// exactly as a `FreshChild` one assembles against the minted cart.
fn compute_replace_chain<'run>(
    prev_chain: Rc<LexicalFrame>,
    block_entry: Option<ScopeId>,
    new_function: Option<ReturnContract<'run>>,
    body_frame: &crate::machine::core::CallArena,
    body_index: usize,
) -> Rc<LexicalFrame> {
    let Some(scope_id) = block_entry else {
        return prev_chain;
    };
    match new_function {
        // `Function` and `PerCall` (a deferred FN body) both assemble the FN-body chain.
        Some(ReturnContract::Function(_) | ReturnContract::PerCall { .. }) => {
            assemble_body_chain(body_frame.scope(), prev_chain, body_index)
        }
        _ => LexicalFrame::push(Some(prev_chain), scope_id, body_index),
    }
}
