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
//! See design/per-call-region/README.md and design/execution/README.md.

use std::rc::Rc;

use crate::machine::CallFrame;

use super::nodes::NodePayload;
use super::runtime::KoanRuntime;

/// The ambient per-step context the driver carries while realizing a decided
/// [`Outcome`](super::outcome::Outcome). Concrete Koan types: the driver is the workload, so the
/// erasure the scheduler core needs is unnecessary here.
#[derive(Default)]
pub(in crate::machine::execute) struct AmbientContext {
    /// Active per-call cart (`Rc<CallFrame>`) of the slot currently being executed. See
    /// [per-call-region/frames.md § Active-frame propagation](../../../design/per-call-region/frames.md#active-frame-propagation).
    active_frame: Option<Rc<CallFrame>>,
    /// Per-slot reserve frame for the running step. `None` between slot steps. See
    /// [per-call-region/frames.md § Ping-pong reserve frame](../../../design/per-call-region/frames.md#ping-pong-reserve-frame).
    active_reserve: Option<Rc<CallFrame>>,
    /// The run frame: a non-dying frame adopting the top-level run scope, lazily built on the first
    /// run-lifetime submission. Top-level slots carry it as their `frame` cart, so `active_frame` is
    /// never `None` during a top-level step and a body's re-dispatch against its own scope is
    /// uniformly framed (Yoked) at every depth.
    run_frame: Option<Rc<CallFrame>>,
    /// The executing slot's opaque workload payload (scope handle + lexical chain), installed per
    /// step. A body that re-dispatches *against its own scope*, or that needs the ambient chain,
    /// reads it back through [`KoanRuntime::active_payload`]. `None` between slot steps.
    active_payload: Option<NodePayload>,
    /// Whether the slot currently executing already carries a kept return contract — i.e. it is a
    /// tail call *within* an established chain. A deferred-return FN dispatched here is a subsequent
    /// tail call whose own contract would be discarded by the keep-first rule, so it skips resolving
    /// its (possibly async `Expression`-form) return type and just tail-replaces its body. Installed
    /// per step by [`KoanRuntime::with_slot_step`]; read via
    /// [`SchedulerView::in_contract_chain`](super::dispatch::SchedulerView::in_contract_chain).
    active_in_contract_chain: bool,
}

/// The previous ambient values a slot step displaces — restored by
/// [`KoanRuntime::with_slot_step`] on every exit path, normal return and unwind alike.
struct SlotStepSave {
    prev_frame: Option<Rc<CallFrame>>,
    prev_payload: Option<NodePayload>,
    /// Saved so nested slot runs (combinator finish closures) don't inherit the
    /// outer slot's reserve frame.
    prev_reserve: Option<Rc<CallFrame>>,
    prev_in_contract_chain: bool,
}

/// The frames of a just-finished step, returned by [`KoanRuntime::with_slot_step`]: the slot's frame
/// *at step end* plus the post-step reserve. An in-step invoke may have swapped the ambient
/// `active_frame`, so this returned `prev_frame`, not the ambient `active_frame`, is authoritative.
pub(in crate::machine::execute) struct PostStep {
    /// The slot's cart at step end. Always present: `with_slot_step` installs the node's cart and
    /// an invoke never empties `active_frame` — a `FreshTail` placement mints its own cart via
    /// `CallFrame::new`, never touching the live active cart — so the slot's own cart rides
    /// through. The Replace arm reinstalls / rotates with it.
    pub(in crate::machine::execute) prev_frame: Rc<CallFrame>,
    /// The slot's reserve frame at step end (see ping-pong reserve rotation).
    pub(in crate::machine::execute) post_step_reserve: Option<Rc<CallFrame>>,
}

impl AmbientContext {
    /// Borrow the active per-call cart — the witness the workload binds a `Yoked` slot's
    /// re-anchored scope borrow to.
    pub(in crate::machine::execute) fn active_frame_ref(&self) -> Option<&Rc<CallFrame>> {
        self.active_frame.as_ref()
    }

    pub(in crate::machine::execute) fn active_payload(&self) -> Option<&NodePayload> {
        self.active_payload.as_ref()
    }

