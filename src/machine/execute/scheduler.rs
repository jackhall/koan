use std::rc::Rc;

use crate::machine::NodeId;

use dep_graph::DepGraph;
use node_store::NodeStore;
use work_queues::WorkQueues;

pub(in crate::machine::execute) use workload::{FramedRead, Workload};

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
mod workload;

/// A dynamic DAG of dispatch and execution work.
///
/// The execute loop drains via [`WorkQueues::pop_next`], which prioritizes in-flight slots
/// (sub-work and notify-walk wakeups) ahead of fresh top-level dispatches. Owned edges never
/// cycle — a new node's `NodeId` is strictly greater than every node it owns. Park (`Notify`)
/// edges can point at an earlier producer, so a self-referential binding (`LET x = x`) forms
/// a cycle that drains with both slots still `PreRun`; `execute` detects the leftover parked
/// slots and returns `KErrorKind::SchedulerDeadlock`.
///
/// Generic over a single [`Workload`] `W`: an opaque per-node payload `W::Payload` (persisted across
/// a slot's steps; Koan: scope handle + lexical chain), an inter-node value `W::Value` passed along
/// dep edges (Koan: the lifted `Carried`), a terminal error `W::Error`, and a per-node memory frame
/// `W::Frame` it manages by `Rc`. The scheduler stores all four and hands them back but inspects
/// none — it names no Koan value, error, scope, memory, or AST type. The Koan instantiation is
/// `KoanWorkload`; the Koan workload carries the scope a node runs against in its payload, sub-nodes
/// default to the spawning node's payload, and a user-fn invocation installs a per-call child via
/// `NodeStep::Replace`.
///
/// See design/execution-model.md and design/memory-model.md.
pub(in crate::machine::execute) struct Scheduler<W: Workload> {
    pub(in crate::machine::execute::scheduler) queues: WorkQueues,
    pub(in crate::machine::execute::scheduler) deps: DepGraph,
    pub(in crate::machine::execute::scheduler) store: NodeStore<W>,
    /// TraceFrame Rc of the slot currently being executed. See
    /// [per-call-arena-protocol.md § Active-frame propagation](../../../design/per-call-arena-protocol.md#active-frame-propagation).
    pub(in crate::machine::execute::scheduler) active_frame: Option<Rc<W::Frame>>,
    /// The run frame: a non-dying workload frame adopting the top-level run scope, lazily built on
    /// the first run-lifetime submission. Top-level slots carry it as their `frame` cart, so
    /// `active_frame` is never `None` during a top-level step and a body's re-dispatch against its
    /// own scope is uniformly framed (Yoked) at every depth.
    pub(in crate::machine::execute::scheduler) run_frame: Option<Rc<W::Frame>>,
    /// Per-slot reserve frame for the running step. `None` between slot steps. See
    /// [per-call-arena-protocol.md § Ping-pong reserve frame](../../../design/per-call-arena-protocol.md#ping-pong-reserve-frame).
    pub(in crate::machine::execute::scheduler) active_reserve: Option<Rc<W::Frame>>,
    /// The executing slot's own opaque workload payload, installed per step (Koan: scope handle +
    /// lexical chain). A body that re-dispatches *against its own scope*, or that needs the ambient
    /// chain, reads this back through [`Self::active_payload`]; the scheduler stores and hands it
    /// back but never inspects it. `None` between slot steps.
    pub(in crate::machine::execute::scheduler) active_payload: Option<W::Payload>,
    /// Whether the slot currently executing already carries a kept return contract — i.e. it is a
    /// tail call *within* an established chain. A deferred-return FN dispatched here is a subsequent
    /// tail call whose own contract would be discarded by the keep-first rule, so it skips resolving
    /// its (possibly async `Expression`-form) return type and just tail-replaces its body. Set per
    /// step in [`KoanRuntime::execute`](super::runtime::KoanRuntime::execute); read via `Scheduler::in_contract_chain`.
    pub(in crate::machine::execute::scheduler) active_in_contract_chain: bool,
    #[cfg(test)]
    pub(in crate::machine::execute::scheduler) tail_reuse_count: usize,
}

/// RAII-shaped save/restore wrapper around the per-step `active_frame`, `active_payload`,
/// and `active_reserve` swap that brackets each iteration of [`KoanRuntime::execute`](super::runtime::KoanRuntime::execute).
/// Bookkeeping spine for the ping-pong reserve-frame rotation; see
/// [per-call-arena-protocol.md § Ping-pong reserve frame](../../../design/per-call-arena-protocol.md#ping-pong-reserve-frame).
pub(in crate::machine::execute::scheduler) struct SlotStepGuard<W: Workload> {
    prev_frame: Option<Rc<W::Frame>>,
    prev_payload: Option<W::Payload>,
    /// Saved so nested slot runs (combinator finish closures) don't inherit the
    /// outer slot's reserve frame.
    prev_reserve: Option<Rc<W::Frame>>,
    /// The step's own payload, kept so [`Scheduler::exit_slot_step`] can hand it back inside the
    /// [`PostStep`] token — the step's scope is then re-derivable from the *returned* payload (and
    /// frame), never the ambient (and possibly invoke-swapped) state.
    step_payload: W::Payload,
}

