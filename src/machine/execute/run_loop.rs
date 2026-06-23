//! The Koan driver over the workload-independent [`Scheduler`](crate::scheduler::Scheduler): the
//! run loop ([`KoanRuntime::execute`]) pops ready slots and hands each to [`run_step`](KoanRuntime::run_step),
//! which brackets the step's ambient frame context end-to-end and applies the [`NodeStep`] it returns
//! through the scheduler's method contract. The scheduler stores and hands back opaque per-node state;
//! all Koan semantics — the per-call region lift, the return-contract enforcement, the lexical-chain
//! assembly — live here.
//!
//! See design/execution/README.md and design/memory-model.md.

use std::rc::Rc;

use crate::machine::core::kfunction::body::{ErasedContract, ReturnContract};
use crate::machine::model::Carried;
use crate::machine::{KError, KErrorKind, KoanRegion, NodeId};
use crate::scheduler::vend_carrier;

use super::dispatch::{reattach_node_scope, SchedulerView};
use super::finalize::NodeFinalize;
use super::nodes::{Node, NodeFrame, NodePayload, NodeScope, NodeStep, NodeWork};
use super::outcome::deps_at_step;
use super::runtime::{KoanRuntime, KoanWorkload};
use super::NodeContinuation;

#[cfg(test)]
mod run_tests;
#[cfg(test)]
mod tests;

