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
pub struct Scheduler<'run> {
    pub(in crate::machine::execute::scheduler) queues: WorkQueues,
    pub(in crate::machine::execute::scheduler) deps: DepGraph,
    pub(in crate::machine::execute::scheduler) store: NodeStore<'run>,
    /// TraceFrame Rc of the slot currently being executed. See
    /// [per-call-arena-protocol.md § Active-frame propagation](../../../design/per-call-arena-protocol.md#active-frame-propagation).
    pub(in crate::machine::execute::scheduler) active_frame: Option<Rc<CallArena>>,
    /// The run frame: a non-dying [`CallArena`] adopting the top-level run scope, lazily built on
    /// the first run-lifetime submission. Top-level slots carry it as their `frame` cart, so
    /// `active_frame` is never `None` during a top-level step and a body's re-dispatch against its
    /// own scope is uniformly framed (Yoked) at every depth. See [`CallArena::adopting`].
    pub(in crate::machine::execute::scheduler) run_frame: Option<Rc<CallArena>>,
    /// Lexical chain of the slot currently executing. `Scheduler::add` reads this to attach
    /// a chain to every sub-slot that doesn't carry an explicit `enter_block` chain, so
    /// internal binder sub-dispatches inherit the parent's chain implicitly.
    pub(in crate::machine::execute::scheduler) active_chain: Option<Rc<LexicalFrame>>,
    /// Per-slot reserve frame for the running step. `None` between slot steps. See
    /// [per-call-arena-protocol.md § Ping-pong reserve frame](../../../design/per-call-arena-protocol.md#ping-pong-reserve-frame).
    pub(in crate::machine::execute::scheduler) active_reserve: Option<Rc<CallArena>>,
    /// The executing slot's own [`NodeScope`] handle, installed per step. A body that
    /// re-dispatches *against its own scope* reads this through the `*_here` handle methods, so
    /// the sub-slot inherits the slot's honest handle — `Anchored(&'run)` for a genuinely run-lived
    /// scope (a binder's decl-scope), `Yoked` for a per-call frame child — rather than the body
    /// trying (and failing) to widen its `&'frame` borrow back to `&'run`.
    pub(in crate::machine::execute::scheduler) active_node_scope: Option<NodeScope<'run>>,
    /// Whether the slot currently executing already carries a kept return contract — i.e. it is a
    /// tail call *within* an established chain. A deferred-return FN dispatched here is a subsequent
    /// tail call whose own contract would be discarded by the keep-first rule, so it skips resolving
    /// its (possibly async `Expression`-form) return type and just tail-replaces its body. Set per
    /// step in [`Scheduler::execute`]; read via `DispatchCtx::in_contract_chain`.
    pub(in crate::machine::execute::scheduler) active_in_contract_chain: bool,
    #[cfg(test)]
    pub(in crate::machine::execute::scheduler) tail_reuse_count: usize,
}

/// RAII-shaped save/restore wrapper around the per-step `active_frame`, `active_chain`,
/// and `active_reserve` swap that brackets each iteration of [`Scheduler::execute`].
/// Bookkeeping spine for the ping-pong reserve-frame rotation; see
/// [per-call-arena-protocol.md § Ping-pong reserve frame](../../../design/per-call-arena-protocol.md#ping-pong-reserve-frame).
pub(in crate::machine::execute::scheduler) struct SlotStepGuard<'run> {
    prev_frame: Option<Rc<CallArena>>,
    prev_chain: Option<Rc<LexicalFrame>>,
    /// Saved so nested slot runs (combinator finish closures) don't inherit the
    /// outer slot's reserve frame.
    prev_reserve: Option<Rc<CallArena>>,
    prev_node_scope: Option<NodeScope<'run>>,
    /// The step's own handle, kept so [`Scheduler::exit_slot_step`] can hand it back inside the
    /// [`PostStep`] token — the step scope is then re-derivable from the *returned* frame, never
    /// the ambient (and possibly invoke-swapped) `active_frame`.
    step_node_scope: NodeScope<'run>,
}

