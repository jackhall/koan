//! The Koan driver over the workload-independent [`Scheduler`](crate::scheduler::Scheduler): the
//! run loop ([`KoanRuntime::execute`]) pops ready slots and hands each to [`run_step`](KoanRuntime::run_step),
//! which brackets the step's ambient frame context end-to-end and applies the [`NodeStep`] it returns
//! through the scheduler's method contract. The scheduler stores and hands back opaque per-node state;
//! all Koan semantics — the per-call arena lift, the return-contract enforcement, the lexical-chain
//! assembly — live here.
//!
//! See design/execution-model.md and design/memory-model.md.

use crate::machine::core::kfunction::body::ErasedContract;
use crate::machine::model::Carried;
use crate::machine::{KError, KErrorKind, NodeId, RuntimeArena};

use super::dispatch::{reattach_node_scope, SchedulerView};
use super::finalize::NodeFinalize;
use super::nodes::{CallFrame, Node, NodePayload, NodeScope, NodeStep, NodeWork};
use super::outcome::deps_at_step;
use super::runtime::{KoanRuntime, KoanWorkload};
use super::NodeCont;

#[cfg(test)]
mod run_tests;
#[cfg(test)]
mod tests;

impl<'run> KoanRuntime<'run> {
    /// On `Done` with a frame, the return `Value` references the per-call arena that's
    /// about to drop, so it must be lifted into the captured scope's arena before the
    /// frame is released. See design/memory-model.md.
    pub fn execute(&mut self) -> Result<(), KError> {
        while let Some(idx) = self.sched.pop_next() {
            let id = NodeId(idx);
            let node = self.sched.take_for_run(id);
            self.run_step(id, node);
        }
        // Any slot still `PreRun` after drain is parked on a dependency that can
        // no longer fire — surface the cycle rather than panic on the caller's
        // top-level result read.
        if let Some((pending, sample)) = self.sched.unresolved() {
            return Err(KError::new(KErrorKind::SchedulerDeadlock {
                pending,
                sample,
            }));
        }
        Ok(())
    }

