//! Ambient per-step context — the driver-side state a pure DAG runtime does not own.
//!
//! [`Scheduler`](crate::scheduler::Scheduler) is a workload-independent DAG of dispatch/execution
//! work, so the values that float across a single step — the active per-call frame, the run frame,
//! the executing slot's opaque payload, the park state — live here on the driver's
//! [`Host`](super::harness::Host), the [`KoanWorkload`](super::harness::KoanWorkload) side of the
//! split and so free to name concrete Koan types. The host brackets this context per step
//! ([`Host::with_slot_step`]); step code reads it back through the methods below.
//!
//! See [per-call-region](../../../design/per-call-region/README.md) and
//! [execution](../../../design/execution/README.md).

use std::cell::RefCell;
use std::rc::Rc;

use crate::machine::model::RunRegistries;
use crate::machine::{CallFrame, RunWriter};

use super::harness::Host;
use super::nodes::NodePayload;
use super::obligation::{ParkState, ReturnObligation};

/// The ambient per-step context the host carries while a decided
/// [`Outcome`](super::outcome::Outcome) is realized.
#[derive(Default)]
pub(in crate::machine::execute) struct AmbientContext {
    /// Active per-call cart of the slot currently being executed. See
    /// [per-call-region/frames.md § Active-frame propagation](../../../design/per-call-region/frames.md#active-frame-propagation).
    active_frame: Option<Rc<CallFrame>>,
    /// A non-dying frame adopting the top-level run scope, lazily minted on the first run-lifetime
    /// submission. Top-level slots carry it as their cart, so `active_frame` is never `None` during
    /// a top-level step and a body's re-dispatch against its own scope is uniformly framed (Yoked)
    /// at every depth.
    run_frame: Option<Rc<CallFrame>>,
    /// The executing slot's opaque workload payload (scope handle + lexical chain). `None` between
    /// slot steps.
    active_payload: Option<NodePayload>,
    /// The park state the executing slot carries: the declared-return obligation (the slot is a
    /// tail call *within* an established chain exactly when that is `Some`) and a leading-carrying
    /// tail's block frame. Held behind a `RefCell` because the depositors reach it through
    /// `&AmbientContext` (via [`DecideCtx`](super::decide::DecideCtx)).
    active_park: RefCell<ParkState>,
}

/// The previous ambient values a slot step displaces — restored by [`Host::with_slot_step`] on
/// every exit path, normal return and unwind alike.
struct SlotStepSave {
    prev_frame: Option<Rc<CallFrame>>,
    prev_payload: Option<NodePayload>,
    prev_park: ParkState,
}

impl AmbientContext {
    /// The witness the workload binds a `Yoked` slot's re-anchored scope borrow to.
    pub(in crate::machine::execute) fn active_frame_ref(&self) -> Option<&Rc<CallFrame>> {
        self.active_frame.as_ref()
    }

    pub(in crate::machine::execute) fn active_payload(&self) -> Option<&NodePayload> {
        self.active_payload.as_ref()
    }

    /// The run's lookup state, owned by the run frame. `ensure_run_frame` installs that frame
    /// before any step runs, so the registries are always reachable from step code.
    pub(in crate::machine::execute) fn registries(&self) -> &RunRegistries {
        self.registries_opt()
            .expect("run frame (and its registries) established before any step")
    }

    /// [`Self::registries`] as an `Option` — a detached read, valid before the run frame exists
    /// and after the run ends.
    pub(in crate::machine::execute) fn registries_opt(&self) -> Option<&RunRegistries> {
        self.run_frame.as_ref().and_then(|frame| frame.registries())
    }

    /// The run's output sink, owned by the run frame exactly as the type registry is, and reached
    /// the same way.
    pub(in crate::machine::execute) fn writer(&self) -> &RunWriter {
        self.run_frame
            .as_ref()
            .and_then(|frame| frame.writer())
            .expect("run frame (and its writer) established before any step")
    }

    /// Install a woken slot's whole park state — the obligation its park established and the block
    /// frame it kept alive — as the ambient state for this step.
    pub(in crate::machine::execute) fn deposit_park(&self, park: ParkState) {
        *self.active_park.borrow_mut() = park;
    }

    /// Take the active obligation out, leaving the slot obligation-free.
    pub(in crate::machine::execute) fn take_obligation(&self) -> Option<ReturnObligation> {
        self.active_park.borrow_mut().obligation.take()
    }

    /// Keep-first and park propagation read copies onward while the current step keeps its own.
    pub(in crate::machine::execute) fn current_obligation(&self) -> Option<ReturnObligation> {
        self.active_park.borrow().obligation
    }

    /// Hand the block frame a leading-carrying tail is about to park on to the park itself, so the
    /// finish reads it back at wake instead of capturing it. One deposit is spent by exactly one
    /// park: the decide deposits it immediately before returning its `Outcome::Park`, and the
    /// finish that park wakes takes it back out.
    pub(in crate::machine::execute) fn deposit_block_frame(&self, frame: Rc<CallFrame>) {
        let mut park = self.active_park.borrow_mut();
        debug_assert!(
            park.block_frame.is_none(),
            "a deposited block frame is taken by the finish its park wakes"
        );
        park.block_frame = Some(frame);
    }