/// The frames and scope of a just-finished step, returned by [`Scheduler::exit_slot_step`]. Owns
/// `prev_frame` (the slot's frame *at step end* — an in-step invoke may have swapped the ambient
/// `active_frame`, so this returned value, not `self.active_frame`, is the authoritative source)
/// and exposes the step scope only through [`Self::step_scope`], which derives it from that frame.
/// Reading the step scope from ambient scheduler state post-step is thereby unspellable.
pub(in crate::machine::execute::scheduler) struct PostStep<'run> {
    /// The slot's cart at step end. Always present: `enter_slot_step` installs the node's cart and
    /// an invoke never empties `active_frame` — reuse draws from the reserve via
    /// `acquire_tail_frame`, never the live active cart — so the slot's own cart rides through. The
    /// Replace arm reinstalls / rotates with it.
    pub(in crate::machine::execute::scheduler) prev_frame: Rc<CallArena>,
    /// The slot's reserve frame at step end (see ping-pong reserve rotation).
    pub(in crate::machine::execute::scheduler) post_step_reserve: Option<Rc<CallArena>>,
    node_scope: NodeScope<'run>,
}

impl<'run> PostStep<'run> {
    /// The step's scope, re-handed from the authoritative `prev_frame` via the bounded brand (an
    /// `Anchored` slot carries its own run-lived borrow). Borrow bounded by `&self`, so it cannot
    /// outlive this token's `prev_frame`.
    pub(in crate::machine::execute::scheduler) fn step_scope(&self) -> &Scope<'run> {
        match self.node_scope {
            NodeScope::Anchored(scope) => scope,
            NodeScope::Yoked => self.prev_frame.scope_bounded(),
        }
    }
}

