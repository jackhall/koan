use std::rc::Rc;

use crate::machine::model::KObject;
use crate::machine::{
    CallArena, CatchFinish, CombineFinish, KError, LexicalFrame, NodeId, Scope, SchedulerHandle,
};
use crate::machine::core::ScopeId;
use crate::machine::model::ast::KExpression;

use super::nodes::NodeWork;
use dep_graph::DepGraph;
use node_store::NodeStore;
use work_queues::WorkQueues;

mod dep_graph;
mod dispatch;
/// Carrier shape ridden by every `NodeWork::Dispatch`. Visible up to
/// `crate::machine::execute` because `nodes.rs` (a sibling of
/// `scheduler.rs`) names `DispatchState` in `NodeWork::Dispatch`.
pub(in crate::machine::execute) mod dispatch_state;
mod execute;
mod finish;
mod literal;
mod node_store;
mod submit;
mod work_queues;
#[cfg(test)]
mod run_tests;
#[cfg(test)]
mod tests;

/// A dynamic DAG of dispatch and execution work. The parser submits `Dispatch` nodes for each
/// top-level expression; running a `Dispatch` may add child `Dispatch`/`Bind`/`Combine`
/// nodes, and builtin bodies holding `&mut dyn SchedulerHandle` can also add `Dispatch` nodes.
///
/// The execute loop drains work via [`WorkQueues::pop_next`], which prioritizes in-flight
/// slots (sub-work spawned during another slot's run, plus consumers woken by the
/// notify-walk when a producer's terminal write decrements `pending_deps` to zero) ahead
/// of fresh top-level dispatches (submission order). Owned edges never cycle — a new
/// node's `NodeId` is strictly greater than every node it owns. Park (`Notify`) edges
/// can point at an earlier producer, so a self-referential binding (`LET x = x`, whose
/// RHS sub-dispatch parks on the binder's own placeholder) forms a cycle: the queues
/// drain with both slots still `PreRun`. `execute` detects the leftover parked slots
/// and returns `KErrorKind::SchedulerDeadlock` rather than letting the top-level read
/// panic on an unresolved slot.
///
/// Each node carries the scope it should run against (`Node::scope`). Sub-nodes default to
/// the spawning node's scope; user-fn invocation installs a per-call child scope via
/// `NodeStep::Replace`.
///
/// See design/execution-model.md and design/memory-model.md.
pub struct Scheduler<'a> {
    /// Routing + priority wrapper over the `fresh` and `in_flight` bands. All push/pop
    /// sites go through [`WorkQueues`]'s five named entry points so the routing arm and
    /// drain priority are enforced by the type rather than restated at each call site.
    /// Scoped to `scheduler/` (matches `WorkQueues`'s `pub(super)`); no caller outside
    /// this module touches it.
    pub(in crate::machine::execute::scheduler) queues: WorkQueues,
    /// Tri-vector dependency state (forward notify edges, pending-deps counters,
    /// backward Owned/Notify edges) bundled behind an enforced surface that
    /// keeps the three vectors in lockstep. See `dep_graph.rs` for the
    /// invariants and the small set of mutation entry points.
    pub(in crate::machine::execute::scheduler) deps: DepGraph,
    /// Slot table — `nodes`, `results`, `free_list` bundled behind a surface
    /// that keeps the three vectors in lockstep across `alloc_slot ->
    /// take_for_run -> reinstall* -> finalize -> free_one`. See
    /// `node_store.rs` for the invariants and the small set of mutation
    /// entry points. Scope matches `deps` and `queues`; `Scheduler::finalize`
    /// reaches `store.stamp_lift_ready` from a sibling submodule to transition
    /// `NodeWork::Lift(Pending → Ready)` at notify-walk time.
    pub(in crate::machine::execute::scheduler) store: NodeStore<'a>,
    /// Frame Rc of the slot currently being executed. Read via `SchedulerHandle::current_frame`
    /// so frame-creating builtins (MATCH) can chain it onto their new frame; see
    /// [memory-model.md § Per-call-frame chaining](../../../design/memory-model.md#per-call-frame-chaining-for-builtin-built-frames).
    pub(in crate::machine::execute::scheduler) active_frame: Option<Rc<CallArena>>,
    /// Lexical chain of the slot currently executing. Mirrors `active_frame`'s
    /// save/restore pattern. `Scheduler::add` reads this to attach a chain to every
    /// sub-slot that doesn't carry an explicit `enter_block` chain — that's how
    /// internal binder sub-dispatches (CONS-head, FN signature subs, NEWTYPE value
    /// sub, USING-body) inherit the parent's chain without each call site naming it.
    pub(in crate::machine::execute::scheduler) active_chain: Option<Rc<LexicalFrame>>,
    /// Count of tail-reuse opportunities accepted by
    /// `try_take_reusable_frame_for_tail`. Test-only observable; the production
    /// path returns `Some`/`None` without touching this field's gate.
    #[cfg(test)]
    pub(in crate::machine::execute::scheduler) tail_reuse_count: usize,
}