    /// Whether the executing slot already carries a kept return contract — set only by the
    /// slot-step bracket. Read via
    /// [`SchedulerView::in_contract_chain`](super::dispatch::SchedulerView::in_contract_chain).
    pub(in crate::machine::execute) fn in_contract_chain(&self) -> bool {
        self.active_in_contract_chain
    }

    /// Install the slot's frame/payload/reserve and contract-chain flag for one step, returning the
    /// displaced values.
    fn install_slot_step(
        &mut self,
        node_frame: Rc<CallFrame>,
        node_reserve: Option<Rc<CallFrame>>,
        node_payload: NodePayload,
        in_contract_chain: bool,
    ) -> SlotStepSave {
        SlotStepSave {
            prev_frame: self.active_frame.replace(node_frame),
            prev_reserve: std::mem::replace(&mut self.active_reserve, node_reserve),
            prev_payload: self.active_payload.replace(node_payload),
            prev_in_contract_chain: std::mem::replace(
                &mut self.active_in_contract_chain,
                in_contract_chain,
            ),
        }
    }

    /// Swap the saved values back in, returning the step-end frame and reserve — the raw material
    /// for a [`PostStep`]. Never panics: the unwind backstop runs it mid-panic.
    fn restore_slot_step(
        &mut self,
        save: SlotStepSave,
    ) -> (Option<Rc<CallFrame>>, Option<Rc<CallFrame>>) {
        let step_end_frame = std::mem::replace(&mut self.active_frame, save.prev_frame);
        let step_end_reserve = std::mem::replace(&mut self.active_reserve, save.prev_reserve);
        self.active_payload = save.prev_payload;
        self.active_in_contract_chain = save.prev_in_contract_chain;
        (step_end_frame, step_end_reserve)
    }
}

/// Unwind backstop for [`KoanRuntime::with_slot_step`]: restores the saved ambient values if the
/// step body panics. On the normal path `save` is taken out first, so the drop is a no-op.
struct SlotStepBracket<'a, 'run> {
    runtime: &'a mut KoanRuntime<'run>,
    save: Option<SlotStepSave>,
}

impl Drop for SlotStepBracket<'_, '_> {
    fn drop(&mut self) {
        if let Some(save) = self.save.take() {
            let _ = self.runtime.ambient.restore_slot_step(save);
        }
    }
}

/// Unwind backstop for [`KoanRuntime::with_active_frame`]: puts the displaced ambient frame back on
/// every exit path. This one restores on the normal path too — there is no data to hand back, so
/// the drop is the single restore point.
struct ActiveFrameBracket<'a, 'run> {
    runtime: &'a mut KoanRuntime<'run>,
    prev: Option<Option<Rc<CallFrame>>>,
}

impl Drop for ActiveFrameBracket<'_, '_> {
    fn drop(&mut self) {
        if let Some(prev) = self.prev.take() {
            self.runtime.ambient.active_frame = prev;
        }
    }
}

impl<'run> KoanRuntime<'run> {
    /// Bracket one slot step: install `node_frame` / `node_reserve` / `node_payload` and the
    /// contract-chain flag as the ambient values, run `body`, restore the previous values, and
    /// return `body`'s result alongside the [`PostStep`] the Replace arm consumes. Restore is a
    /// bracket by construction — an early return restores on the way out, and an unwind restores
    /// through the backstop's `Drop` (which discards the `PostStep` data).
    ///
    /// `post_step_reserve` carries the slot's reserve at step end, which the Replace arm reads to
    /// decide rotation: with a new frame, the post-step reserve is two iterations old and gets
    /// dropped; without one, it rides along on the reinstalled node. A `FreshTail` invoke never
    /// draws on the reserve, so it always reads back exactly what was installed.
    ///
    /// The `expect` asserts the "every step runs against a cart" invariant: the bracket installs
    /// the node's non-optional cart and an invoke never touches the reserve, only ever mints its
    /// own fresh cart, so `active_frame` is `Some` for the whole step. It stays `Option` because it
    /// is legitimately `None` *between* steps.
    pub(in crate::machine::execute) fn with_slot_step<R>(
        &mut self,
        node_frame: Rc<CallFrame>,
        node_reserve: Option<Rc<CallFrame>>,
        node_payload: NodePayload,
        in_contract_chain: bool,
        body: impl FnOnce(&mut Self) -> R,
    ) -> (R, PostStep) {
        let save = self.ambient.install_slot_step(
            node_frame,
            node_reserve,
            node_payload,
            in_contract_chain,
        );
        let mut bracket = SlotStepBracket {
            runtime: self,
            save: Some(save),
        };
        let result = body(&mut *bracket.runtime);
        let save = bracket
            .save
            .take()
            .expect("the save is consumed exactly once, here");
        let (step_end_frame, step_end_reserve) = bracket.runtime.ambient.restore_slot_step(save);
        (
            result,
            PostStep {
                prev_frame: step_end_frame.expect(
                    "a step runs against a cart; an invoke reuses the reserve, never the active",
                ),
                post_step_reserve: step_end_reserve,
            },
        )
    }