impl<'run> Scheduler<'run> {
    /// Install the slot's frame/chain/reserve as the ambient values for one step. The
    /// caller passes the returned guard to [`Scheduler::exit_slot_step`] when the step
    /// returns; the `node_chain` Rc is cloned only here, so the caller's own clone for
    /// the Replace arm doesn't double-count.
    pub(in crate::machine::execute::scheduler) fn enter_slot_step(
        &mut self,
        node_frame: Rc<CallArena>,
        node_reserve: Option<Rc<CallArena>>,
        node_chain: Rc<LexicalFrame>,
        node_scope: NodeScope<'run>,
    ) -> SlotStepGuard<'run> {
        let prev_frame = self.active_frame.replace(node_frame);
        let prev_chain = self.active_chain.replace(node_chain);
        let prev_reserve = std::mem::replace(&mut self.active_reserve, node_reserve);
        let prev_node_scope = self.active_node_scope.replace(node_scope);
        SlotStepGuard {
            prev_frame,
            prev_chain,
            prev_reserve,
            prev_node_scope,
            step_node_scope: node_scope,
        }
    }

    /// Restore the values saved by [`Scheduler::enter_slot_step`] and return
    /// `(post_step_frame, post_step_reserve)`.
    ///
    /// `post_step_reserve` carries the slot's reserve at step end. The Replace arm reads it to
    /// decide rotation: with a new frame, the post-step reserve is two iterations old and gets
    /// dropped; without one, it rides along on the reinstalled node. An invoke that reused the
    /// reserve via `acquire_tail_frame` already consumed it, so it reads back `None` there.
    ///
    /// This is the single boundary where the "every step runs against a cart" invariant is
    /// asserted: `active_frame` is `Some` for the whole step (`enter_slot_step` installs the
    /// node's non-optional cart; an invoke reuses the *reserve*, never the active cart, so nothing
    /// empties it), so the `expect` cannot fire. `active_frame` itself stays `Option` because it is
    /// legitimately `None` *between* steps.
    pub(in crate::machine::execute::scheduler) fn exit_slot_step(
        &mut self,
        guard: SlotStepGuard<'run>,
    ) -> PostStep<'run> {
        let post_step_frame = std::mem::replace(&mut self.active_frame, guard.prev_frame);
        self.active_chain = guard.prev_chain;
        let post_step_reserve = std::mem::replace(&mut self.active_reserve, guard.prev_reserve);
        self.active_node_scope = guard.prev_node_scope;
        PostStep {
            prev_frame: post_step_frame.expect(
                "a step runs against a cart; an invoke reuses the reserve, never the active",
            ),
            post_step_reserve,
            node_scope: guard.step_node_scope,
        }
    }

    pub fn new() -> Self {
        Self {
            queues: WorkQueues::new(),
            deps: DepGraph::new(),
            store: NodeStore::new(),
            active_frame: None,
            run_frame: None,
            active_node_scope: None,
            active_chain: None,
            active_reserve: None,
            active_in_contract_chain: false,
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

    /// Run a builtin body directly against `scope` as the executing slot's scope. A body
    /// reads its scope on demand via [`SchedulerHandle::current_scope`], so a direct-body
    /// test installs `Anchored(scope)` for the duration of the call. The cart is a throwaway
    /// fixture frame adopting `scope`; `Anchored` reads `scope` directly, so the cart only
    /// satisfies the non-optional `enter_slot_step` contract.
    #[cfg(test)]
    pub fn run_body_against<R>(
        &mut self,
        scope: &'run Scope<'run>,
        body: impl FnOnce(&mut dyn SchedulerHandle<'run, 'run>) -> R,
    ) -> R {
        let chain = LexicalFrame::root(scope.id, 0);
        let guard = self.enter_slot_step(
            CallArena::adopting(scope),
            None,
            chain,
            NodeScope::Anchored(scope),
        );
        let out = body(self);
        let _ = self.exit_slot_step(guard);
        out
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
    pub fn read_result(&self, id: NodeId) -> Result<Carried<'run>, &KError> {
        self.store.read_result(id)
    }

    /// Panics on `Err`.
    pub fn read(&self, id: NodeId) -> Carried<'run> {
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

    /// Whether the executing slot already carries a kept return contract (a tail call inside an
    /// established chain). See [`Self::active_in_contract_chain`].
    pub(in crate::machine::execute) fn in_contract_chain(&self) -> bool {
        self.active_in_contract_chain
    }

    /// The executing slot's scope, materialized on demand: an `Anchored` slot hands back its
    /// stored run-lived `&Scope`; a `Yoked` slot re-projects from the live `active_frame` cart via
    /// the bounded brand. A short borrow bounded by `&self` — fetched per use, never held across a
    /// `&mut self` call — so it holds nothing across the in-step TCO frame reset. See
    /// [`SchedulerHandle::current_scope`](crate::machine::SchedulerHandle::current_scope).
    pub(in crate::machine::execute) fn current_scope(&self) -> &Scope<'run> {
        self.current_scope_opt()
            .expect("a slot step installs active_node_scope (and a Yoked slot keeps its frame)")
    }

    /// Like [`Self::current_scope`] but `None` outside a slot step (no `active_node_scope`
    /// installed). Within a step the scope is always present: an `Anchored` slot carries its own
    /// borrow, and a `Yoked` slot's `active_frame` is never emptied mid-step (an invoke reuses the
    /// reserve, not the active cart), so the inner `expect` cannot fire.
    pub(in crate::machine::execute) fn current_scope_opt(&self) -> Option<&Scope<'run>> {
        match self.active_node_scope? {
            NodeScope::Anchored(scope) => Some(scope),
            NodeScope::Yoked => Some(
                self.active_frame
                    .as_ref()
                    .expect("a Yoked slot step keeps its active cart")
                    .scope_bounded(),
            ),
        }
    }

    /// Replace the ambient `active_frame` with `new`, returning the prior
    /// value. The `with_active_frame` body bracket goes through this.
    pub(in crate::machine::execute) fn active_frame_replace(
        &mut self,
        new: Option<Rc<CallArena>>,
    ) -> Option<Rc<CallArena>> {
        std::mem::replace(&mut self.active_frame, new)
    }
}

