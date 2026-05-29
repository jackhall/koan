use crate::machine::model::{KObject, Parseable};
use crate::machine::{
    BodyResult, CatchFinish, CombineFinish, Frame, KError, KErrorKind, KFuture, NodeId,
    ResolveOutcome, Scope,
};
use crate::machine::model::ast::{ExpressionPart, KExpression};

use super::dispatch::propagate_dep_error;
use super::super::nodes::{LiftState, NodeOutput, NodeStep, NodeWork};
use super::Scheduler;

impl<'a> Scheduler<'a> {
    pub(super) fn run_bind(
        &mut self,
        mut expr: KExpression<'a>,
        subs: Vec<(usize, NodeId)>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        // Sub slots stay in `dep_edges[idx]` on the error path so chain-free at
        // finalize reclaims them; eager free is the success-path optimization.
        for (_, dep_id) in &subs {
            if let Err(e) = self.read_result(*dep_id) {
                let frame = Frame::from_expr("<bind>", &expr);
                return Ok(NodeStep::Done(NodeOutput::Err(
                    propagate_dep_error(e, Some(frame)),
                )));
            }
        }
        let dep_indices: Vec<usize> = subs.iter().map(|(_, d)| d.index()).collect();
        for (part_idx, dep_id) in subs {
            let value = self.read(dep_id);
            expr.parts[part_idx].value = ExpressionPart::Future(value);
        }
        // Spliced `Future(&'a KObject)` references survive `results[dep] = None`
        // because the objects live in arenas tied to lexical scope. Reclaim happens
        // before resolution so the dispatched body's `add()` calls can recycle the
        // indices immediately.
        self.reclaim_deps(idx, dep_indices);
        let future = match scope.resolve_dispatch(&expr) {
            ResolveOutcome::Resolved(r) => r.function.bind(expr)?,
            ResolveOutcome::Ambiguous(n) => {
                return Err(KError::new(KErrorKind::AmbiguousDispatch {
                    expr: expr.summarize(),
                    candidates: n,
                }));
            }
            // `Deferred` shouldn't reach here: every Expression/ListLiteral/DictLiteral
            // part was scheduled before this Bind, and after reclaim the slots now hold
            // `Future(_)`. Treat as a defensive dispatch failure rather than re-entering
            // the eager loop (which would risk a no-progress cycle).
            ResolveOutcome::Deferred | ResolveOutcome::Unmatched => {
                return Err(KError::new(KErrorKind::DispatchFailed {
                    expr: expr.summarize(),
                    reason: "no matching function".to_string(),
                }));
            }
            // A bare name in the rebuilt expression is still a pending forward
            // reference: route through the stateful overload-park track installer
            // so the resume rebuilds via `stateful_keyworded_initial` rather than
            // the deleted legacy re-Dispatch path.
            ResolveOutcome::ParkOnProducers(producers) => {
                // `Bind` deps resolved; re-Dispatch carries no `pre_subs` (the
                // recursive-submission optimization fires only at the original
                // outermost `add_with_chain`, not at bind-time re-resolve).
                return Ok(self.stateful_install_overload_park(producers, expr, Vec::new(), idx));
            }
            ResolveOutcome::UnboundName(name) => {
                return Err(KError::new(KErrorKind::UnboundName(name)));
            }
        };
        Ok(self.invoke_to_step(future, scope, idx))
    }

    /// Success-path eager free; the error path leaves deps for chain-free at slot drop.
    /// `dep_edges[idx].clear()` is sound here: Bind / Combine slots at reclaim time hold
    /// only `Owned` edges (their `subs` / `deps`, all spawned by this slot). Notify
    /// edges land only on Dispatch slots via the bare-name short-circuit / replay-park
    /// in `run_dispatch`, never on Bind /
    /// Combine, so clearing the list cannot drop a wake intent.
    fn reclaim_deps(&mut self, idx: usize, dep_indices: Vec<usize>) {
        self.deps.clear_dep_edges(idx);
        for d in dep_indices {
            self.free(d);
        }
    }