impl<'run> KoanRuntime<'run> {
    /// On `Done` with a frame, the return `Value` references the per-call region that's
    /// about to drop, so it must be lifted into the captured scope's region before the
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
    /// through, the continuation decides), run the slot's continuation against a read-only
    /// [`SchedulerView`], reclaim the owned-dep suffix, apply the decided [`Outcome`] into a
    /// [`NodeStep`], exit the step guard, then realize that step. The continuation issues no graph
    /// write, so the reclaim lands after it and before the apply that installs the continuation's
    /// edges. The whole bracket — enter and exit — lives here, so the
    /// [`SlotStepGuard`](super::ambient::SlotStepGuard) is born and consumed without escaping.
    ///
    /// The whole step runs at the step lifetime `'s`: the scheduler's `vend_carrier` re-anchors the
    /// erased continuation to `'s` witnessed by the held cart `Rc` (`continuation_witness`), which the
    /// step guard keeps live across the continuation's run — and the body, including the
    /// [`NodeStep::Done`] terminal's finalize, runs while `consumer_frame` (also the step's cart `Rc`)
    /// witnesses the value-channel pull-lift. So the Done value, born at `'s` in the consumer frame, is
    /// finalized into the slot store (where `finalize` erases it) *within* `'s`: it never has to be
    /// laundered to `'run` to cross a step-guard exit. Both cart clones are confined to this call and
    /// dropped at return — before the next iteration's `try_reset_for_tail`, which resets a *different*
    /// (the prior step's) cart — so they do not contend with the TCO `Rc::get_mut` uniqueness gate.
    fn run_step(&mut self, id: NodeId, node: Node<KoanWorkload>) {
        let idx = id.index();
        // The step reads its scope on demand (`current_scope`), and the post-step uses below
        // re-acquire it per use, so nothing holds a scope borrow across the step's `&mut self`
        // work or the in-step TCO frame reset.
        let node_scope = node.payload.scope;
        let NodeFrame {
            cart,
            reserve,
            contract: prev_contract,
        } = node.frame;
        let prev_chain_carrier = node.payload.chain;
        let NodeWork {
            deps,
            park_count,
            continuation: erased_continuation,
            carrier: _,
        } = node.work;
        // Hold the cart as the continuation's reattach witness across the whole step: a step-confined
        // clone, dropped at return — before the next iteration's `try_reset_for_tail`, which resets a
        // *different* (the prior step's) cart — so it does not contend with the TCO `Rc::get_mut`
        // gate. `vend_continuation` (below) re-anchors the erased continuation to the step lifetime
        // this witness pins; the scheduler owns that reattach, so no fabricated free `'_` lives here.
        let continuation_witness = Rc::clone(&cart);
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
        // own scope region, so the value dies with the consumer and the producer keeps no surviving
        // copy that would outlive its own dying frame. A frameless / run-region terminal already
        // survives and is forwarded as-is.
        //
        // `dest` is the consumer *scope's* region (the right region even for a transparent USING
        // window, whose scope region differs from the active frame's), re-anchored at the step
        // lifetime `'s` bounded by the cart `Rc` cloned into `consumer_frame` — not the run global.
        // `read_lifted` re-anchors each producer read to it.
        let consumer_frame = self.ambient.active_frame_ref().cloned();
        let dest: &KoanRegion = {
            let payload = self
                .ambient
                .active_payload()
                .expect("a slot step installs the ambient payload");
            reattach_node_scope(&payload.scope, consumer_frame.as_ref()).region
        };
        let results: Vec<Result<Carried<'_>, KError>> =
            deps.iter().map(|d| self.read_lifted(*d, dest)).collect();
        let owned_indices: Vec<usize> = deps[park_count..].iter().map(|d| d.index()).collect();
        // Vend the slot's continuation re-anchored to the step lifetime the held `continuation_witness`
        // cart pins (cart-scale data in the cart region or a strict ancestor its `outer` chain pins).
        // The scheduler owns the `unsafe` reattach inside `vend_carrier`; its safe signature bounds the
        // step lifetime to the witness borrow, so no fabricated free `'_` or `unsafe` lives here.
        let continuation: NodeContinuation<'_> =
            vend_carrier(erased_continuation, &continuation_witness);
        // The pull-lifted values die with this consumer's frame; deliver them at that `'s`.
        let outcome = continuation(
            &SchedulerView::new(&self.sched, &self.ambient),
            deps_at_step(&results, &continuation_witness),
            idx,
        );
        self.sched.reclaim_deps(idx, owned_indices);
        let step = self.apply_outcome(outcome, idx, &continuation_witness);
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
                // Vend the slot's erased contract re-anchored to the step lifetime the producer cart
                // `frame` witnesses — the scheduler owns the `unsafe` reattach inside `vend_carrier`.
                // `zip(frame)` drops the contract for a frameless / run-frame producer (which carries
                // no per-call return obligation), matching the prior frame-gated reattach.
                let live_contract: Option<ReturnContract<'_>> = prev_contract
                    .zip(frame)
                    .map(|(c, witness)| vend_carrier(c, witness));
                // Contract layer (the `NodeFinalize` workload hook): check the declared return and —
                // when the declared type coarsens the value (e.g. `List<Number>` through
                // `:(LIST OF Any)`) — re-tag it into the contract's own home region so the terminal
                // survives to every consumer's pull-lift and the top-level read even when the producer
                // frame is reused/freed. A non-coarsened terminal stays in the producer's own frame
                // (pinned below); the producer does **not** lift it at Done. The whole finalize runs
                // at `'s`.
                let result = self.finalize_terminal(output, frame, live_contract);
                if result.is_err() {
                    reattach_node_scope(&post.payload().scope, Some(&post.prev_frame))
                        .clear_placeholders_for_producer(id);
                }
                // Pin the producer's per-call frame in the slot's terminal: a dying frame is held
                // until the slot is freed (frame death Done->free), keeping the terminal readable
                // until every consumer has pulled it; a frameless / run-frame producer pins nothing
                // (its value already lives in the run region). Hand the scheduler the live `'s`
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
                let body_frame: &crate::machine::core::CallFrame =
                    new_frame.as_deref().unwrap_or(&prev_frame);
                let new_chain = chain.apply(prev_chain_carrier, body_frame);
                match new_frame {
                    Some(f) => {
                        // Rotate the ping-pong reserve: the post-step reserve is superseded by
                        // today's post-step frame (which we park as the new reserve).
                        drop(post_step_reserve);
                        // The non-dying run frame is not a reusable per-call region; parking it as
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
                                frame: NodeFrame {
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
                                frame: NodeFrame {
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