impl<'run> Default for Scheduler<'run> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'run, 's> SchedulerHandle<'run, 's> for Scheduler<'run> {
    fn add_dispatch(&mut self, expr: KExpression<'run>, scope: &'run Scope<'run>) -> NodeId {
        Scheduler::add_dispatch(self, expr, scope)
    }

    fn add_combine(
        &mut self,
        owned_subs: Vec<NodeId>,
        park_producers: Vec<NodeId>,
        scope: &'run Scope<'run>,
        finish: CombineFinish<'run>,
    ) -> NodeId {
        Scheduler::add_combine(self, owned_subs, park_producers, scope, finish)
    }

    fn add_catch(
        &mut self,
        from: NodeId,
        scope: &'run Scope<'run>,
        finish: CatchFinish<'run>,
    ) -> NodeId {
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
        body: &mut dyn FnMut(&mut dyn SchedulerHandle<'run, 's>),
    ) {
        let prev = self.active_frame.take();
        self.active_frame = Some(frame);
        body(self);
        self.active_frame = prev;
    }

    /// Reuse the slot's reserve cart (reset in place) when uniquely owned, else allocate fresh
    /// under `outer`. Reuse draws from the *reserve*, never the live `active_frame`, so the
    /// slot's own cart is never emptied by an invoke — `try_reset_for_tail`'s `Rc::get_mut`
    /// gate is exactly the "no escape" uniqueness check (a cloned `Rc` would foreclose reuse).
    /// The just-finished cart is rotated back in as the next reserve by `execute`'s Replace arm.
    fn acquire_tail_frame(&mut self, outer: &Scope<'_>) -> Rc<CallArena> {
        if let Some(mut reserve) = self.active_reserve.take() {
            if reserve.try_reset_for_tail(outer) {
                #[cfg(test)]
                {
                    self.tail_reuse_count += 1;
                }
                return reserve;
            }
        }
        CallArena::new(outer, None)
    }

    fn current_lexical_chain(&self) -> Option<Rc<LexicalFrame>> {
        self.active_chain.clone()
    }

    fn enter_block(
        &mut self,
        scope_id: ScopeId,
        statements: Vec<KExpression<'run>>,
        scope: &'run Scope<'run>,
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
        expr: KExpression<'run>,
        scope: &'run Scope<'run>,
        chain: Rc<LexicalFrame>,
    ) -> NodeId {
        Scheduler::add_with_chain(self, NodeWork::dispatch(expr), scope, Some(chain))
    }

    fn add_dispatch_with_chain_in_frame(
        &mut self,
        expr: KExpression<'run>,
        chain: Rc<LexicalFrame>,
    ) -> NodeId {
        let frame = self
            .active_frame
            .clone()
            .expect("in-frame dispatch requires an active frame");
        // `scope_for_bind` is `Rc`-bounded — not a free `'run`-fabrication. The
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

    fn add_dispatch_in_frame(&mut self, expr: KExpression<'run>) -> NodeId {
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
        finish: CombineFinish<'run>,
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

    fn add_catch_in_frame(&mut self, from: NodeId, finish: CatchFinish<'run>) -> NodeId {
        let frame = self
            .active_frame
            .clone()
            .expect("in-frame catch requires an active frame");
        let explicit_chain = self.active_chain.is_none().then(LexicalFrame::detached);
        let scope = frame.scope_for_bind();
        self.submit_node(
            NodeWork::Catch { from, finish },
            scope,
            NodeScope::Yoked,
            explicit_chain,
        )
    }

    fn current_scope(&self) -> &Scope<'run> {
        Scheduler::current_scope(self)
    }

    fn add_dispatch_here(&mut self, expr: KExpression<'run>) -> NodeId {
        self.submit_here(NodeWork::dispatch(expr))
    }

    fn add_combine_here(
        &mut self,
        owned_subs: Vec<NodeId>,
        park_producers: Vec<NodeId>,
        finish: CombineFinish<'run>,
    ) -> NodeId {
        let park_count = park_producers.len();
        let mut deps = park_producers;
        deps.extend(owned_subs);
        self.submit_here(NodeWork::Combine {
            deps,
            park_count,
            finish,
        })
    }

    fn add_catch_here(&mut self, from: NodeId, finish: CatchFinish<'run>) -> NodeId {
        self.submit_here(NodeWork::Catch { from, finish })
    }
}