    /// Run a `Combine` slot: short-circuit on the first errored dep using the same
    /// frame-attached propagation as `run_bind`, then call `finish` over the dep values
    /// and decode the returned `BodyResult` (Value, Tail, or Err) into a `NodeStep`
    /// using the same dispatch as `invoke_to_step`. Only the `deps[park_count..]`
    /// owned-sub suffix is eagerly freed on the success path; the `[..park_count]`
    /// park-producer prefix is kept alive (those slots are sibling producers the
    /// Combine merely read at finish-time). The error path leaves the Combine's
    /// edges in `dep_edges[idx]` for chain-free at slot drop.
    pub(super) fn run_combine(
        &mut self,
        deps: Vec<NodeId>,
        park_count: usize,
        finish: CombineFinish<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> NodeStep<'a> {
        // The closure carries its own framing context (e.g. "<list>", "<dict>") via its
        // capture; the Combine machinery only handles dep-error propagation, which uses
        // the generic "<combine>" frame to match `run_bind`'s "<bind>" convention.
        let make_frame = || Frame::bare("<combine>", "combine");
        for dep in &deps {
            if let Err(e) = self.read_result(*dep) {
                return NodeStep::Done(NodeOutput::Err(
                    propagate_dep_error(e, Some(make_frame())),
                ));
            }
        }
        // Pre-collect refs so `finish` (which holds `&mut self` via the trait object)
        // doesn't reborrow `self` for reads.
        let values: Vec<&'a KObject<'a>> = deps.iter().map(|d| self.read(*d)).collect();
        let owned_indices: Vec<usize> =
            deps[park_count..].iter().map(|d| d.index()).collect();
        let body = finish(scope, self, &values);
        self.reclaim_deps(idx, owned_indices);
        match body {
            BodyResult::Value(v) => NodeStep::Done(NodeOutput::Value(v)),
            BodyResult::Tail { expr, frame, function, block_entry, body_index } => {
                NodeStep::Replace {
                    work: NodeWork::dispatch(expr),
                    frame,
                    function,
                    block_entry,
                    body_index,
                }
            }
            BodyResult::DeferTo(id) => self.defer_to_lift(idx, id),
            BodyResult::Err(e) => NodeStep::Done(NodeOutput::Err(e)),
        }
    }

    /// Run a `Catch` slot: read `from`'s terminal as a `Result`, hand it to `finish`, and
    /// decode the returned `BodyResult` the same way `run_combine` does. Unlike Combine,
    /// an errored `from` does not short-circuit; the finish closure decides whether to
    /// recover (TRY-WITH's per-arm dispatch) or re-raise. `from` is freed on both paths.
    pub(super) fn run_catch(
        &mut self,
        from: NodeId,
        finish: CatchFinish<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> NodeStep<'a> {
        let result: Result<&'a KObject<'a>, KError> = match self.read_result(from) {
            Ok(v) => Ok(v),
            // Frameless propagation: TRY-WITH's per-arm dispatch reads this `Err` and
            // attaches its own frame at the recovery site, so adding one here would
            // double-frame the error.
            Err(e) => Err(propagate_dep_error(e, None)),
        };
        let body = finish(scope, self, result);
        self.reclaim_deps(idx, vec![from.index()]);
        match body {
            BodyResult::Value(v) => NodeStep::Done(NodeOutput::Value(v)),
            BodyResult::Tail { expr, frame, function, block_entry, body_index } => {
                NodeStep::Replace {
                    work: NodeWork::dispatch(expr),
                    frame,
                    function,
                    block_entry,
                    body_index,
                }
            }
            BodyResult::DeferTo(id) => self.defer_to_lift(idx, id),
            BodyResult::Err(e) => NodeStep::Done(NodeOutput::Err(e)),
        }
    }