/// RAII-shaped save/restore wrapper around the per-step `active_frame` and
/// `active_chain` swap that brackets each iteration of [`Scheduler::execute`].
///
/// `enter_slot_step` installs the running slot's `frame` and `chain` into the
/// scheduler's ambient slots, parking the previous values in the guard.
/// `exit_slot_step` mem-replaces the originals back in and hands the caller the
/// post-step frame (which may differ from the entered frame if the step took it
/// via `try_take_reusable_frame_for_tail`).
///
/// This is the bookkeeping spine the ping-pong reserve frame will extend (see
/// `roadmap/dispatch_fix/ping-pong-reserve-frame.md`); a future PR adds
/// `prev_reserve` here and threads it through the same enter/exit pair.
pub(in crate::machine::execute::scheduler) struct SlotStepGuard {
    prev_frame: Option<Rc<CallArena>>,
    prev_chain: Option<Rc<LexicalFrame>>,
}

impl<'a> Scheduler<'a> {
    /// Install `node_frame` as `active_frame` and `node_chain` as `active_chain`
    /// for the duration of one slot's step. Returns a guard the caller must
    /// hand to [`Scheduler::exit_slot_step`] when the step returns. The
    /// `node_chain` Rc is cloned at exactly one point (here) — the caller may
    /// retain its own clone for the Replace arm without bumping the count a
    /// second time.
    pub(in crate::machine::execute::scheduler) fn enter_slot_step(
        &mut self,
        node_frame: Option<Rc<CallArena>>,
        node_chain: Rc<LexicalFrame>,
    ) -> SlotStepGuard {
        let prev_frame = std::mem::replace(&mut self.active_frame, node_frame);
        let prev_chain = self.active_chain.replace(node_chain);
        SlotStepGuard { prev_frame, prev_chain }
    }

    /// Restore the previous `active_frame`/`active_chain` saved by
    /// [`Scheduler::enter_slot_step`] and return the post-step frame. The
    /// returned `Option<Rc<CallArena>>` is `Some(frame)` if the step left the
    /// node's frame intact (Done with a frame, or Replace that didn't consume
    /// it via tail-reuse) and `None` if the step took it via
    /// `try_take_reusable_frame_for_tail` (tail-reuse path). Callers read it
    /// to drive Done's lift/finalize and Replace's rotation.
    pub(in crate::machine::execute::scheduler) fn exit_slot_step(
        &mut self,
        guard: SlotStepGuard,
    ) -> Option<Rc<CallArena>> {
        let post_step_frame = std::mem::replace(&mut self.active_frame, guard.prev_frame);
        self.active_chain = guard.prev_chain;
        post_step_frame
    }

    pub fn new() -> Self {
        Self {
            queues: WorkQueues::new(),
            deps: DepGraph::new(),
            store: NodeStore::new(),
            active_frame: None,
            active_chain: None,
            #[cfg(test)]
            tail_reuse_count: 0,
        }
    }

    #[cfg(test)]
    pub fn tail_reuse_count(&self) -> usize { self.tail_reuse_count }

    /// Test-only chain peek. Returns the `LexicalFrame` chain attached to slot
    /// `id`. Only valid before the slot terminalizes — once a slot is `Done` the
    /// payload (and its chain) has been moved out by `take_for_run`.
    #[cfg(test)]
    pub fn chain_of(&self, id: NodeId) -> Option<Rc<LexicalFrame>> {
        self.store.chain_of(id)
    }

    pub fn len(&self) -> usize { self.store.len() }
    pub fn is_empty(&self) -> bool { self.store.is_empty() }

    /// True iff slot `id` holds a terminal result. An errored sub counts as ready — the
    /// parent short-circuits on it in `run_bind`/`run_combine`.
    pub(in crate::machine::execute::scheduler) fn is_result_ready(&self, id: NodeId) -> bool {
        self.store.is_result_ready(id)
    }

    /// Retrieve the resolved result for a top-level dispatch. Only safe on IDs returned by
    /// `add_dispatch`; internal slots may have been eagerly freed by their parent.
    pub fn read_result(&self, id: NodeId) -> Result<&'a KObject<'a>, &KError> {
        self.store.read_result(id)
    }

