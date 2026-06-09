//! Dispatch-side facade over `&mut Scheduler<'a>`.
//!
//! `DispatchCtx` is the typed surface every dispatch entry point sees.
//! It names exactly the scheduler operations the dispatcher uses — slot
//! queries, dep-graph mutations, sub-submission, the recent-wakes
//! side-channel, and the dispatcher-only ops (`build_bare_outcomes`,
//! `install_eager_subs`, `replace_with_parked_dispatch`,
//! `resume_eager_subs`, `invoke_to_step{,_pinned}`) that used to live
//! on `impl Scheduler` solely so they could spell the scheduler's
//! internal fields.
//!
//! Dispatch *shape* modules (`keyworded`, `fn_value`, `single_poll`)
//! never name scheduler fields directly — only `ctx.foo(...)` — so a
//! future scheduler internal rename (`active_chain` → ..., `DepGraph`
//! split) is a single-file change inside `scheduler/`.
//!
//! `DispatchCtx` also implements [`SchedulerHandle`] so builtin bodies
//! invoked through `KFunction::invoke` (e.g. `newtype_construct`) see
//! the dispatcher's contextual frame/chain rather than a fresh borrow
//! of the bare scheduler; the impl forwards every method to
//! `self.sched` and the `with_active_frame` body closure re-receives
//! the same `&mut DispatchCtx`.

use std::rc::Rc;

use crate::machine::core::kfunction::KFunction;
use crate::machine::core::source::Spanned;
use crate::machine::core::{CallArena, ScopeId};
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::Carried;
use crate::machine::{
    CatchFinish, CombineFinish, KError, KFuture, LexicalFrame, NameOutcome, NodeId,
    SchedulerHandle, Scope,
};

use super::super::nodes::{NodeOutput, NodeStep, NodeWork};
use super::super::scheduler::Scheduler;
use super::{
    bind_frame_err, keyworded::KeywordedState, resolve_name_part, DispatchState, EagerSubsInstall,
    EagerSubsTrack, PendingSub,
};

/// Newtype wrapping `&'b mut Scheduler<'a>`, exposing exactly the
/// scheduler operations the dispatcher uses. `'a` is the scheduler's
/// arena lifetime; `'b` is the borrow lifetime of the scheduler the
/// dispatcher holds for the duration of one Dispatch step.
pub(in crate::machine::execute) struct DispatchCtx<'a, 'b> {
    sched: &'b mut Scheduler<'a>,
}

impl<'a, 'b> DispatchCtx<'a, 'b> {
    pub(in crate::machine::execute) fn new(sched: &'b mut Scheduler<'a>) -> Self {
        Self { sched }
    }

    // ----- ambient state -----

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

    // ----- slot queries -----

    pub(super) fn is_result_ready(&self, id: NodeId) -> bool {
        self.sched.is_result_ready(id)
    }

