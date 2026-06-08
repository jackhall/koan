use std::rc::Rc;

use crate::machine::core::ScopeId;
use crate::machine::model::ast::KExpression;
use crate::machine::model::Carried;
use crate::machine::{
    CallArena, CatchFinish, CombineFinish, KError, LexicalFrame, NodeId, SchedulerHandle, Scope,
};

use super::nodes::{NodeScope, NodeWork};
use dep_graph::DepGraph;
use node_store::NodeStore;
use work_queues::WorkQueues;

mod dep_graph;
mod execute;
mod finish;
mod literal;
mod node_store;
#[cfg(test)]
mod run_tests;
mod submit;
#[cfg(test)]
mod tests;
mod work_queues;

/// A dynamic DAG of dispatch and execution work.
///
/// The execute loop drains via [`WorkQueues::pop_next`], which prioritizes in-flight slots
/// (sub-work and notify-walk wakeups) ahead of fresh top-level dispatches. Owned edges never
/// cycle — a new node's `NodeId` is strictly greater than every node it owns. Park (`Notify`)
/// edges can point at an earlier producer, so a self-referential binding (`LET x = x`) forms
/// a cycle that drains with both slots still `PreRun`; `execute` detects the leftover parked
/// slots and returns `KErrorKind::SchedulerDeadlock`.
///
/// Each node carries the scope it runs against (`Node::scope`). Sub-nodes default to the
/// spawning node's scope; user-fn invocation installs a per-call child scope via
/// `NodeStep::Replace`.
///
/// See design/execution-model.md and design/memory-model.md.
pub struct Scheduler<'a> {
    pub(in crate::machine::execute::scheduler) queues: WorkQueues,
    pub(in crate::machine::execute::scheduler) deps: DepGraph,
    pub(in crate::machine::execute::scheduler) store: NodeStore<'a>,
    /// Frame Rc of the slot currently being executed. See
    /// [per-call-arena-protocol.md § Active-frame propagation](../../../design/per-call-arena-protocol.md#active-frame-propagation).
    pub(in crate::machine::execute::scheduler) active_frame: Option<Rc<CallArena>>,
    /// Lexical chain of the slot currently executing. `Scheduler::add` reads this to attach
    /// a chain to every sub-slot that doesn't carry an explicit `enter_block` chain, so
    /// internal binder sub-dispatches inherit the parent's chain implicitly.
    pub(in crate::machine::execute::scheduler) active_chain: Option<Rc<LexicalFrame>>,
    /// Per-slot reserve frame for the running step. `None` between slot steps. See
    /// [per-call-arena-protocol.md § Ping-pong reserve frame](../../../design/per-call-arena-protocol.md#ping-pong-reserve-frame).
    pub(in crate::machine::execute::scheduler) active_reserve: Option<Rc<CallArena>>,
    #[cfg(test)]
    pub(in crate::machine::execute::scheduler) tail_reuse_count: usize,
}

/// RAII-shaped save/restore wrapper around the per-step `active_frame`, `active_chain`,
/// and `active_reserve` swap that brackets each iteration of [`Scheduler::execute`].
/// Bookkeeping spine for the ping-pong reserve-frame rotation; see
/// [per-call-arena-protocol.md § Ping-pong reserve frame](../../../design/per-call-arena-protocol.md#ping-pong-reserve-frame).
pub(in crate::machine::execute::scheduler) struct SlotStepGuard {
    prev_frame: Option<Rc<CallArena>>,
    prev_chain: Option<Rc<LexicalFrame>>,
    /// Saved so nested slot runs (combinator finish closures) don't inherit the
    /// outer slot's reserve frame.
    prev_reserve: Option<Rc<CallArena>>,
}

impl<'a> Scheduler<'a> {
    /// Install the slot's frame/chain/reserve as the ambient values for one step. The
    /// caller passes the returned guard to [`Scheduler::exit_slot_step`] when the step
    /// returns; the `node_chain` Rc is cloned only here, so the caller's own clone for
    /// the Replace arm doesn't double-count.
    pub(in crate::machine::execute::scheduler) fn enter_slot_step(
        &mut self,
        node_frame: Option<Rc<CallArena>>,
        node_reserve: Option<Rc<CallArena>>,
        node_chain: Rc<LexicalFrame>,
    ) -> SlotStepGuard {
        let prev_frame = std::mem::replace(&mut self.active_frame, node_frame);
        let prev_chain = self.active_chain.replace(node_chain);
        let prev_reserve = std::mem::replace(&mut self.active_reserve, node_reserve);
        SlotStepGuard {
            prev_frame,
            prev_chain,
            prev_reserve,
        }
    }