/// The frames and payload of a just-finished step, returned by [`Scheduler::exit_slot_step`]. Owns
/// `prev_frame` (the slot's frame *at step end* — an in-step invoke may have swapped the ambient
/// `active_frame`, so this returned value, not `self.active_frame`, is the authoritative source)
/// and hands back the slot's opaque workload payload through [`Self::payload`]; the workload
/// re-anchors it against `prev_frame` at the Done boundary. Reading the step state from ambient
/// scheduler state post-step is thereby unspellable.
pub(in crate::machine::execute::scheduler) struct PostStep<W: Workload> {
    /// The slot's cart at step end. Always present: `enter_slot_step` installs the node's cart and
    /// an invoke never empties `active_frame` — reuse draws from the reserve via
    /// `acquire_tail_frame`, never the live active cart — so the slot's own cart rides through. The
    /// Replace arm reinstalls / rotates with it.
    pub(in crate::machine::execute::scheduler) prev_frame: Rc<W::Frame>,
    /// The slot's reserve frame at step end (see ping-pong reserve rotation).
    pub(in crate::machine::execute::scheduler) post_step_reserve: Option<Rc<W::Frame>>,
    payload: W::Payload,
}

impl<W: Workload> PostStep<W> {
    /// The slot's opaque workload payload at step end — lifetime-free and uninterpreted. The
    /// workload re-anchors it (Koan: the scope handle) against [`Self::prev_frame`] at the Done
    /// boundary; `PostStep` never materializes a `&Scope` itself, so reading the step scope from
    /// ambient (possibly invoke-swapped) state stays unspellable.
    pub(in crate::machine::execute::scheduler) fn payload(&self) -> &W::Payload {
        &self.payload
    }
}

impl<W: Workload> Scheduler<W> {
    /// Install the slot's frame/payload/reserve as the ambient values for one step. The
    /// caller passes the returned guard to [`Scheduler::exit_slot_step`] when the step
    /// returns; `node_payload` is cloned only here (so the caller can keep its own copy for
    /// the Replace arm without double-counting any `Rc` it holds). The sole `W::Payload: Clone` site.
    pub(in crate::machine::execute::scheduler) fn enter_slot_step(
        &mut self,
        node_frame: Rc<W::Frame>,
        node_reserve: Option<Rc<W::Frame>>,
        node_payload: W::Payload,
    ) -> SlotStepGuard<W> {
        let prev_frame = self.active_frame.replace(node_frame);
        let prev_reserve = std::mem::replace(&mut self.active_reserve, node_reserve);
        let prev_payload = self.active_payload.replace(node_payload.clone());
        SlotStepGuard {
            prev_frame,
            prev_payload,
            prev_reserve,
            step_payload: node_payload,
        }
    }
}

