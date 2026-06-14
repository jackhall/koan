use std::rc::Rc;

use crate::machine::core::kfunction::body::{ErasedContract, ReturnContract};
use crate::machine::core::{assemble_body_chain, ScopeId};
use crate::machine::model::KType;
use crate::machine::{KError, KErrorKind, LexicalFrame, NodeId};

use super::super::lift::{lift_kobject, lift_ktype};
use super::super::nodes::{CallFrame, Node, NodeOutput, NodeStep, NodeWork};
use super::Scheduler;
use crate::machine::model::Carried;

impl<'run> Scheduler<'run> {
    /// On `Done` with a frame, the return `Value` references the per-call arena that's
    /// about to drop, so it must be lifted into the captured scope's arena before the
    /// frame is released. See design/memory-model.md.
    pub fn execute(&mut self) -> Result<(), KError> {
        while let Some(idx) = self.queues.pop_next() {
            let id = NodeId(idx);
            let node = self.store.take_for_run(id);
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
            let guard = self.enter_slot_step(cart, reserve, prev_chain_carrier.clone(), node_scope);
            // Expose to the dispatch step whether this slot is a tail call within an established
            // contract chain — a deferred-return FN dispatched here skips resolving its own return
            // type (keep-first discards it anyway).
            self.active_in_contract_chain = prev_contract.is_some();
            let step = match work {
                NodeWork::Dispatch { expr, pre_subs } => {
                    crate::machine::execute::dispatch::run_dispatch(self, expr, pre_subs, idx)
                }
                NodeWork::DispatchResume { resume, .. } => {
                    crate::machine::execute::dispatch::run_dispatch_resume(self, resume, idx)
                }
                NodeWork::Combine {
                    deps,
                    park_count,
                    finish,
                    dep_error_frame,
                } => self.run_combine(deps, park_count, finish, dep_error_frame, idx),
                NodeWork::Catch { from, finish } => self.run_catch(from, finish, idx),
                NodeWork::Lift(state) => NodeStep::Done(Self::run_lift(state)),
            };
            // The post-step token owns the slot's frame at step end and is the *only* source of
            // the step scope (via `post.step_scope()`), so the wrong-frame read that ambient
            // `active_frame` allowed is unspellable here.
            let post = self.exit_slot_step(guard);
            self.active_in_contract_chain = false;
            // Drain re-entrant writes against the step scope.
            post.step_scope().drain_pending();
            match step {
                NodeStep::Done(output) => {
                    // Lift the terminal out of the dying per-call frame into the surviving
                    // captured-scope arena (`dest_arena`, a genuine `&'run`). A non-dying run frame
                    // (empty arena; top-level values live in the run arena) reads as frameless.
                    let dest_arena = post.step_scope().outer().map(|o| o.arena);
                    let frame = (!post.prev_frame.non_dying()).then_some(&post.prev_frame);
                    // Re-anchor the erased contract against the step's cart, witnessed by `frame`.
                    // `compute_done_output` consults the contract only when `frame` is `Some` (a
                    // real per-call frame, which is exactly when a contract is set), so a contract
                    // on the non-dying run frame is harmlessly skipped.
                    let prev_function = match (prev_contract, frame) {
                        (Some(c), Some(witness)) => Some(unsafe { c.reattach(witness) }),
                        _ => None,
                    };
                    let result = compute_done_output(output, frame, dest_arena, prev_function);
                    if matches!(result, NodeOutput::Err(_)) {
                        post.step_scope().clear_placeholders_for_producer(id);
                    }
                    self.finalize(idx, result);
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
                    let new_chain = compute_replace_chain(
                        prev_chain_carrier,
                        block_entry,
                        new_function,
                        new_frame.as_deref(),
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
                            self.store.reinstall_with_frame(
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
                            self.store.reinstall(
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
                    if self.deps.pending_count(idx) == 0 {
                        self.queues.push_after_replace(idx);
                    }
                }
            }
        }
        // Any slot still `PreRun` after drain is parked on a dependency that can
        // no longer fire — surface the cycle rather than panic on the caller's
        // top-level result read.
        if let Some((pending, sample)) = self.store.unresolved() {
            return Err(KError::new(KErrorKind::SchedulerDeadlock {
                pending,
                sample,
            }));
        }
        Ok(())
    }

    /// Invariant: every consumer drained here is parked with a non-zero counter;
    /// freed slots are scrubbed from every producer's `notify_list` before the
    /// producer drains.
    ///
    /// Stamps must all land before any queue push: a later stamp re-reading the
    /// slot must observe the prior transition.
    pub(in crate::machine::execute::scheduler) fn finalize(
        &mut self,
        idx: usize,
        output: NodeOutput<'run>,
    ) {
        let id = NodeId(idx);
        self.store.finalize(id, output);
        let drained = self.deps.drain_notify(idx);
        let mut woken: Vec<usize> = Vec::new();
        for (consumer, hit_zero) in drained {
            self.store.push_recent_wake(NodeId(consumer), id);
            if hit_zero {
                self.store.stamp_lift_ready(NodeId(consumer), id);
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

/// Lift a `Done` step's terminal out of the dying per-call `frame` into `dest_arena` (the
/// surviving captured-scope arena) and enforce the declared return contract, returning the slot's
/// final terminal. A `None` frame (a frameless slot or the non-dying run frame) passes the value
/// through untouched. A failed return-type check becomes `Err` — the caller clears placeholders
/// and finalizes. Pure: the scope-derived inputs were captured by the caller while the step's
/// scope was still ambient, so this holds no scope borrow.
fn compute_done_output<'run>(
    output: NodeOutput<'run>,
    frame: Option<&Rc<crate::machine::core::CallArena>>,
    dest_arena: Option<&'run crate::machine::core::RuntimeArena>,
    prev_function: Option<ReturnContract<'run>>,
) -> NodeOutput<'run> {
    match (output, frame) {
        (NodeOutput::Value(Carried::Object(v)), Some(frame)) => {
            let dest = dest_arena.expect("per-call scope must have an outer (its captured scope)");
            let mut lifted_obj = lift_kobject(v, frame);
            match check_declared_return(
                prev_function,
                |d| d.matches_value(&lifted_obj),
                || lifted_obj.ktype().name(),
            ) {
                // Re-tag to the declared return type so downstream dispatch sees the contract
                // (may coarsen, e.g. `List<Number>` through `:(LIST OF Any)` -> `List<Any>`).
                Ok(Some(declared)) => lifted_obj = lifted_obj.stamp_type(declared),
                Ok(None) => {}
                Err(err) => return NodeOutput::Err(err),
            }
            NodeOutput::Value(Carried::Object(dest.alloc_object(lifted_obj)))
        }
        // A type flowing the type channel re-anchors any `Module` frame and re-allocs into the
        // destination arena, after the shared declared-return check via `matches_type`. The type
        // channel ignores the returned declared type — unlike the `Object` arm, it does not re-tag.
        (NodeOutput::Value(Carried::Type(t)), Some(frame)) => {
            let dest = dest_arena.expect("per-call scope must have an outer (its captured scope)");
            let lifted_t = lift_ktype(t, frame);
            if let Err(err) = check_declared_return(
                prev_function,
                |d| d.matches_type(&lifted_t),
                || lifted_t.name(),
            ) {
                return NodeOutput::Err(err);
            }
            NodeOutput::Value(Carried::Type(dest.alloc_ktype(lifted_t)))
        }
        (NodeOutput::Err(e), Some(_frame)) => {
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
            NodeOutput::Err(with_frame)
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
/// - `Some(_)` + `Function(fn)` — FN body invoke. Chain is assembled from the FN's
///   lexical `outer` walk so depth tracks lexical nesting, not call depth
///   (tail-recursive loops produce equal-depth chains each iteration).
fn compute_replace_chain<'run>(
    prev_chain: Rc<LexicalFrame>,
    block_entry: Option<ScopeId>,
    new_function: Option<ReturnContract<'run>>,
    new_frame: Option<&crate::machine::core::CallArena>,
    body_index: usize,
) -> Rc<LexicalFrame> {
    let Some(scope_id) = block_entry else {
        return prev_chain;
    };
    match (new_function, new_frame) {
        // `Function` and `PerCall` (a deferred FN body) both assemble the FN-body chain.
        (Some(ReturnContract::Function(_) | ReturnContract::PerCall { .. }), Some(frame)) => {
            assemble_body_chain(frame.scope(), prev_chain, body_index)
        }
        _ => LexicalFrame::push(Some(prev_chain), scope_id, body_index),
    }
}