    /// Restore the values saved by [`Scheduler::enter_slot_step`] and return
    /// `(post_step_frame, post_step_reserve)`.
    ///
    /// `post_step_frame` is `None` if the step took the frame via
    /// `try_take_reusable_frame_for_tail`, else the slot's frame.
    /// `post_step_reserve` is normally `None` (consumed by `invoke_to_step_pinned`) but
    /// carries through when the step didn't run an invoke. The Replace arm reads it to
    /// decide rotation: with a new frame, the post-step reserve is two iterations old
    /// and gets dropped; without one, it rides along on the reinstalled node.
    pub(in crate::machine::execute::scheduler) fn exit_slot_step(
        &mut self,
        guard: SlotStepGuard,
    ) -> (Option<Rc<CallArena>>, Option<Rc<CallArena>>) {
        let post_step_frame = std::mem::replace(&mut self.active_frame, guard.prev_frame);
        self.active_chain = guard.prev_chain;
        let post_step_reserve = std::mem::replace(&mut self.active_reserve, guard.prev_reserve);
        (post_step_frame, post_step_reserve)
    }

    pub fn new() -> Self {
        Self {
            queues: WorkQueues::new(),
            deps: DepGraph::new(),
            store: NodeStore::new(),
            active_frame: None,
            active_chain: None,
            active_reserve: None,
            #[cfg(test)]
            tail_reuse_count: 0,
        }
    }

    #[cfg(test)]
    pub fn tail_reuse_count(&self) -> usize {
        self.tail_reuse_count
    }

    /// Only valid before the slot terminalizes — once a slot is `Done` the payload
    /// (and its chain) has been moved out by `take_for_run`.
    #[cfg(test)]
    pub fn chain_of(&self, id: NodeId) -> Option<Rc<LexicalFrame>> {
        self.store.chain_of(id)
    }

    pub fn len(&self) -> usize {
        self.store.len()
    }
    pub fn is_empty(&self) -> bool {
        self.store.is_empty()
    }

    /// An errored sub counts as ready — parents short-circuit on it.
    pub(in crate::machine::execute) fn is_result_ready(&self, id: NodeId) -> bool {
        self.store.is_result_ready(id)
    }

    /// Only safe on IDs returned by `add_dispatch`; internal slots may have been eagerly
    /// freed by their parent.
    pub fn read_result(&self, id: NodeId) -> Result<Carried<'a>, &KError> {
        self.store.read_result(id)
    }

    /// Panics on `Err`.
    pub fn read(&self, id: NodeId) -> Carried<'a> {
        self.store.read(id)
    }

    // ----- Narrow dispatcher-facing surface (pub(in execute)) -----
    //
    // These methods are the dispatcher's named contract with the scheduler:
    // every `DispatchCtx` touch routes through one of them, so the storage
    // layout (`deps` / `store` / `queues` / `active_*` fields) stays
    // scheduler-internal. Order mirrors `DispatchCtx`'s method groups in
    // `dispatch/ctx.rs` for cross-reference.

    /// Atomic +1 on the consumer's pending count, edges list, and the
    /// producer's notify list (`DepGraph::add_park_edge`).
    pub(in crate::machine::execute) fn add_park_edge(
        &mut self,
        producer: NodeId,
        consumer: NodeId,
    ) {
        self.deps.add_park_edge(producer, consumer);
    }

    /// Atomic +1 on the consumer's pending count, edges list, and the
    /// producer's notify list, recording the edge as `Owned` so reclaim
    /// recurses through it (`DepGraph::add_owned_edge`).
    pub(in crate::machine::execute) fn add_owned_edge(
        &mut self,
        producer: NodeId,
        consumer: NodeId,
    ) {
        self.deps.add_owned_edge(producer, consumer);
    }

    /// True iff `producer` is forward-reachable from `consumer`
    /// (`DepGraph::would_create_cycle`).
    pub(in crate::machine::execute) fn would_create_cycle(
        &self,
        producer: NodeId,
        consumer: NodeId,
    ) -> bool {
        self.deps.would_create_cycle(producer, consumer)
    }

    /// Success-path eager free for the slot's owned edges
    /// (`DepGraph::clear_dep_edges`).
    pub(in crate::machine::execute) fn clear_dep_edges(&mut self, idx: usize) {
        self.deps.clear_dep_edges(idx);
    }

    /// Drain producers that fired since this slot's last poll
    /// (`NodeStore::take_recent_wakes`).
    pub(in crate::machine::execute) fn take_recent_wakes(
        &mut self,
        consumer: NodeId,
    ) -> Vec<NodeId> {
        self.store.take_recent_wakes(consumer)
    }

    /// Borrow the ambient lexical chain (`&self.active_chain.as_deref()`).
    /// Name-resolution helpers read this to apply chain-aware visibility.
    pub(in crate::machine::execute) fn chain_deref(&self) -> Option<&LexicalFrame> {
        self.active_chain.as_deref()
    }

    /// Cloned `Rc` to the ambient lexical chain. Used by initial-resolve
    /// sites that capture the chain's `index` for `BindingIndex`.
    pub(in crate::machine::execute) fn active_chain_clone(&self) -> Option<Rc<LexicalFrame>> {
        self.active_chain.clone()
    }

    /// Cloned `Rc` to the ambient per-call frame, for the local-pin guard
    /// in `invoke_to_step_pinned`. See
    /// [per-call-arena-protocol.md § Ping-pong reserve frame](../../../design/per-call-arena-protocol.md#ping-pong-reserve-frame).
    pub(in crate::machine::execute) fn active_frame_clone(&self) -> Option<Rc<CallArena>> {
        self.active_frame.clone()
    }

    /// Take the per-slot reserve frame. Pairs with
    /// [`Scheduler::active_frame_replace`] in the pin/swap pattern that
    /// installs `reserve` as the ambient frame for one nested invoke.
    pub(in crate::machine::execute) fn active_reserve_take(&mut self) -> Option<Rc<CallArena>> {
        self.active_reserve.take()
    }

    /// Replace the ambient `active_frame` with `new`, returning the prior
    /// value. The `with_active_frame` body bracket and the
    /// `invoke_to_step_pinned` pin/swap both go through this.
    pub(in crate::machine::execute) fn active_frame_replace(
        &mut self,
        new: Option<Rc<CallArena>>,
    ) -> Option<Rc<CallArena>> {
        std::mem::replace(&mut self.active_frame, new)
    }
}

