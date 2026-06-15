use std::rc::Rc;

use crate::machine::core::{assemble_body_chain, ScopeId};
use crate::machine::model::ast::KExpression;
use crate::machine::model::Carried;
use crate::machine::{CallArena, KError, LexicalFrame, NodeId, Scope};

use super::nodes::NodeScope;
use super::runtime::KoanRuntime;
use dep_graph::DepGraph;
use node_store::NodeStore;
use work_queues::WorkQueues;

mod dep_graph;
mod execute;
mod finish;
mod node_store;
#[cfg(test)]
mod run_tests;
mod splice;
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
    /// step in [`KoanRuntime::execute`](super::runtime::KoanRuntime::execute); read via `Scheduler::in_contract_chain`.
    pub(in crate::machine::execute::scheduler) active_in_contract_chain: bool,
    #[cfg(test)]
    pub(in crate::machine::execute::scheduler) tail_reuse_count: usize,
}

/// RAII-shaped save/restore wrapper around the per-step `active_frame`, `active_chain`,
/// and `active_reserve` swap that brackets each iteration of [`KoanRuntime::execute`](super::runtime::KoanRuntime::execute).
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

    pub fn len(&self) -> usize {
        self.store.len()
    }
    pub fn is_empty(&self) -> bool {
        self.store.is_empty()
    }

    /// An errored sub counts as ready — parents short-circuit on it. Follows a bare-name-forward
    /// alias to the real producer (see [`splice`](self::splice)).
    pub(in crate::machine::execute) fn is_result_ready(&self, id: NodeId) -> bool {
        self.store.is_result_ready(self.resolve_alias(id))
    }

    /// Only safe on IDs returned by `add_dispatch`; internal slots may have been eagerly
    /// freed by their parent. Follows a bare-name-forward alias to the real producer.
    pub fn read_result(&self, id: NodeId) -> Result<Carried<'run>, &KError> {
        self.store.read_result(self.resolve_alias(id))
    }

    /// Panics on `Err`. Follows a bare-name-forward alias to the real producer.
    pub fn read(&self, id: NodeId) -> Carried<'run> {
        self.store.read(self.resolve_alias(id))
    }

    // ----- Narrow dispatcher-facing surface (pub(in execute)) -----
    //
    // These methods are the dispatcher's named contract with the scheduler:
    // the read view (`SchedulerView`) and the write harness route through them,
    // so the storage layout (`deps` / `store` / `queues` / `active_*` fields)
    // stays scheduler-internal.

    // `add_owned_edge` / `add_park_edge` (the alias-resolving edge installs) and the splice itself
    // live in [`splice`](self::splice), the one home for the bare-name-forward graph logic.

    /// True iff `producer` is forward-reachable from `consumer`
    /// (`DepGraph::would_create_cycle`).
    pub(in crate::machine::execute) fn would_create_cycle(
        &self,
        producer: NodeId,
        consumer: NodeId,
    ) -> bool {
        self.deps.would_create_cycle(producer, consumer)
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
    /// `&mut self` call — so it holds nothing across the in-step TCO frame reset.
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
}

impl<'run> Default for Scheduler<'run> {
    fn default() -> Self {
        Self::new()
    }
}

/// The scheduler's frame/chain reads and the per-call-frame allocator that
/// [`KoanRuntime`](super::runtime::KoanRuntime) — the sole `&mut Scheduler` — calls while realizing
/// a decided [`Outcome`](super::outcome::Outcome). AST-free state operations: the AST-aware
/// submission wrappers (`enter_block`, `dispatch_here`, …) live on `KoanRuntime`.
impl<'run> Scheduler<'run> {
    /// Active slot's `Rc<CallArena>`. See
    /// [per-call-arena-protocol.md § Active-frame propagation](../../../design/per-call-arena-protocol.md#active-frame-propagation).
    pub(in crate::machine::execute) fn current_frame(&self) -> Option<Rc<CallArena>> {
        self.active_frame.clone()
    }