    /// Take the parked block frame out — read at wake by the finish the deposit was made for, and
    /// at park install by the continuation that carries it across the dormancy.
    pub(in crate::machine::execute) fn take_block_frame(&self) -> Option<Rc<CallFrame>> {
        self.active_park.borrow_mut().block_frame.take()
    }

    /// The park state a replacement or a fresh park carries onward: the chain's established
    /// obligation, plus a block frame this step's decide just deposited.
    pub(in crate::machine::execute) fn park_state(&self) -> ParkState {
        ParkState {
            obligation: self.current_obligation(),
            block_frame: self.take_block_frame(),
        }
    }

    pub(in crate::machine::execute) fn has_run_frame(&self) -> bool {
        self.run_frame.is_some()
    }

    /// The non-dying frame adopting the run root scope. A top-level submission carries it as the
    /// slot's cart, so the root re-projects from it as `Yoked` rather than anchoring at `'run` —
    /// see [`Host::resolve_node_scope`](super::harness::Host).
    pub(in crate::machine::execute) fn run_frame_ref(&self) -> Option<&Rc<CallFrame>> {
        self.run_frame.as_ref()
    }

    pub(in crate::machine::execute) fn set_run_frame(&mut self, frame: Rc<CallFrame>) {
        self.run_frame = Some(frame);
    }

    /// Resolve the cart a submission's slot carries, plus whether a frame was active. Top-level
    /// submissions (no active frame) fall back to the run frame, which `ensure_run_frame`
    /// establishes before the first submission, so every slot carries a cart. The `framed` flag
    /// drives `alloc_node`'s fresh-vs-in-flight queue split.
    pub(in crate::machine::execute) fn submission_cart(&self) -> (Rc<CallFrame>, bool) {
        let framed = self.active_frame.is_some();
        let cart = self.active_frame.clone().unwrap_or_else(|| {
            self.run_frame
                .clone()
                .expect("run_frame established by ensure_run_frame before any submission")
        });
        (cart, framed)
    }

    /// Install the slot's frame/payload for one step and reset the park state to empty (the step's
    /// wrapper deposits its own), returning the displaced values.
    fn install_slot_step(
        &mut self,
        node_frame: Rc<CallFrame>,
        node_payload: NodePayload,
    ) -> SlotStepSave {
        SlotStepSave {
            prev_frame: self.active_frame.replace(node_frame),
            prev_payload: self.active_payload.replace(node_payload),
            prev_park: std::mem::take(self.active_park.get_mut()),
        }
    }

    /// Never panics: the unwind backstop runs it mid-panic.
    fn restore_slot_step(&mut self, save: SlotStepSave) {
        self.active_frame = save.prev_frame;
        self.active_payload = save.prev_payload;
        *self.active_park.get_mut() = save.prev_park;
    }
}

/// Unwind backstop for [`Host::with_slot_step`]. On the normal path `save` is taken out first, so
/// the drop is a no-op.
struct SlotStepBracket<'a, 'run> {
    host: &'a mut Host<'run>,
    save: Option<SlotStepSave>,
}

impl Drop for SlotStepBracket<'_, '_> {
    fn drop(&mut self) {
        if let Some(save) = self.save.take() {
            self.host.ambient.restore_slot_step(save);
        }
    }
}

/// Unwind backstop for [`Host::with_active_frame`]. This one restores on the normal path too —
/// there is no data to hand back, so the drop is the single restore point.
struct ActiveFrameBracket<'a, 'run> {
    host: &'a mut Host<'run>,
    prev: Option<Option<Rc<CallFrame>>>,
}

impl Drop for ActiveFrameBracket<'_, '_> {
    fn drop(&mut self) {
        if let Some(prev) = self.prev.take() {
            self.host.ambient.active_frame = prev;
        }
    }
}

impl<'run> Host<'run> {
    /// Bracket one slot step. The whole step — decide, effects, and the apply that realizes the
    /// outcome — runs inside the bracket, so the step-end frame and the deposited obligation are
    /// read off the ambient context by the apply itself. Restore is a bracket by construction: an
    /// early return restores on the way out, an unwind through the backstop's `Drop`.
    ///
    /// The bracket installs the node's non-optional cart, so `active_frame` is `Some` for the whole
    /// step; it stays `Option` because it is legitimately `None` *between* steps
    /// ([frames.md § Active-frame propagation](../../../design/per-call-region/frames.md#active-frame-propagation)).
    pub(in crate::machine::execute) fn with_slot_step<R>(
        &mut self,
        node_frame: Rc<CallFrame>,
        node_payload: NodePayload,
        body: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let save = self.ambient.install_slot_step(node_frame, node_payload);
        let mut bracket = SlotStepBracket {
            host: self,
            save: Some(save),
        };
        let result = body(&mut *bracket.host);
        let save = bracket
            .save
            .take()
            .expect("the save is consumed exactly once, here");
        bracket.host.ambient.restore_slot_step(save);
        result
    }

    /// Bracket `frame` as the ambient cart for the duration of `body`, restoring the previous cart
    /// on every exit path, unwind included.
    pub(in crate::machine::execute) fn with_active_frame<R>(
        &mut self,
        frame: Rc<CallFrame>,
        body: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let prev = self.ambient.active_frame.replace(frame);
        let bracket = ActiveFrameBracket {
            host: self,
            prev: Some(prev),
        };
        body(&mut *bracket.host)
    }
}