    /// Consume the stamped Lift state. By the time the slot pops, the notify-walk
    /// in `Scheduler::finalize` has transitioned `Pending → Ready`, so this match
    /// performs no result-table lookup and the `&KObject<'a>` inside `Value` is the
    /// same reference the producer wrote — not a clone. The execute loop's Done arm
    /// handles frame-aware deep-cloning into the outer arena.
    ///
    /// The `Pending` arm is a wake-misfire panic that localizes to the notify graph:
    /// reaching it means a Lift slot was enqueued without its `from` finalizing,
    /// which would indicate a bug in `Scheduler::finalize`'s stamp or `DepGraph`'s
    /// pending-deps accounting — not in any read-side caller.
    pub(super) fn run_lift(state: LiftState<'a>) -> NodeOutput<'a> {
        match state {
            LiftState::Ready(output) => output,
            LiftState::Pending(_) => panic!(
                "scheduler invariant: notify-walk must stamp Lift to Ready before enqueue",
            ),
        }
    }

    /// `BodyResult::Tail` rewrites the current slot's work in place — this is what gives
    /// recursion constant scheduler memory. `BodyResult::DeferTo(id)` rewrites to a Lift
    /// off `id`, so the slot's terminal becomes whatever `id` produces; matches
    /// `defer_to_lift`'s post-Bind shape but for body-driven combinator planning (MODULE
    /// and SIG body wrap-up via `add_combine`).
    ///
    /// `idx` is the executing slot. Needed so the `DeferTo` arm can install an
    /// `Owned` edge for `id` via `defer_to_lift` (which calls `DepGraph::add_owned_edge`)
    /// before returning the `Replace` — without that install, the Replace gate's
    /// `pending_count(idx)` read sees zero and re-enqueues the Lift before the
    /// producer runs.
    pub(super) fn invoke_to_step(
        &mut self,
        future: KFuture<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> NodeStep<'a> {
        match future.function.invoke(scope, self, future.bundle) {
            BodyResult::Value(v) => NodeStep::Done(NodeOutput::Value(v)),
            BodyResult::Tail { expr, frame, function, block_entry, body_index } => {
                NodeStep::Replace {
                    work: NodeWork::dispatch(expr),
                    frame,
                    function,
                    block_entry,
                    body_index,
                }
            }
            BodyResult::DeferTo(id) => self.defer_to_lift(idx, id),
            BodyResult::Err(e) => NodeStep::Done(NodeOutput::Err(e)),
        }
    }

    /// `invoke_to_step` with the slot's reserve frame consumed when available,
    /// falling back to the pin-only shape otherwise. Used by the stateful
    /// resume / install-time short-circuit sites where the dispatch slot holds
    /// the only `Rc<CallArena>` for the arena `scope` lives in.
    ///
    /// **Reserve-consuming arm** (`Some` reserve): the per-slot reserve was
    /// rotated in two iterations ago by the Replace arm in `execute.rs`, so
    /// its scope is past every live tree-borrows protector. The helper:
    ///
    /// 1. Pins `self.active_frame` (the slot's current frame) via a local
    ///    clone — this keeps `scope` alive across the invoke.
    /// 2. Swaps the reserve into `self.active_frame`. The reserve was uniquely
    ///    held by `active_reserve` (`SchedulerHandle::current_frame` returns
    ///    `active_frame`, never `active_reserve`; the only other Rc was the
    ///    `slot.reserve_frame` field, drained by `take_for_run` and routed
    ///    through `enter_slot_step`), so `strong_count == 1` on the now-active
    ///    reserve.
    /// 3. Calls `invoke_to_step`. Inside,
    ///    `try_take_reusable_frame_for_tail`'s uniqueness check succeeds on
    ///    the reserve, the reset lands, and the body runs in the reset arena.
    /// 4. Restores `self.active_frame = local_pin` so the post-step swap in
    ///    `execute.rs` sees the slot's frame and can rotate it into the next
    ///    iteration's reserve.
    ///
    /// **Pin-only arm** (`None` reserve, first or second iteration): clones
    /// `self.active_frame` for the duration of the invoke. Without the pin,
    /// `KFunction::invoke` would successfully take the frame for tail-reuse
    /// and `try_reset_for_tail` would deallocate the arena while `scope`'s
    /// tree-borrows protector is still live (UB). The pin keeps
    /// `strong_count >= 2` across the invoke, foreclosing the tail-reuse on
    /// the slot's only frame Rc.
    ///
    /// See `roadmap/dispatch_fix/ping-pong-reserve-frame.md` for the rotation
    /// design and `recursive_tagged_match_no_uaf` for the Miri witness.
    pub(super) fn invoke_to_step_pinned(
        &mut self,
        future: KFuture<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> NodeStep<'a> {
        if let Some(reserve) = self.active_reserve.take() {
            // `local_pin` anchors the slot's frame (and therefore `scope`)
            // across the invoke; the reserve takes its place as `active_frame`
            // so tail-reuse consumes the reserve, not the slot's only frame
            // Rc. Restored after the invoke so the post-step swap in
            // `execute.rs` reads `active_frame == slot.frame` and can rotate
            // for the next iteration.
            let local_pin = self.active_frame.clone();
            self.active_frame = Some(reserve);
            let step = self.invoke_to_step(future, scope, idx);
            self.active_frame = local_pin;
            step
        } else {
            let _frame_pin = self.active_frame.clone();
            self.invoke_to_step(future, scope, idx)
        }
    }
}