    pub(super) fn read_result(&self, id: NodeId) -> Result<Carried<'a>, &KError> {
        self.sched.read_result(id)
    }

    pub(super) fn read(&self, id: NodeId) -> Carried<'a> {
        self.sched.read(id)
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
    // Sub-Dispatch submission goes through the `SchedulerHandle`
    // `add_dispatch` impl below — that path inherits `active_chain` /
    // `active_frame` correctly via `Scheduler::add_dispatch -> add`.

    pub(super) fn schedule_list_literal(
        &mut self,
        items: Vec<ExpressionPart<'a>>,
        scope: &'a Scope<'a>,
    ) -> NodeId {
        self.sched.schedule_list_literal(items, scope)
    }

    pub(super) fn schedule_dict_literal(
        &mut self,
        pairs: Vec<(ExpressionPart<'a>, ExpressionPart<'a>)>,
        scope: &'a Scope<'a>,
    ) -> NodeId {
        self.sched.schedule_dict_literal(pairs, scope)
    }

    pub(super) fn schedule_record_literal(
        &mut self,
        fields: Vec<(String, ExpressionPart<'a>)>,
        scope: &'a Scope<'a>,
    ) -> NodeId {
        self.sched.schedule_record_literal(fields, scope)
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
    pub(super) fn defer_to_lift(&mut self, idx: usize, bind_id: NodeId) -> NodeStep<'a> {
        self.sched.defer_to_lift(idx, bind_id)
    }

    // ----- relocated dispatcher-only ops (bodies were on `impl Scheduler`) -----

    /// `KFunction::invoke` shim that tail-rewrites the slot's work. See
    /// the historical `Scheduler::invoke_to_step` for the contract; the
    /// only change is that we now pass `self` (a `&mut dyn
    /// SchedulerHandle<'a, 's>` via the `SchedulerHandle for DispatchCtx`
    /// impl) so sub-slots spawned by the body inherit the dispatcher's
    /// contextual chain/frame state.
    pub(super) fn invoke_to_step(&mut self, future: KFuture<'a>, idx: usize) -> NodeStep<'a> {
        use crate::machine::core::kfunction::BodyResult;
        match future.function.invoke(self, future.bundle) {
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

    /// `invoke_to_step` with the slot's reserve frame consumed when
    /// available; see [per-call-arena-protocol.md § Ping-pong reserve
    /// frame](../../../../design/per-call-arena-protocol.md#ping-pong-reserve-frame).
    /// All scheduler frame state is reached through the narrow
    /// `active_*` accessors so the storage layout stays scheduler-side.
    pub(super) fn invoke_to_step_pinned(
        &mut self,
        future: KFuture<'a>,
        idx: usize,
    ) -> NodeStep<'a> {
        if let Some(reserve) = self.sched.active_reserve_take() {
            let local_pin = self.sched.active_frame_replace(Some(reserve));
            let step = self.invoke_to_step(future, idx);
            let _ = self.sched.active_frame_replace(local_pin);
            step
        } else {
            let _frame_pin = self.sched.active_frame_clone();
            self.invoke_to_step(future, idx)
        }
    }

    /// Build the per-part `bare_outcomes` cache: one `resolve_name_part`
    /// per bare-name part, `None` otherwise. `consumer = None` defers
    /// cycle detection to the splice walk.
    pub(super) fn build_bare_outcomes(
        &self,
        parts: &[Spanned<ExpressionPart<'a>>],
        scope: &'a Scope<'a>,
    ) -> Vec<Option<NameOutcome<'a>>> {
        parts
            .iter()
            .map(|p| match &p.value {
                ExpressionPart::Identifier(_) | ExpressionPart::Type(_) => {
                    Some(resolve_name_part(scope, &p.value, self.sched, None))
                }
                _ => None,
            })
            .collect()
    }

    /// Submit each `PendingSub`, splice already-terminal subs inline,
    /// install an Owned dep_edge from each in-flight sub to this slot,
    /// and return the routed [`EagerSubsInstall`]. `picked = Some(f)` is
    /// the FunctionValueCall install; `None` is Keyworded.
    pub(super) fn install_eager_subs(
        &mut self,
        mut working_expr: KExpression<'a>,
        staged_subs: Vec<(usize, PendingSub<'a>)>,
        picked: Option<&'a KFunction<'a>>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> EagerSubsInstall<'a> {
        let mut pending_subs: Vec<(usize, NodeId)> = Vec::with_capacity(staged_subs.len());
        for (i, pending) in staged_subs {
            let sub_id = match pending {
                PendingSub::Reuse(id) => id,
                PendingSub::Dispatch(sub_expr) => self.add_dispatch(sub_expr, scope),
                PendingSub::ListLit(items) => self.schedule_list_literal(items, scope),
                PendingSub::DictLit(pairs) => self.schedule_dict_literal(pairs, scope),
                PendingSub::RecordLit(fields) => self.schedule_record_literal(fields, scope),
            };
            if self.is_result_ready(sub_id) {
                match self.read_result(sub_id) {
                    Err(e) => return EagerSubsInstall::DepError(bind_frame_err(e, &working_expr)),
                    Ok(value) => {
                        working_expr.parts[i].value = ExpressionPart::Future(value);
                        self.free(sub_id.index());
                    }
                }
            } else {
                self.add_owned_edge(sub_id, NodeId(idx));
                pending_subs.push((i, sub_id));
            }
        }
        if pending_subs.is_empty() {
            EagerSubsInstall::AllInline(working_expr)
        } else {
            EagerSubsInstall::Parked(EagerSubsTrack {
                working_expr,
                subs: pending_subs,
                picked,
            })
        }
    }

    /// Standard `NodeStep::Replace` for parked-Dispatch install sites:
    /// drops the entry expression to an empty placeholder (the state
    /// carries the evolving `working_expr` from here on).
    pub(super) fn replace_with_parked_dispatch(&self, state: DispatchState<'a>) -> NodeStep<'a> {
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

    /// Track-completion continuation for `eager_subs` tracks. Routes on
    /// `track.picked`:
    ///
    /// - `None` (Keyworded install) — tail into
    ///   [`KeywordedState::finish`], which re-resolves dispatch so an
    ///   element-typed `Future(_)` revealed by a sub can surface as
    ///   `DispatchFailed` rather than a bind-time `TypeMismatch`.
    /// - `Some(f)` (FunctionValueCall install) — bind `f` directly.
    pub(super) fn resume_eager_subs(
        &mut self,
        track: EagerSubsTrack<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        let EagerSubsTrack {
            mut working_expr,
            subs,
            picked,
        } = track;
        for (_, sub_id) in &subs {
            if let Err(e) = self.read_result(*sub_id) {
                return Ok(bind_frame_err(e, &working_expr));
            }
        }
        let dep_indices: Vec<usize> = subs.iter().map(|(_, d)| d.index()).collect();
        for (part_idx, dep_id) in subs {
            let value = self.read(dep_id);
            working_expr.parts[part_idx].value = ExpressionPart::Future(value);
        }
        self.clear_dep_edges(idx);
        for d in dep_indices {
            self.free(d);
        }
        match picked {
            None => KeywordedState::finish(self, working_expr, scope, idx),
            Some(f) => match f.bind(working_expr) {
                Ok(future) => Ok(self.invoke_to_step_pinned(future, idx)),
                Err(e) => Ok(NodeStep::Done(NodeOutput::Err(e))),
            },
        }
    }
}

// =====================================================================
// SchedulerHandle impl
// =====================================================================
//
// Builtin-facing surface for closures the dispatcher hands off to
// `KFunction::invoke`. Every method forwards to the wrapped scheduler;
// the body of `with_active_frame` re-receives `&mut DispatchCtx`, so
// further sub-builtins still see the dispatcher's contextual state.

impl<'a, 'b, 's> SchedulerHandle<'a, 's> for DispatchCtx<'a, 'b> {
    fn add_dispatch(&mut self, expr: KExpression<'a>, scope: &'a Scope<'a>) -> NodeId {
        self.sched.add_dispatch(expr, scope)
    }

    fn add_combine(
        &mut self,
        owned_subs: Vec<NodeId>,
        park_producers: Vec<NodeId>,
        scope: &'a Scope<'a>,
        finish: CombineFinish<'a>,
    ) -> NodeId {
        self.sched
            .add_combine(owned_subs, park_producers, scope, finish)
    }

    fn add_catch(&mut self, from: NodeId, scope: &'a Scope<'a>, finish: CatchFinish<'a>) -> NodeId {
        self.sched.add_catch(from, scope, finish)
    }

    fn current_frame(&self) -> Option<Rc<CallArena>> {
        self.sched.current_frame()
    }

    /// Pin/swap the ambient `active_frame` around `body`. The closure
    /// receives `&mut DispatchCtx` (this same object as
    /// `&mut dyn SchedulerHandle<'a, 's>`), so nested builtin invokes also
    /// route through the dispatcher's facade rather than re-borrowing
    /// the bare scheduler.
    fn with_active_frame(
        &mut self,
        frame: Rc<CallArena>,
        body: &mut dyn FnMut(&mut dyn SchedulerHandle<'a, 's>),
    ) {
        let prev = self.sched.active_frame_replace(Some(frame));
        body(self);
        let _ = self.sched.active_frame_replace(prev);
    }

    fn try_take_reusable_frame_for_tail(&mut self) -> Option<Rc<CallArena>> {
        self.sched.try_take_reusable_frame_for_tail()
    }

    fn current_lexical_chain(&self) -> Option<Rc<LexicalFrame>> {
        self.sched.current_lexical_chain()
    }

    fn enter_block(
        &mut self,
        scope_id: ScopeId,
        statements: Vec<KExpression<'a>>,
        scope: &'a Scope<'a>,
    ) -> Vec<NodeId> {
        self.sched.enter_block(scope_id, statements, scope)
    }

    fn add_dispatch_with_chain(
        &mut self,
        expr: KExpression<'a>,
        scope: &'a Scope<'a>,
        chain: Rc<LexicalFrame>,
    ) -> NodeId {
        self.sched.add_dispatch_with_chain(expr, scope, chain)
    }

    fn add_dispatch_with_chain_in_frame(
        &mut self,
        expr: KExpression<'a>,
        chain: Rc<LexicalFrame>,
    ) -> NodeId {
        self.sched.add_dispatch_with_chain_in_frame(expr, chain)
    }

    fn add_dispatch_in_frame(&mut self, expr: KExpression<'a>) -> NodeId {
        self.sched.add_dispatch_in_frame(expr)
    }

    fn add_combine_in_frame(
        &mut self,
        owned_subs: Vec<NodeId>,
        park_producers: Vec<NodeId>,
        finish: CombineFinish<'a>,
    ) -> NodeId {
        self.sched
            .add_combine_in_frame(owned_subs, park_producers, finish)
    }

    fn add_catch_in_frame(&mut self, from: NodeId, finish: CatchFinish<'a>) -> NodeId {
        self.sched.add_catch_in_frame(from, finish)
    }

    fn current_scope(&self) -> &Scope<'a> {
        self.sched.current_scope()
    }

    fn add_dispatch_here(&mut self, expr: KExpression<'a>) -> NodeId {
        self.sched.add_dispatch_here(expr)
    }

    fn add_combine_here(
        &mut self,
        owned_subs: Vec<NodeId>,
        park_producers: Vec<NodeId>,
        finish: CombineFinish<'a>,
    ) -> NodeId {
        self.sched
            .add_combine_here(owned_subs, park_producers, finish)
    }

    fn add_catch_here(&mut self, from: NodeId, finish: CatchFinish<'a>) -> NodeId {
        self.sched.add_catch_here(from, finish)
    }
}
