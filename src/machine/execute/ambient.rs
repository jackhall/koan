//! Ambient per-step context — the driver-side state a pure DAG runtime does not own.
//!
//! [`Scheduler`](crate::scheduler::Scheduler) is a workload-independent DAG of dispatch/execution
//! work; the *ambient* values that float across a single step (the active per-call frame, the slot's
//! reserve, the run frame, the executing slot's opaque payload, and the contract-chain flag) live
//! here on the driver [`KoanRuntime`](super::runtime::KoanRuntime), which is the
//! [`KoanWorkload`](super::runtime::KoanWorkload) instantiation and so may name the concrete Koan
//! types. The scheduler stores only `queues`/`deps`/`store`; the driver installs the ambient context
//! per step and reads it back through the methods below.
//!
//! See design/per-call-arena-protocol.md and design/execution-model.md.

use std::rc::Rc;

use crate::machine::CallArena;

use super::nodes::NodePayload;
use super::runtime::KoanRuntime;

/// The ambient per-step context the driver carries while realizing a decided
/// [`Outcome`](super::outcome::Outcome). Holds the active per-call frame, the slot's ping-pong
/// reserve, the lazily-built run frame, the executing slot's opaque payload (scope handle + lexical
/// chain), and whether that slot already carries a kept return contract. Concrete Koan types: the
/// driver is the workload, so the erasure the scheduler core needs is unnecessary here.
#[derive(Default)]
pub(in crate::machine::execute) struct AmbientContext {
    /// TraceFrame `Rc` of the slot currently being executed. See
    /// [per-call-arena-protocol.md § Active-frame propagation](../../../design/per-call-arena-protocol.md#active-frame-propagation).
    active_frame: Option<Rc<CallArena>>,
    /// Per-slot reserve frame for the running step. `None` between slot steps. See
    /// [per-call-arena-protocol.md § Ping-pong reserve frame](../../../design/per-call-arena-protocol.md#ping-pong-reserve-frame).
    active_reserve: Option<Rc<CallArena>>,
    /// The run frame: a non-dying frame adopting the top-level run scope, lazily built on the first
    /// run-lifetime submission. Top-level slots carry it as their `frame` cart, so `active_frame` is
    /// never `None` during a top-level step and a body's re-dispatch against its own scope is
    /// uniformly framed (Yoked) at every depth.
    run_frame: Option<Rc<CallArena>>,
    /// The executing slot's own opaque workload payload, installed per step (scope handle + lexical
    /// chain). A body that re-dispatches *against its own scope*, or that needs the ambient chain,
    /// reads this back through [`KoanRuntime::active_payload`]. `None` between slot steps.
    active_payload: Option<NodePayload>,
    /// Whether the slot currently executing already carries a kept return contract — i.e. it is a
    /// tail call *within* an established chain. A deferred-return FN dispatched here is a subsequent
    /// tail call whose own contract would be discarded by the keep-first rule, so it skips resolving
    /// its (possibly async `Expression`-form) return type and just tail-replaces its body. Set per
    /// step in [`KoanRuntime::execute`](super::runtime::KoanRuntime::execute); read via
    /// [`SchedulerView::in_contract_chain`](super::dispatch::SchedulerView::in_contract_chain).
    pub(in crate::machine::execute) active_in_contract_chain: bool,
    #[cfg(test)]
    tail_reuse_count: usize,
}

/// RAII-shaped save/restore wrapper around the per-step `active_frame`, `active_payload`,
/// and `active_reserve` swap that brackets each iteration of [`KoanRuntime::execute`](super::runtime::KoanRuntime::execute).
/// Bookkeeping spine for the ping-pong reserve-frame rotation; see
/// [per-call-arena-protocol.md § Ping-pong reserve frame](../../../design/per-call-arena-protocol.md#ping-pong-reserve-frame).
pub(in crate::machine::execute) struct SlotStepGuard {
    prev_frame: Option<Rc<CallArena>>,
    prev_payload: Option<NodePayload>,
    /// Saved so nested slot runs (combinator finish closures) don't inherit the
    /// outer slot's reserve frame.
    prev_reserve: Option<Rc<CallArena>>,
    /// The step's own payload, kept so [`KoanRuntime::exit_slot_step`] can hand it back inside the
    /// [`PostStep`] token — the step's scope is then re-derivable from the *returned* payload (and
    /// frame), never the ambient (and possibly invoke-swapped) state.
    step_payload: NodePayload,
}

