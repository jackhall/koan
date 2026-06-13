//! Dispatch-side facade over `&mut Scheduler<'run>`.
//!
//! `DispatchCtx` is the `&mut`-holding surface a dispatch step is entered with — slot queries,
//! dep-graph mutations, sub-submission, the recent-wakes side-channel, and the dispatcher-only
//! ops (`build_bare_outcomes`, `install_eager_subs`, `replace_with_parked_dispatch`) that spell
//! the scheduler's internal fields on the dispatcher's behalf. It is **not** a `SchedulerHandle`:
//! the *execution* machinery a resolved call hands off to (`exec::invoke`, `field_list`) takes the
//! raw `&mut Scheduler` via [`Self::scheduler_mut`], so `Scheduler` is the sole `SchedulerHandle`
//! impl.
//!
//! [`DispatchCx`] is the read-only peer used by a migrated handler's decide phase: it holds
//! `&Scheduler` (never `&mut`) and returns a
//! [`DispatchOutcome`](super::outcome::DispatchOutcome) the harness applies.
//!
//! Dispatch *shape* modules (`keyworded`, `fn_value`, `single_poll`)
//! never name scheduler fields directly — only `ctx.foo(...)` — so a
//! future scheduler internal rename (`active_chain` → ..., `DepGraph`
//! split) is a single-file change inside `scheduler/`.

use std::rc::Rc;

use crate::machine::core::kfunction::KFunction;
use crate::machine::core::source::Spanned;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::Carried;
use crate::machine::{KError, LexicalFrame, NameOutcome, NodeId, SchedulerHandle, Scope};

use super::super::nodes::{NodeOutput, NodeStep, NodeWork};
use super::super::scheduler::Scheduler;
use super::outcome::{DispatchDep, DispatchOutcome};
use super::{bind_frame_err, harness, resolve_name_part, DispatchState, PendingSub};

/// Newtype wrapping `&'b mut Scheduler<'run>`, exposing exactly the
/// scheduler operations the dispatcher uses. `'run` is the scheduler's
/// arena lifetime; `'b` is the borrow lifetime of the scheduler the
/// dispatcher holds for the duration of one Dispatch step.
pub(in crate::machine::execute) struct DispatchCtx<'run, 'b> {
    sched: &'b mut Scheduler<'run>,
}

/// Read-only dispatch view — the decide-phase peer of [`DispatchCtx`] that holds only
/// `&Scheduler`, never `&mut`. A migrated shape handler decides against this and *returns*
/// a [`DispatchOutcome`](super::outcome::DispatchOutcome); the harness reborrows the
/// scheduler mutably to apply the writes. The borrow contract: a `DispatchCx` lives only
/// for the decide call, the handler returns an owned outcome, and the immutable borrow ends
/// before the harness takes `&mut` — so decide and apply never overlap.
///
/// The static-over-the-step reads (`current_scope`, `chain_deref`, …) and the live reads of
/// *pre-existing* producers (`is_result_ready`, `would_create_cycle`, `read_result`) both
/// forward to the borrowed scheduler; the dispatcher genuinely reads evolving graph state, so
/// full scheduler-unawareness (the builtin model) is not a goal — only the *writes* defer.
pub(in crate::machine::execute) struct DispatchCx<'run, 's> {
    sched: &'s Scheduler<'run>,
}

impl<'run, 's> DispatchCx<'run, 's> {
    pub(super) fn new(sched: &'s Scheduler<'run>) -> Self {
        Self { sched }
    }

    // Read surface (forwards on `&self`). This grows one method per migrated handler — the
    // static-over-the-step reads (`current_scope`, `chain_deref`, `active_chain`,
    // `in_contract_chain`) and the live reads of pre-existing producers (`is_result_ready`,
    // `would_create_cycle`, `read_result`) all forward to the borrowed scheduler.

    pub(super) fn current_scope(&self) -> &Scope<'run> {
        self.sched.current_scope()
    }

    pub(super) fn chain_deref(&self) -> Option<&LexicalFrame> {
        self.sched.chain_deref()
    }
}