impl<W: Workload> Scheduler<W> {

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
        guard: SlotStepGuard<W>,
    ) -> PostStep<W> {
        let post_step_frame = std::mem::replace(&mut self.active_frame, guard.prev_frame);
        let post_step_reserve = std::mem::replace(&mut self.active_reserve, guard.prev_reserve);
        self.active_payload = guard.prev_payload;
        PostStep {
            prev_frame: post_step_frame.expect(
                "a step runs against a cart; an invoke reuses the reserve, never the active",
            ),
            post_step_reserve,
            payload: guard.step_payload,
        }
    }

    pub fn new() -> Self {
        Self {
            queues: WorkQueues::new(),
            deps: DepGraph::new(),
            store: NodeStore::new(),
            active_frame: None,
            run_frame: None,
            active_payload: None,
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

    /// The live slot's opaque workload payload, or `None` once it has terminalized — at which point
    /// `take_for_run` has moved the payload out. Test-only; the workload extracts the field it wants.
    #[cfg(test)]
    pub fn payload_of(&self, id: NodeId) -> Option<&W::Payload> {
        self.store.payload_of(id)
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

    /// Only safe on IDs returned by `dispatch_in_scope`; internal slots may have been eagerly
    /// freed by their parent. Follows a bare-name-forward alias to the real producer.
    pub fn read_result(&self, id: NodeId) -> Result<W::Value, &W::Error> {
        self.store.read_result(self.resolve_alias(id))
    }

    /// Panics on `Err`. Follows a bare-name-forward alias to the real producer.
    pub fn read(&self, id: NodeId) -> W::Value {
        self.store.read(self.resolve_alias(id))
    }

    /// Read a terminal with the producer frame `Rc` backing it, for the consumer-pull lift. Follows
    /// a bare-name-forward alias to the real producer (which holds the value in its own frame).
    pub(in crate::machine::execute) fn read_result_with_frame(
        &self,
        id: NodeId,
    ) -> FramedRead<'_, W> {
        self.store.read_result_with_frame(self.resolve_alias(id))
    }

    /// Re-home a finalized terminal (already lifted into a surviving arena), dropping the pinned
    /// producer frame. The drain boundary uses this for consumer-less roots. Resolves a bare-name
    /// alias so the real producer's frame — not the alias slot — is released.
    pub(in crate::machine::execute) fn rehome_terminal(
        &mut self,
        id: NodeId,
        output: Result<W::Value, W::Error>,
    ) {
        let target = self.resolve_alias(id);
        self.store.rehome_terminal(target, output);
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

    /// Borrow the executing slot's opaque workload payload (Koan: scope handle + lexical chain),
    /// installed per step by [`Self::enter_slot_step`]. The single accessor the workload reads its
    /// name-resolution state back through; the scheduler stores and hands it back but never inspects
    /// it. `None` between slot steps.
    pub(in crate::machine::execute) fn active_payload(&self) -> Option<&W::Payload> {
        self.active_payload.as_ref()
    }

    /// Whether the executing slot already carries a kept return contract (a tail call inside an
    /// established chain). See [`Self::active_in_contract_chain`].
    pub(in crate::machine::execute) fn in_contract_chain(&self) -> bool {
        self.active_in_contract_chain
    }

    /// Borrow the active per-call cart — the witness the workload binds a `Yoked` slot's
    /// re-anchored scope borrow to. Mirrors [`Self::current_frame`] but hands back a borrow, not a
    /// clone.
    pub(in crate::machine::execute) fn active_frame_ref(&self) -> Option<&Rc<W::Frame>> {
        self.active_frame.as_ref()
    }
}

impl<W: Workload> Default for Scheduler<W> {
    fn default() -> Self {
        Self::new()
    }
}

/// The scheduler's frame/chain reads and the per-call-frame allocator that
/// [`KoanRuntime`](super::runtime::KoanRuntime) — the sole `&mut Scheduler` — calls while realizing
/// a decided [`Outcome`](super::outcome::Outcome). AST-free state operations: the AST-aware
/// submission wrappers (`enter_block`, `dispatch_in_own_scope`, …) live on `KoanRuntime`.
impl<W: Workload> Scheduler<W> {
    /// Active slot's frame `Rc`. See
    /// [per-call-arena-protocol.md § Active-frame propagation](../../../design/per-call-arena-protocol.md#active-frame-propagation).
    pub(in crate::machine::execute) fn current_frame(&self) -> Option<Rc<W::Frame>> {
        self.active_frame.clone()
    }

    /// Install `frame` as the ambient cart and return the previous one — the transient save/restore
    /// [`dispatch_body`](super::runtime::KoanRuntime::dispatch_body) wraps each
    /// body sub-slot in, so the sub-slot inherits the body frame rather than the caller's.
    pub(in crate::machine::execute) fn swap_active_frame(
        &mut self,
        frame: Option<Rc<W::Frame>>,
    ) -> Option<Rc<W::Frame>> {
        std::mem::replace(&mut self.active_frame, frame)
    }

    /// Take the slot's reserve cart for a TCO tail reuse, leaving none. The workload resets it under
    /// the body's outer scope (or, on a uniqueness-gate miss, drops it and mints fresh) — the
    /// scope-dependent frame construction the scheduler does not own. The just-finished cart is
    /// rotated back in as the next reserve by `execute`'s Replace arm. Reuse draws from the
    /// *reserve*, never the live `active_frame`, so the slot's own cart is never emptied by an invoke.
    pub(in crate::machine::execute) fn take_active_reserve(&mut self) -> Option<Rc<W::Frame>> {
        self.active_reserve.take()
    }

    /// Record a TCO reserve reuse (test-only counter). A no-op outside tests.
    pub(in crate::machine::execute) fn note_tail_reuse(&mut self) {
        #[cfg(test)]
        {
            self.tail_reuse_count += 1;
        }
    }

    /// Whether the run frame is established. The workload mints it (adopting the run scope) on the
    /// first run-lifetime submission via [`Self::set_run_frame`].
    pub(in crate::machine::execute) fn has_run_frame(&self) -> bool {
        self.run_frame.is_some()
    }

    /// Install the run frame the workload minted by adopting the top-level run scope. Idempotent at
    /// the call site (the workload guards on [`Self::has_run_frame`]).
    pub(in crate::machine::execute) fn set_run_frame(&mut self, frame: Rc<W::Frame>) {
        self.run_frame = Some(frame);
    }
}