/// The frames and payload of a just-finished step, returned by [`KoanRuntime::exit_slot_step`]. Owns
/// `prev_frame` (the slot's frame *at step end* — an in-step invoke may have swapped the ambient
/// `active_frame`, so this returned value, not the ambient `active_frame`, is the authoritative
/// source) and hands back the slot's opaque workload payload through [`Self::payload`]; the workload
/// re-anchors it against `prev_frame` at the Done boundary. Reading the step state from ambient
/// state post-step is thereby unspellable.
pub(in crate::machine::execute) struct PostStep {
    /// The slot's cart at step end. Always present: `enter_slot_step` installs the node's cart and
    /// an invoke never empties `active_frame` — reuse draws from the reserve via
    /// `acquire_tail_frame`, never the live active cart — so the slot's own cart rides through. The
    /// Replace arm reinstalls / rotates with it.
    pub(in crate::machine::execute) prev_frame: Rc<CallArena>,
    /// The slot's reserve frame at step end (see ping-pong reserve rotation).
    pub(in crate::machine::execute) post_step_reserve: Option<Rc<CallArena>>,
    payload: NodePayload,
}

impl PostStep {
    /// The slot's opaque workload payload at step end. The workload re-anchors it (the scope handle)
    /// against [`Self::prev_frame`] at the Done boundary; `PostStep` never materializes a `&Scope`
    /// itself, so reading the step scope from ambient (possibly invoke-swapped) state stays
    /// unspellable.
    pub(in crate::machine::execute) fn payload(&self) -> &NodePayload {
        &self.payload
    }
}

impl AmbientContext {
    /// Borrow the active per-call cart — the witness the workload binds a `Yoked` slot's
    /// re-anchored scope borrow to.
    pub(in crate::machine::execute) fn active_frame_ref(&self) -> Option<&Rc<CallArena>> {
        self.active_frame.as_ref()
    }

    /// Borrow the executing slot's opaque workload payload (scope handle + lexical chain), installed
    /// per step by [`KoanRuntime::enter_slot_step`]. `None` between slot steps.
    pub(in crate::machine::execute) fn active_payload(&self) -> Option<&NodePayload> {
        self.active_payload.as_ref()
    }
}

impl<'run> KoanRuntime<'run> {
    /// Install the slot's frame/payload/reserve as the ambient values for one step. The caller passes
    /// the returned guard to [`Self::exit_slot_step`] when the step returns; `node_payload` is cloned
    /// only here (so the caller can keep its own copy for the Replace arm without double-counting any
    /// `Rc` it holds).
    pub(in crate::machine::execute) fn enter_slot_step(
        &mut self,
        node_frame: Rc<CallArena>,
        node_reserve: Option<Rc<CallArena>>,
        node_payload: NodePayload,
    ) -> SlotStepGuard {
        let prev_frame = self.ambient.active_frame.replace(node_frame);
        let prev_reserve = std::mem::replace(&mut self.ambient.active_reserve, node_reserve);
        let prev_payload = self.ambient.active_payload.replace(node_payload.clone());
        SlotStepGuard {
            prev_frame,
            prev_payload,
            prev_reserve,
            step_payload: node_payload,
        }
    }

    /// Restore the values saved by [`Self::enter_slot_step`] and return the
    /// [`PostStep`] token (post-step frame + reserve + payload).
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
    pub(in crate::machine::execute) fn exit_slot_step(&mut self, guard: SlotStepGuard) -> PostStep {
        let post_step_frame = std::mem::replace(&mut self.ambient.active_frame, guard.prev_frame);
        let post_step_reserve =
            std::mem::replace(&mut self.ambient.active_reserve, guard.prev_reserve);
        self.ambient.active_payload = guard.prev_payload;
        PostStep {
            prev_frame: post_step_frame.expect(
                "a step runs against a cart; an invoke reuses the reserve, never the active",
            ),
            post_step_reserve,
            payload: guard.step_payload,
        }
    }

    /// Borrow the executing slot's opaque workload payload (scope handle + lexical chain), installed
    /// per step by [`Self::enter_slot_step`]. The single accessor the workload reads its
    /// name-resolution state back through. `None` between slot steps.
    pub(in crate::machine::execute) fn active_payload(&self) -> Option<&NodePayload> {
        self.ambient.active_payload.as_ref()
    }