    /// Borrow the executing slot's opaque workload payload — the accessor the workload reads its
    /// name-resolution state (scope handle + lexical chain) back through. `None` between slot steps.
    pub(in crate::machine::execute) fn active_payload(&self) -> Option<&NodePayload> {
        self.ambient.active_payload()
    }

    /// Whether a slot step is currently installed (a non-`None` ambient payload). The workload reads
    /// this to decide whether to default a submission's chain to the ambient one or synthesize a
    /// detached chain (test fixtures / top level).
    pub(in crate::machine::execute) fn has_active_payload(&self) -> bool {
        self.ambient.active_payload.is_some()
    }

    /// Active slot's frame `Rc`. See
    /// [per-call-region/frames.md § Active-frame propagation](../../../design/per-call-region/frames.md#active-frame-propagation).
    pub(in crate::machine::execute) fn current_frame(&self) -> Option<Rc<CallFrame>> {
        self.ambient.active_frame.clone()
    }

    /// Borrow the active per-call cart — the witness the workload binds a `Yoked` slot's
    /// re-anchored scope borrow to. Mirrors [`Self::current_frame`] but hands back a borrow, not a
    /// clone.
    pub(in crate::machine::execute) fn active_frame_ref(&self) -> Option<&Rc<CallFrame>> {
        self.ambient.active_frame_ref()
    }

    /// Bracket `frame` as the ambient cart for the duration of `body` — the sub-slot dispatch in
    /// [`dispatch_body`](Self::dispatch_body) inherits it rather than the caller's — restoring the
    /// previous cart on every exit path, unwind included.
    pub(in crate::machine::execute) fn with_active_frame<R>(
        &mut self,
        frame: Rc<CallFrame>,
        body: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let prev = self.ambient.active_frame.replace(frame);
        let bracket = ActiveFrameBracket {
            runtime: self,
            prev: Some(prev),
        };
        body(&mut *bracket.runtime)
    }

    /// Take the slot's reserve cart, leaving none. Unconsumed by production this phase — a
    /// `FreshTail` invoke mints its own cart via `CallFrame::new` and never draws on the reserve —
    /// so this is test-only, exercised by the ambient-bracket tests that assert the reserve is
    /// empty around a step.
    #[cfg(test)]
    pub(in crate::machine::execute) fn take_active_reserve(&mut self) -> Option<Rc<CallFrame>> {
        self.ambient.active_reserve.take()
    }

    /// Resolve the cart a submission's slot carries, plus whether a frame was active. Top-level
    /// submissions (no active frame) fall back to the run frame, so every slot carries a cart and
    /// the active frame is `Some` during its step. `run_frame` is established by `ensure_run_frame`
    /// before the first submission, so the fallback is always `Some`. The `framed` flag (the active
    /// frame was present) drives `alloc_node`'s fresh-vs-in-flight queue split.
    pub(in crate::machine::execute) fn submission_cart(&self) -> (Rc<CallFrame>, bool) {
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
    pub(in crate::machine::execute) fn run_frame_ref(&self) -> Option<&Rc<CallFrame>> {
        self.ambient.run_frame.as_ref()
    }

    /// Install the run frame the workload minted by adopting the top-level run scope. Idempotent at
    /// the call site (the workload guards on [`Self::has_run_frame`]).
    pub(in crate::machine::execute) fn set_run_frame(&mut self, frame: Rc<CallFrame>) {
        self.ambient.run_frame = Some(frame);
    }
}