    /// Convenience wrapper for the value-only path: panics on `Err`.
    pub fn read(&self, id: NodeId) -> &'a KObject<'a> {
        self.store.read(id)
    }
}

impl<'a> Default for Scheduler<'a> {
    fn default() -> Self { Self::new() }
}

impl<'a> SchedulerHandle<'a> for Scheduler<'a> {
    fn add_dispatch(&mut self, expr: KExpression<'a>, scope: &'a Scope<'a>) -> NodeId {
        // Delegate to the inherent method so both `sched.add_dispatch(...)` (direct
        // struct call from test harnesses / the interpret driver) and the trait
        // method (sub-dispatches from inside a builtin body) share the same
        // auto-root behavior for top-level submissions and ambient-inherit for
        // sub-dispatches.
        Scheduler::add_dispatch(self, expr, scope)
    }

    fn add_combine(
        &mut self,
        owned_subs: Vec<NodeId>,
        park_producers: Vec<NodeId>,
        scope: &'a Scope<'a>,
        finish: CombineFinish<'a>,
    ) -> NodeId {
        Scheduler::add_combine(self, owned_subs, park_producers, scope, finish)
    }

    fn add_catch(
        &mut self,
        from: NodeId,
        scope: &'a Scope<'a>,
        finish: CatchFinish<'a>,
    ) -> NodeId {
        Scheduler::add_catch(self, from, scope, finish)
    }

    /// Active slot's frame `Rc<CallArena>`, set by `execute` for the duration of each
    /// slot's run. Frame-creating builtins (MATCH) clone this Rc into the new frame so the
    /// call-site arena stays alive while the new frame is in use.
    fn current_frame(&self) -> Option<Rc<CallArena>> {
        self.active_frame.clone()
    }

    /// Temporarily install `frame` as the active frame while running `body`. Sub-slots
    /// spawned inside `body` inherit `frame` via the `Scheduler::add` site that reads
    /// `self.active_frame`. The previous `active_frame` is saved and restored on return,
    /// so the caller's slot-tracking invariant survives unchanged.
    fn with_active_frame(
        &mut self,
        frame: std::rc::Rc<crate::machine::core::CallArena>,
        body: &mut dyn FnMut(&mut dyn SchedulerHandle<'a>),
    ) {
        let prev = self.active_frame.take();
        self.active_frame = Some(frame);
        body(self);
        self.active_frame = prev;
    }

    /// Take the active frame iff it is uniquely owned. Because `execute` moves the
    /// slot's frame directly into `self.active_frame` (no clone — see the
    /// `mem::replace` pair in `execute.rs`), uniqueness here is exactly the
    /// "no escape" condition: any cloned `Rc` would have bumped strong_count past 1.
    fn try_take_reusable_frame_for_tail(&mut self) -> Option<Rc<CallArena>> {
        let candidate = self.active_frame.take()?;
        if Rc::strong_count(&candidate) == 1 && Rc::weak_count(&candidate) == 0 {
            #[cfg(test)]
            { self.tail_reuse_count += 1; }
            Some(candidate)
        } else {
            self.active_frame = Some(candidate);
            None
        }
    }

    fn current_lexical_chain(&self) -> Option<Rc<LexicalFrame>> {
        self.active_chain.clone()
    }

    fn enter_block(
        &mut self,
        scope_id: ScopeId,
        statements: Vec<KExpression<'a>>,
        scope: &'a Scope<'a>,
    ) -> Vec<NodeId> {
        let parent = self.active_chain.clone();
        // Statement indices start at 1 (not 0): the visibility predicate is strict
        // less-than (`b.idx < c`), and builtins sit at `idx = 0`. A top-level user
        // statement at index 1 has cutoff 1, so `0 < 1` makes builtins visible.
        // Indices reset per `enter_block` call; a REPL / test-fixture submission
        // against a scope already holding bindings goes through the detached
        // auto-root path in `add` instead (no ambient chain), which makes those
        // bindings visible via the `index_for → None ⇒ complete` arm.
        statements
            .into_iter()
            .enumerate()
            .map(|(i, expr)| {
                let chain = LexicalFrame::push(parent.clone(), scope_id, i + 1);
                self.add_dispatch_with_chain(expr, scope, chain)
            })
            .collect()
    }

    fn add_dispatch_with_chain(
        &mut self,
        expr: KExpression<'a>,
        scope: &'a Scope<'a>,
        chain: Rc<LexicalFrame>,
    ) -> NodeId {
        Scheduler::add_with_chain(self, NodeWork::dispatch(expr), scope, Some(chain))
    }
}