impl<'a> Default for Scheduler<'a> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a, 's> SchedulerHandle<'a, 's> for Scheduler<'a> {
    fn add_dispatch(&mut self, expr: KExpression<'a>, scope: &'a Scope<'a>) -> NodeId {
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

    fn add_catch(&mut self, from: NodeId, scope: &'a Scope<'a>, finish: CatchFinish<'a>) -> NodeId {
        Scheduler::add_catch(self, from, scope, finish)
    }

    fn current_frame(&self) -> Option<Rc<CallArena>> {
        self.active_frame.clone()
    }

    /// Sub-slots spawned inside `body` inherit `frame` via the `Scheduler::add` site
    /// that reads `self.active_frame`.
    fn with_active_frame(
        &mut self,
        frame: std::rc::Rc<crate::machine::core::CallArena>,
        body: &mut dyn FnMut(&mut dyn SchedulerHandle<'a, 's>),
    ) {
        let prev = self.active_frame.take();
        self.active_frame = Some(frame);
        body(self);
        self.active_frame = prev;
    }

    /// Take the active frame iff it is uniquely owned. `execute` moves the slot's frame
    /// directly into `self.active_frame` (no clone), so uniqueness here is exactly the
    /// "no escape" condition — any cloned `Rc` would have bumped strong_count past 1.
    fn try_take_reusable_frame_for_tail(&mut self) -> Option<Rc<CallArena>> {
        let candidate = self.active_frame.take()?;
        if Rc::strong_count(&candidate) == 1 && Rc::weak_count(&candidate) == 0 {
            #[cfg(test)]
            {
                self.tail_reuse_count += 1;
            }
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
        // Indices start at 1: visibility is strict less-than and builtins sit at idx 0,
        // so a top-level statement at index 1 sees them via `0 < 1`.
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

    fn add_dispatch_with_chain_in_frame(
        &mut self,
        expr: KExpression<'a>,
        chain: Rc<LexicalFrame>,
    ) -> NodeId {
        let frame = self
            .active_frame
            .clone()
            .expect("in-frame dispatch requires an active frame");
        // `scope_for_bind` is `Rc`-bounded — not the `anchored_parts` `'a`-fabrication. The
        // slot stores `Yoked` and re-projects the scope from the frame cart at the read
        // boundary, so this short borrow only needs to outlive the `submit_node` call.
        let scope = frame.scope_for_bind();
        self.submit_node(
            NodeWork::dispatch(expr),
            scope,
            NodeScope::Yoked,
            Some(chain),
        )
    }

    fn add_dispatch_in_frame(&mut self, expr: KExpression<'a>) -> NodeId {
        let frame = self
            .active_frame
            .clone()
            .expect("in-frame dispatch requires an active frame");
        let explicit_chain = self.active_chain.is_none().then(LexicalFrame::detached);
        let scope = frame.scope_for_bind();
        self.submit_node(
            NodeWork::dispatch(expr),
            scope,
            NodeScope::Yoked,
            explicit_chain,
        )
    }

    fn add_combine_in_frame(
        &mut self,
        owned_subs: Vec<NodeId>,
        park_producers: Vec<NodeId>,
        finish: CombineFinish<'a>,
    ) -> NodeId {
        let park_count = park_producers.len();
        let mut deps = park_producers;
        deps.extend(owned_subs);
        let frame = self
            .active_frame
            .clone()
            .expect("in-frame combine requires an active frame");
        let explicit_chain = self.active_chain.is_none().then(LexicalFrame::detached);
        let scope = frame.scope_for_bind();
        self.submit_node(
            NodeWork::Combine {
                deps,
                park_count,
                finish,
            },
            scope,
            NodeScope::Yoked,
            explicit_chain,
        )
    }
}