    /// Reuse the slot's reserve cart (reset in place) when uniquely owned, else allocate fresh
    /// under `outer`. Reuse draws from the *reserve*, never the live `active_frame`, so the
    /// slot's own cart is never emptied by an invoke — `try_reset_for_tail`'s `Rc::get_mut`
    /// gate is exactly the "no escape" uniqueness check (a cloned `Rc` would foreclose reuse).
    /// The just-finished cart is rotated back in as the next reserve by `execute`'s Replace arm.
    pub(in crate::machine::execute) fn acquire_tail_frame(
        &mut self,
        outer: &Scope<'_>,
    ) -> Rc<CallArena> {
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

    /// Active slot's lexical chain. Mirrors [`Self::current_frame`].
    pub(in crate::machine::execute) fn current_lexical_chain(&self) -> Option<Rc<LexicalFrame>> {
        self.active_chain.clone()
    }
}

/// The AST-aware submission wrappers — the dispatch-submission surface the roadmap moves onto the
/// harness. Each resolves `(scope, node_scope, chain)` from scheduler state and forwards to
/// [`Self::submit_dispatch`]; none holds `&mut Scheduler` outside `KoanRuntime`.
impl<'run> KoanRuntime<'run> {
    /// Submit each `statement` as a fresh lexical block over `scope`: mint a frame `(scope_id, i+1)`
    /// per statement (parent = current `active_chain`) and dispatch each against `scope`. The
    /// program / REPL / test entry point for a block of top-level statements.
    pub fn enter_block(
        &mut self,
        scope_id: ScopeId,
        statements: Vec<KExpression<'run>>,
        scope: &'run Scope<'run>,
    ) -> Vec<NodeId> {
        let parent = self.sched.active_chain.clone();
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

    /// Schedule `expr` against `scope` with `chain` attached explicitly. The ambient
    /// `add_dispatch` reads `active_chain` instead; this is the only way to override that.
    pub(in crate::machine::execute) fn add_dispatch_with_chain(
        &mut self,
        expr: KExpression<'run>,
        scope: &'run Scope<'run>,
        chain: Rc<LexicalFrame>,
    ) -> NodeId {
        self.sched.ensure_run_frame(scope);
        let node_scope = self.sched.resolve_node_scope(scope);
        self.submit_dispatch(expr, scope, node_scope, Some(chain))
    }

    /// Dispatch `expr` as a sub-slot of the currently-active per-call frame, storing the slot's
    /// scope as a `Yoked` handle re-projected from the frame cart rather than a fabricated `&'run`.
    /// The caller must have installed the per-call frame as `active_frame` (the run loop does this
    /// per step; [`Self::dispatch_body_statements`] does it transiently). `chain` is the explicit
    /// lexical chain (`Some` for an `enter_block`-routed body statement; the ambient-inheriting
    /// `ActiveFrame` placement passes [`Scheduler::ambient_or_detached_chain`]).
    pub(in crate::machine::execute) fn add_dispatch_in_frame(
        &mut self,
        expr: KExpression<'run>,
        chain: Option<Rc<LexicalFrame>>,
    ) -> NodeId {
        let frame = self
            .sched
            .active_frame
            .clone()
            .expect("in-frame dispatch requires an active frame");
        // `scope_for_bind` is `Rc`-bounded — not a free `'run`-fabrication. The
        // slot stores `Yoked` and re-projects the scope from the frame cart at the read
        // boundary, so this short borrow only needs to outlive the `submit_dispatch` call.
        let scope = frame.scope_for_bind();
        self.submit_dispatch(expr, scope, NodeScope::Yoked, chain)
    }

    /// Dispatch a body's non-tail `statements` as sibling sub-slots in `frame`, each positioned at
    /// body-chain index `i + 1` (the params / `it` sit at idx 0) over the frame's body scope, with
    /// the parent chain reconstructed from the call site via [`assemble_body_chain`]. The shared
    /// "execute a block of expressions" primitive: a multi-statement FN body (`KFunction::invoke`),
    /// a deferred return-type dep, and a MATCH/TRY arm body (the action harness) all use it. The
    /// caller tail-replaces into the body's last statement separately. Returns the sub-slots' ids.
    pub(in crate::machine::execute) fn dispatch_body_statements(
        &mut self,
        frame: &Rc<CallArena>,
        statements: Vec<KExpression<'run>>,
    ) -> Vec<NodeId> {
        let body_scope = frame.scope_for_bind();
        let body_scope_id = body_scope.id;
        let parent = assemble_body_chain(
            body_scope,
            self.sched
                .current_lexical_chain()
                .expect("a body block runs inside an active lexical chain"),
            0,
        )
        .parent
        .clone();
        let mut ids = Vec::with_capacity(statements.len());
        for (i, statement) in statements.into_iter().enumerate() {
            let statement_chain = LexicalFrame::push(parent.clone(), body_scope_id, i + 1);
            // Install `frame` as the ambient cart so `add_dispatch_in_frame` reads it back, then
            // restore the previous — the sub-slot inherits this frame, not the caller's.
            let prev = self.sched.active_frame.replace(frame.clone());
            let bid = self.add_dispatch_in_frame(statement, Some(statement_chain));
            self.sched.active_frame = prev;
            ids.push(bid);
        }
        ids
    }
}