impl<'run, 'b> DispatchCtx<'run, 'b> {
    pub(in crate::machine::execute) fn new(sched: &'b mut Scheduler<'run>) -> Self {
        Self { sched }
    }

    /// Immutable reborrow of the wrapped scheduler — the bridge that lets a still-`&mut`
    /// router build a read-only [`DispatchCx`] for a migrated handler, decide, then reborrow
    /// `&mut self` for the harness. The returned `DispatchCx` must be dropped (last-used)
    /// before the harness call, which NLL enforces.
    pub(super) fn read_view(&self) -> DispatchCx<'run, '_> {
        DispatchCx::new(self.sched)
    }

    /// `&mut` reborrow of the wrapped scheduler — the harness's hand-off to the *execution*
    /// machinery (`exec::invoke`, `field_list`), which runs a resolved call body against the
    /// scheduler's own `SchedulerHandle`. Execution is not dispatch decide, so it takes the raw
    /// scheduler rather than this facade — which is why [`DispatchCtx`] is itself not a
    /// `SchedulerHandle`.
    pub(super) fn scheduler_mut(&mut self) -> &mut Scheduler<'run> {
        self.sched
    }

    // ----- ambient state (reads forwarded to the scheduler) -----

    /// The slot's scope — the dispatcher's primary read surface for name / type resolution.
    pub(super) fn current_scope(&self) -> &Scope<'run> {
        self.sched.current_scope()
    }

    /// `&` borrow of the active lexical chain for name-resolution
    /// helpers; thin wrap over `Scheduler::chain_deref`.
    pub(super) fn chain_deref(&self) -> Option<&LexicalFrame> {
        self.sched.chain_deref()
    }

    /// Cloned `Rc` to the active chain — read by the `KeywordedState`
    /// initial-resolve site that takes the chain's `index`.
    pub(super) fn active_chain(&self) -> Option<Rc<LexicalFrame>> {
        self.sched.active_chain_clone()
    }

    /// Cloned `Rc` to the active lexical chain — the `record_type` field-list elaborator needs it
    /// by value to rebuild the elaborator across a Combine deferral.
    pub(super) fn current_lexical_chain(&self) -> Option<Rc<LexicalFrame>> {
        self.sched.current_lexical_chain()
    }

    // ----- slot queries -----

    pub(super) fn is_result_ready(&self, id: NodeId) -> bool {
        self.sched.is_result_ready(id)
    }

    pub(super) fn read_result(&self, id: NodeId) -> Result<Carried<'run>, &KError> {
        self.sched.read_result(id)
    }

    // ----- dep graph -----

    pub(super) fn add_park_edge(&mut self, producer: NodeId, consumer: NodeId) {
        self.sched.add_park_edge(producer, consumer);
    }

    pub(super) fn add_owned_edge(&mut self, producer: NodeId, consumer: NodeId) {
        self.sched.add_owned_edge(producer, consumer);
    }

    pub(super) fn would_create_cycle(&self, producer: NodeId, consumer: NodeId) -> bool {
        self.sched.would_create_cycle(producer, consumer)
    }

    pub(super) fn clear_dep_edges(&mut self, idx: usize) {
        self.sched.clear_dep_edges(idx);
    }

    // ----- submission / reclaim -----
    //
    // Each forwards to the scheduler, which inherits `active_chain` / `active_frame` correctly
    // via its own `add` path. Fresh-`Dispatch` sub-submission (`add_dispatch_here`) lives in the
    // harness, off `scheduler_mut()`.

    pub(super) fn schedule_list_literal(&mut self, items: Vec<ExpressionPart<'run>>) -> NodeId {
        self.sched.schedule_list_literal(items)
    }

    pub(super) fn schedule_dict_literal(
        &mut self,
        pairs: Vec<(ExpressionPart<'run>, ExpressionPart<'run>)>,
    ) -> NodeId {
        self.sched.schedule_dict_literal(pairs)
    }

    pub(super) fn schedule_record_literal(
        &mut self,
        fields: Vec<(String, ExpressionPart<'run>)>,
    ) -> NodeId {
        self.sched.schedule_record_literal(fields)
    }

    pub(super) fn free(&mut self, idx: usize) {
        self.sched.free(idx);
    }

    // ----- recent wakes side channel -----

    pub(super) fn take_recent_wakes(&mut self, consumer: NodeId) -> Vec<NodeId> {
        self.sched.take_recent_wakes(consumer)
    }

    // ----- thin forward to scheduler op shared with combinators -----

    /// `Scheduler::defer_to_lift` is shared with `run_combine` /
    /// `run_catch`; the DispatchCtx wrapper keeps the dispatch-side
    /// surface uniform.
    pub(super) fn defer_to_lift(&mut self, idx: usize, bind_id: NodeId) -> NodeStep<'run> {
        self.sched.defer_to_lift(idx, bind_id)
    }

    // ----- relocated dispatcher-only ops (bodies were on `impl Scheduler`) -----

    /// Map a body's [`BodyResult`] onto the scheduler's [`NodeStep`]. Shared by the builtin-call and
    /// user-fn `exec` paths in `dispatch::exec`, so both land a body's outcome identically.
    pub(super) fn body_result_to_step(
        &mut self,
        result: crate::machine::core::kfunction::BodyResult<'run>,
        idx: usize,
    ) -> NodeStep<'run> {
        use crate::machine::core::kfunction::BodyResult;
        match result {
            BodyResult::Value(c) => NodeStep::Done(NodeOutput::Value(c)),
            BodyResult::Tail {
                expr,
                frame,
                function,
                block_entry,
                body_index,
            } => NodeStep::Replace {
                work: NodeWork::dispatch(expr),
                frame,
                function,
                block_entry,
                body_index,
            },
            BodyResult::DeferTo(id) => self.sched.defer_to_lift(idx, id),
            BodyResult::Err(e) => NodeStep::Done(NodeOutput::Err(e)),
        }
    }

    /// Build the per-part `bare_outcomes` cache: one `resolve_name_part`
    /// per bare-name part, `None` otherwise. `consumer = None` defers
    /// cycle detection to the splice walk.
    pub(super) fn build_bare_outcomes(
        &self,
        parts: &[Spanned<ExpressionPart<'run>>],
    ) -> Vec<Option<NameOutcome<'run>>> {
        parts
            .iter()
            .map(|p| match &p.value {
                ExpressionPart::Identifier(_) | ExpressionPart::Type(_) => Some(resolve_name_part(
                    self.current_scope(),
                    &p.value,
                    self.sched,
                    None,
                )),
                _ => None,
            })
            .collect()
    }

    /// Submit each `PendingSub` and park the slot on the in-flight ones as a
    /// [`NodeWork::DispatchCombine`]. A `Reuse` of an already-resolved producer splices inline (a
    /// freshly minted sub is never terminal in the same step); every in-flight sub becomes an
    /// owned dep whose finish splices the resolved values into `working_expr` and routes on
    /// `picked` — `Some(f)` calls `f`, `None` re-resolves through [`KeywordedState::finish`]. The
    /// `<bind>` dep-error frame rides on `dep_error_frame`, attached by `run_dispatch_combine` at
    /// the short-circuit (so the finish only ever sees resolved deps). `picked = Some(f)` is the
    /// FunctionValueCall install; `None` is Keyworded.
    pub(super) fn install_eager_subs(
        &mut self,
        mut working_expr: KExpression<'run>,
        staged_subs: Vec<(usize, PendingSub<'run>)>,
        picked: Option<&'run KFunction<'run>>,
        idx: usize,
    ) -> NodeStep<'run> {
        use super::super::nodes::DispatchCombineFinish;
        let mut deps: Vec<DispatchDep<'run>> = Vec::with_capacity(staged_subs.len());
        let mut part_indices: Vec<usize> = Vec::with_capacity(staged_subs.len());
        // Reuse producers consumed inline (spliced into `working_expr`); the harness reclaims
        // them so the decide phase issues no `free` write.
        let mut free: Vec<usize> = Vec::new();
        for (i, pending) in staged_subs {
            // A `Reuse` is a pre-existing producer the pre-pick found: splice it inline if it has
            // already resolved (a read of a static-over-this-step slot), else park on it as an
            // `Existing` dep. A freshly-staged sub (`Dispatch`/`*Lit`) is never terminal in the
            // same step (submission is enqueue-then-drain), so it is always a fresh dep the harness
            // submits — never read back here.
            let dep = match pending {
                PendingSub::Reuse(id) => {
                    if self.is_result_ready(id) {
                        match self.read_result(id) {
                            Err(e) => return bind_frame_err(e, &working_expr),
                            Ok(value) => {
                                working_expr.parts[i].value = ExpressionPart::Future(value);
                                free.push(id.index());
                                continue;
                            }
                        }
                    }
                    DispatchDep::Existing(id)
                }
                PendingSub::Dispatch(sub_expr) => DispatchDep::Dispatch(sub_expr),
                PendingSub::ListLit(items) => DispatchDep::ListLit(items),
                PendingSub::DictLit(pairs) => DispatchDep::DictLit(pairs),
                PendingSub::RecordLit(fields) => DispatchDep::RecordLit(fields),
            };
            deps.push(dep);
            part_indices.push(i);
        }
        if deps.is_empty() {
            // Every sub was an already-resolved `Reuse` spliced inline — `working_expr` is fully
            // resolved, so continue to the finish now instead of parking on a Combine; the inline
            // frees ride on the resulting Invoke/Redispatch outcome.
            let outcome = finish_eager_subs(working_expr, picked, free);
            return harness::apply_dispatch_outcome(self, outcome, idx);
        }
        let dep_error_frame = Some(crate::machine::TraceFrame::from_expr(
            "<bind>",
            &working_expr,
        ));
        let finish: DispatchCombineFinish<'run> = Box::new(move |ctx, results, idx| {
            // The short-circuit already guaranteed every dep resolved; splice each into the
            // slot it was staged from, then run the routed continuation. No inline frees remain at
            // wake — those were drained when the Combine was installed.
            for (slot, value) in part_indices.iter().zip(results) {
                working_expr.parts[*slot].value = ExpressionPart::Future(*value);
            }
            let outcome = finish_eager_subs(working_expr, picked, Vec::new());
            harness::apply_dispatch_outcome(ctx, outcome, idx)
        });
        let outcome = DispatchOutcome::Combine {
            deps,
            dep_error_frame,
            finish,
            free,
        };
        harness::apply_dispatch_outcome(self, outcome, idx)
    }

    /// Standard `NodeStep::Replace` for parked-Dispatch install sites:
    /// drops the entry expression to an empty placeholder (the state
    /// carries the evolving `working_expr` from here on).
    pub(super) fn replace_with_parked_dispatch(
        &self,
        state: DispatchState<'run>,
    ) -> NodeStep<'run> {
        NodeStep::Replace {
            work: NodeWork::Dispatch {
                expr: KExpression::new(Vec::new()),
                state,
            },
            frame: None,
            function: None,
            block_entry: None,
            body_index: 0,
        }
    }
}

/// Route a fully-spliced eager-subs `working_expr` to its continuation — the shared tail of
/// the `DispatchCombine` finish and its all-inline fast path. `Some(f)` names the committed
/// call as an [`DispatchOutcome::Invoke`]; `None` defers to a [`DispatchOutcome::Redispatch`]
/// (the harness re-resolves via [`KeywordedState::finish`], where an element-typed `Future(_)`
/// revealed by a sub surfaces as a slot-terminal `DispatchFailed`). Pure data — no `&mut`.
fn finish_eager_subs<'run>(
    working_expr: KExpression<'run>,
    picked: Option<&'run KFunction<'run>>,
    free: Vec<usize>,
) -> DispatchOutcome<'run> {
    match picked {
        Some(f) => DispatchOutcome::Invoke {
            picked: f,
            working_expr,
            free,
        },
        None => DispatchOutcome::Redispatch { working_expr, free },
    }
}