    /// The unified node handler, owning one slot step start to finish: enter the step's ambient frame
    /// context, collect the resolved dep terminals (as owned `Result`s — an errored dep is handed
    /// through, the continuation decides), run `cont` against a read-only [`SchedulerView`], reclaim
    /// the owned-dep suffix, apply the decided [`Outcome`] into a [`NodeStep`], exit the step guard,
    /// then realize that step. The continuation issues no graph write, so the reclaim lands after it
    /// and before the apply that installs the continuation's edges. The whole bracket — enter and
    /// exit — lives here, so the [`SlotStepGuard`](super::ambient::SlotStepGuard) is born and consumed
    /// without escaping.
    ///
    /// The whole step runs at the step lifetime `'s`: the cont re-anchor fabricates a `'run` witnessed
    /// by the cart `Rc`, which the step guard holds live across the continuation's run — and the body,
    /// including the [`NodeStep::Done`] terminal's finalize, runs while `consumer_frame` (the step's
    /// cart `Rc`, cloned into the sole `'s` witness) is live. So the Done value, born at `'s` in the
    /// consumer frame, is finalized into the slot store (where `finalize` erases it) *within* `'s`: it
    /// never has to be laundered to `'run` to cross a step-guard exit. The clone is confined to this
    /// call and dropped at return — before the next iteration's `try_reset_for_tail`, which resets a
    /// *different* (the prior step's) cart — so it does not contend with the TCO `Rc::get_mut`
    /// uniqueness gate.
    fn run_step(&mut self, id: NodeId, node: Node<KoanWorkload>) {
        let idx = id.index();
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
        // Re-anchor the slot's erased continuation against its own cart before that cart moves into
        // the step guard. The guard keeps the cart live across the whole step, so the fabricated
        // `'run` cannot outlive the continuation's captures (which live in the run arena or a strict
        // ancestor of the cart). Mirrors the contract re-anchor at the Done boundary — the same erase
        // / reattach discipline, generalized to the whole closure.
        // SAFETY: `cart` is the witness pinning the captures' home for the whole step; it is held
        // live below (moved into the step guard) across the continuation's run.
        let cont: NodeCont<'run> = unsafe { erased_cont.reattach() };
        let guard = self.enter_slot_step(
            cart,
            reserve,
            NodePayload {
                scope: node_scope,
                chain: prev_chain_carrier.clone(),
            },
        );
        // Expose to the dispatch step whether this slot is a tail call within an established contract
        // chain — a deferred-return FN dispatched here skips resolving its own return type (keep-first
        // discards it anyway).
        self.ambient.active_in_contract_chain = prev_contract.is_some();
        // Consumer-pull: lift each dep's terminal out of its producer frame into this consumer's
        // own scope arena, so the value dies with the consumer and the producer keeps no surviving
        // copy that would outlive its own dying frame. A frameless / run-arena terminal already
        // survives and is forwarded as-is.
        //
        // `dest` is the consumer *scope's* arena (the right arena even for a transparent USING
        // window, whose scope arena differs from the active frame's), re-anchored at the step
        // lifetime `'s` bounded by the cart `Rc` cloned into `consumer_frame` — not the run global.
        // `read_lifted` re-anchors each producer read to it.
        let consumer_frame = self.ambient.active_frame_ref().cloned();
        let dest: &RuntimeArena = {
            let payload = self
                .ambient
                .active_payload()
                .expect("a slot step installs the ambient payload");
            reattach_node_scope(&payload.scope, consumer_frame.as_ref()).arena
        };
        let results: Vec<Result<Carried<'_>, KError>> =
            deps.iter().map(|d| self.read_lifted(*d, dest)).collect();
        let owned_indices: Vec<usize> = deps[park_count..].iter().map(|d| d.index()).collect();
        // The pull-lifted values die with this consumer's frame; deliver them at that `'s`.
        let outcome = cont(
            &SchedulerView::new(&self.sched, &self.ambient),
            deps_at_step(&results),
            idx,
        );
        self.sched.reclaim_deps(idx, owned_indices);
        let step = self.apply_outcome(outcome, idx);
        // The post-step token owns the slot's frame at step end and is the *only* source of the
        // step scope (via `post.payload()`), so the wrong-frame read that ambient `active_frame`
        // allowed is unspellable here.
        let post = self.exit_slot_step(guard);
        self.ambient.active_in_contract_chain = false;
        // Drain re-entrant writes against the step scope (re-anchored at the workload boundary from
        // the slot's raw handle and the authoritative post-step frame).
        reattach_node_scope(&post.payload().scope, Some(&post.prev_frame)).drain_pending();
        match step {
            NodeStep::Done(output) => {
                let frame = (!post.prev_frame.non_dying()).then_some(&post.prev_frame);
                // Contract layer (the `NodeFinalize` workload hook): re-anchor the slot's erased
                // contract against `frame`, check the declared return, and — when the declared type
                // coarsens the value (e.g. `List<Number>` through `:(LIST OF Any)`) — re-tag it into
                // the contract's own home arena so the terminal survives to every consumer's
                // pull-lift and the top-level read even when the producer frame is reused/freed. A
                // non-coarsened terminal stays in the producer's own frame (pinned below); the
                // producer does **not** lift it at Done. The whole finalize runs at `'s`.
                let result = self.finalize_terminal(output, frame, prev_contract);
                if result.is_err() {
                    reattach_node_scope(&post.payload().scope, Some(&post.prev_frame))
                        .clear_placeholders_for_producer(id);
                }
                // Pin the producer's per-call frame in the slot's terminal: a dying frame is held
                // until the slot is freed (frame death Done->free), keeping the terminal readable
                // until every consumer has pulled it; a frameless / run-frame producer pins nothing
                // (its value already lives in the run arena). Hand the scheduler the live `'s`
                // terminal; it erases it for storage internally — severing `'s` before
                // `consumer_frame` drops at return.
                self.sched.finalize(idx, result, frame.cloned());
            }
            NodeStep::Replace {
                work: new_work,
                frame: new_frame,
                contract: new_contract,
                chain,
            } => {
                let prev_frame = post.prev_frame;
                let post_step_reserve = post.post_step_reserve;
                // Keep the **first** contract of a tail chain: once a contract is set, a nested tail
                // call does not overwrite it, so the chain checks the original caller's declared
                // return — not the tail-most callee's. Both contracts are already erased (the new one
                // by `apply_outcome`), so this is a plain keep-first with no narrowing here.
                let next_contract: Option<ErasedContract> = prev_contract.or(new_contract);
                // The frame the body runs in: a freshly installed cart, else the slot's current one
                // (a `FramePlacement::Inherit` FN-body re-enters the cart a prior `Continue` already
                // installed — the folded `invoke`). The `ChainOp` reads it (for an `AssembleBody`) to
                // walk the body scope's lexical chain.
                let body_frame: &crate::machine::core::CallArena =
                    new_frame.as_deref().unwrap_or(&prev_frame);
                let new_chain = chain.apply(prev_chain_carrier, body_frame);
                match new_frame {
                    Some(f) => {
                        // Rotate the ping-pong reserve: the post-step reserve is superseded by
                        // today's post-step frame (which we park as the new reserve).
                        drop(post_step_reserve);
                        // The non-dying run frame is not a reusable per-call arena; parking it as
                        // the ping-pong reserve would defer (and mis-time) a real frame's drop.
                        // Treat it as no reserve — the run scope is re-reached through the
                        // scheduler's `run_frame`, never a reset reserve.
                        let new_reserve = (!prev_frame.non_dying()).then_some(prev_frame);
                        // The tail-replace slot's scope is always this `f` cart's own child, so
                        // store a payload-less `NodeScope::Yoked` re-projected from the co-located
                        // cart at the read boundary — no persisted `&'run` to dangle across a TCO
                        // reset. This Yoked payload is the Koan workload's, built here off the
                        // scheduler.
                        self.sched.replace(
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
                        // A frameless Replace keeps the prior cart — an invoke reuses the reserve,
                        // never the active cart, so the slot's cart is always present.
                        self.sched.replace(
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
            }
            NodeStep::Alias(producer) => {
                // The slot spliced itself out as a bare-name forward: move its consumers onto
                // `producer` and alias it for reads. The slot is not re-queued; `producer`'s fire
                // wakes the moved consumers, and late parkers resolve the alias when they wire in.
                // See `scheduler::splice`.
                self.sched.splice_forward(id, producer);
            }
        }
    }
}