    /// Whether a slot step is currently installed (a non-`None` ambient payload). The workload reads
    /// this to decide whether to default a submission's chain to the ambient one or synthesize a
    /// detached chain (test fixtures / top level).
    pub(in crate::machine::execute) fn has_active_payload(&self) -> bool {
        self.ambient.active_payload.is_some()
    }

    /// Active slot's frame `Rc`. See
    /// [per-call-arena-protocol.md § Active-frame propagation](../../../design/per-call-arena-protocol.md#active-frame-propagation).
    pub(in crate::machine::execute) fn current_frame(&self) -> Option<Rc<CallArena>> {
        self.ambient.active_frame.clone()
    }

    /// Borrow the active per-call cart — the witness the workload binds a `Yoked` slot's
    /// re-anchored scope borrow to. Mirrors [`Self::current_frame`] but hands back a borrow, not a
    /// clone.
    pub(in crate::machine::execute) fn active_frame_ref(&self) -> Option<&Rc<CallArena>> {
        self.ambient.active_frame.as_ref()
    }

    /// Install `frame` as the ambient cart and return the previous one — the transient save/restore
    /// [`dispatch_body`](Self::dispatch_body) wraps each body sub-slot in, so the sub-slot inherits
    /// the body frame rather than the caller's.
    pub(in crate::machine::execute) fn swap_active_frame(
        &mut self,
        frame: Option<Rc<CallArena>>,
    ) -> Option<Rc<CallArena>> {
        std::mem::replace(&mut self.ambient.active_frame, frame)
    }

    /// Take the slot's reserve cart for a TCO tail reuse, leaving none. The workload resets it under
    /// the body's outer scope (or, on a uniqueness-gate miss, drops it and mints fresh) — the
    /// scope-dependent frame construction the scheduler does not own. The just-finished cart is
    /// rotated back in as the next reserve by `execute`'s Replace arm. Reuse draws from the
    /// *reserve*, never the live `active_frame`, so the slot's own cart is never emptied by an invoke.
    pub(in crate::machine::execute) fn take_active_reserve(&mut self) -> Option<Rc<CallArena>> {
        self.ambient.active_reserve.take()
    }

    /// Record a TCO reserve reuse (test-only counter). A no-op outside tests.
    pub(in crate::machine::execute) fn note_tail_reuse(&mut self) {
        #[cfg(test)]
        {
            self.ambient.tail_reuse_count += 1;
        }
    }

    #[cfg(test)]
    pub(in crate::machine::execute) fn ambient_tail_reuse_count(&self) -> usize {
        self.ambient.tail_reuse_count
    }

    /// Resolve the cart a submission's slot carries, plus whether a frame was active. Top-level
    /// submissions (no active frame) fall back to the run frame, so every slot carries a cart and
    /// the active frame is `Some` during its step. `run_frame` is established by `ensure_run_frame`
    /// before the first submission, so the fallback is always `Some`. The `framed` flag (the active
    /// frame was present) drives `submit_node`'s fresh-vs-in-flight queue split.
    pub(in crate::machine::execute) fn submission_cart(&self) -> (Rc<CallArena>, bool) {
        let framed = self.ambient.active_frame.is_some();
        let cart = self.ambient.active_frame.clone().unwrap_or_else(|| {
            self.ambient
                .run_frame
                .clone()
                .expect("run_frame established by ensure_run_frame before any submission")
        });
        (cart, framed)
    }

    /// Whether the run frame is established. The workload mints it (adopting the run scope) on the
    /// first run-lifetime submission via [`Self::set_run_frame`].
    pub(in crate::machine::execute) fn has_run_frame(&self) -> bool {
        self.ambient.run_frame.is_some()
    }

    /// Borrow the run frame cart (the non-dying frame adopting the run root scope). A top-level
    /// submission carries it as the slot's cart, so the root re-projects from it as `Yoked` rather
    /// than anchoring at `'run` — see [`KoanRuntime::resolve_node_scope`](super::runtime::KoanRuntime).
    pub(in crate::machine::execute) fn run_frame_ref(&self) -> Option<&Rc<CallArena>> {
        self.ambient.run_frame.as_ref()
    }

    /// Install the run frame the workload minted by adopting the top-level run scope. Idempotent at
    /// the call site (the workload guards on [`Self::has_run_frame`]).
    pub(in crate::machine::execute) fn set_run_frame(&mut self, frame: Rc<CallArena>) {
        self.ambient.run_frame = Some(frame);
    }
}
