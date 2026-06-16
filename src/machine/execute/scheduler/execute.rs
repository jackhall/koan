use std::rc::Rc;

use crate::machine::core::kfunction::body::{ErasedContract, ReturnContract};
use crate::machine::core::{assemble_body_chain, ScopeId};
use crate::machine::model::KType;
use crate::machine::{KError, KErrorKind, LexicalFrame, NodeId};

use super::super::lift::NodeLift;
use super::super::nodes::{CallFrame, Node, NodeStep, NodeWork};
use super::super::runtime::KoanRuntime;
use super::Scheduler;
use crate::machine::model::Carried;

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
            let node_scope = node.scope;
            let work = node.work;
            let CallFrame {
                cart,
                reserve,
                contract: prev_contract,
            } = node.frame;
            let prev_chain_carrier = node.chain;
            let guard =
                self.sched
                    .enter_slot_step(cart, reserve, prev_chain_carrier.clone(), node_scope);
            // Expose to the dispatch step whether this slot is a tail call within an established
            // contract chain — a deferred-return FN dispatched here skips resolving its own return
            // type (keep-first discards it anyway).
            self.sched.active_in_contract_chain = prev_contract.is_some();
            let NodeWork {
                deps,
                park_count,
                cont,
                ..
            } = work;
            let step = self.run_wait(deps, park_count, cont, idx);
            // The post-step token owns the slot's frame at step end and is the *only* source of
            // the step scope (via `post.step_scope()`), so the wrong-frame read that ambient
            // `active_frame` allowed is unspellable here.
            let post = self.sched.exit_slot_step(guard);
            self.sched.active_in_contract_chain = false;
            // Drain re-entrant writes against the step scope.
            post.step_scope().drain_pending();
            match step {
                NodeStep::Done(output) => {
                    let frame = (!post.prev_frame.non_dying()).then_some(&post.prev_frame);
                    // Re-anchor the erased contract against the step's cart, witnessed by `frame`.
                    // The contract layer consults it only when `frame` is `Some` (a real per-call
                    // frame, which is exactly when a contract is set), so a contract on the
                    // non-dying run frame is harmlessly skipped.
                    let prev_function = match (prev_contract, frame) {
                        (Some(c), Some(witness)) => Some(unsafe { c.reattach(witness) }),
                        _ => None,
                    };
                    // Contract layer: check the declared return and stamp the declared type, in
                    // place in the producer frame's own arena (`producer_arena`, where `output`
                    // already lives). No lift — kept separate from the relocation below.
                    let producer_arena = post.step_scope().arena;
                    let checked =
                        enforce_return_contract(output, frame, prev_function, producer_arena);
                    // Lift layer: relocate the now-final producer-frame terminal into the surviving
                    // captured-scope arena (`dest_arena`, a genuine `&'run`) via the workload hook.
                    // A non-dying run frame (empty arena; top-level values live in the run arena)
                    // reads as frameless and passes through untouched.
                    let result = match (checked, frame) {
                        (Ok(c), Some(src)) => {
                            let dest =
                                post.step_scope().outer().map(|o| o.arena).expect(
                                    "per-call scope must have an outer (its captured scope)",
                                );
                            Ok(self.lift(c, src, dest))
                        }
                        (other, _) => other,
                    };
                    if result.is_err() {
                        post.step_scope().clear_placeholders_for_producer(id);
                    }
                    // Pin the producer's per-call frame in the slot's terminal: a dying frame is
                    // held until the slot is freed (frame death Done->free); a frameless / run-frame
                    // producer pins nothing.
                    self.sched.finalize(idx, result, frame.cloned());
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
                            self.sched.store.reinstall_with_frame(
                                id,
                                f,
                                new_reserve,
                                new_work,
                                next_contract,
                                new_chain,
                            );
                        }
                        None => {
                            // A frameless Replace keeps the prior cart — an invoke reuses the
                            // reserve, never the active cart, so the slot's cart is always present.
                            self.sched.store.reinstall(
                                id,
                                Node {
                                    work: new_work,
                                    scope: node_scope,
                                    frame: CallFrame {
                                        cart: prev_frame,
                                        reserve: post_step_reserve,
                                        contract: next_contract,
                                    },
                                    chain: new_chain,
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

impl<'run> Scheduler<'run> {
    /// Invariant: every consumer drained here is parked with a non-zero counter;
    /// freed slots are scrubbed from every producer's `notify_list` before the
    /// producer drains.
    ///
    /// Wakes must all land before any queue push: a later wake re-reading the
    /// slot must observe the prior transition.
    pub(in crate::machine::execute::scheduler) fn finalize(
        &mut self,
        idx: usize,
        output: Result<Carried<'run>, KError>,
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

/// Enforce a `Done` step's declared return contract, in place in the producer frame, returning the
/// terminal still bound to that frame (the relocation into the captured-scope arena is a separate
/// step — the `lift` hook — run by the caller). A `None` frame (a frameless slot or the non-dying
/// run frame) passes the value through untouched. A failed return-type check becomes `Err` — the
/// caller clears placeholders and finalizes. `producer_arena` is the producer frame's own arena,
/// where a re-tagged `Object` is re-allocated in place. Pure: the scope-derived inputs were
/// captured by the caller while the step's scope was still ambient, so this holds no scope borrow.
fn enforce_return_contract<'run>(
    output: Result<Carried<'run>, KError>,
    frame: Option<&Rc<crate::machine::core::CallArena>>,
    prev_function: Option<ReturnContract<'run>>,
    producer_arena: &'run crate::machine::core::RuntimeArena,
) -> Result<Carried<'run>, KError> {
    match (output, frame) {
        (Ok(Carried::Object(v)), Some(_)) => {
            match check_declared_return(prev_function, |d| d.matches_value(v), || v.ktype().name())?
            {
                // Re-tag to the declared return type so downstream dispatch sees the contract
                // (may coarsen, e.g. `List<Number>` through `:(LIST OF Any)` -> `List<Any>`). The
                // re-tag is a shallow rebuild re-allocated in place in the producer frame.
                Some(declared) => {
                    let stamped = v.deep_clone().stamp_type(declared);
                    Ok(Carried::Object(producer_arena.alloc_object(stamped)))
                }
                None => Ok(Carried::Object(v)),
            }
        }
        // A type flowing the type channel runs the shared declared-return check via `matches_type`.
        // The type channel ignores the returned declared type — unlike the `Object` arm, it does
        // not re-tag — so the in-frame value passes through unchanged.
        (Ok(Carried::Type(t)), Some(_)) => {
            check_declared_return(prev_function, |d| d.matches_type(t), || t.name())?;
            Ok(Carried::Type(t))
        }
        (Err(e), Some(_frame)) => {
            let with_frame = match prev_function {
                Some(contract) => {
                    let label = match contract {
                        ReturnContract::Function(f) => f.summarize(),
                        ReturnContract::Arm { kind, .. } => kind.to_string(),
                        ReturnContract::PerCall { func, .. } => func.summarize(),
                    };
                    e.with_frame(crate::machine::TraceFrame::bare(label.clone(), label))
                }
                None => e,
            };
            Err(with_frame)
        }
        (other, None) => other,
    }
}

/// The declared-return check shared by the `Object` and `Type` finalize arms: pull the
/// declared return type off `contract` (a `Function`'s resolved `return_type`, or an
/// `Arm`'s `-> :T`), and if there is one, verify the lifted carrier satisfies it.
/// `satisfies` runs the channel-appropriate predicate (`matches_value` / `matches_type`)
/// and `got_name` names the carrier for the mismatch error. Returns the declared type so
/// the caller can re-tag against it (the `Object` arm coarsens; the `Type` arm discards
/// it), `Ok(None)` when nothing is declared — a `Function` whose signature return is
/// non-`Resolved` (a `Deferred` carrier still in its FN-def signature) has no type here —
/// or `Err` with the labelled `TypeMismatch`. A `PerCall` carries the *resolved* per-call
/// type and is checked + stamped here, labelled "per-call return type".
fn check_declared_return<'run>(
    contract: Option<ReturnContract<'run>>,
    satisfies: impl FnOnce(&KType<'run>) -> bool,
    got_name: impl FnOnce() -> String,
) -> Result<Option<&'run KType<'run>>, KError> {
    let (declared, label, per_call) = match contract {
        Some(ReturnContract::Function(f)) => match &f.signature.return_type {
            crate::machine::model::types::ReturnType::Resolved(d) => (d, f.summarize(), false),
            _ => return Ok(None),
        },
        Some(ReturnContract::Arm { ret, kind }) => (ret, kind.to_string(), false),
        Some(ReturnContract::PerCall { func, ret }) => (ret, func.summarize(), true),
        None => return Ok(None),
    };
    if !satisfies(declared) {
        let expected = if per_call {
            format!("{} (per-call return type)", declared.name())
        } else {
            declared.name()
        };
        return Err(KError::new(KErrorKind::TypeMismatch {
            arg: "<return>".to_string(),
            expected,
            got: got_name(),
        })
        .with_frame(crate::machine::TraceFrame::bare(label.clone(), label)));
    }
    Ok(Some(declared))
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
