//! Slot-table state pulled out of `Scheduler<'a>`. A single `slots` vector of
//! [`SlotState`] enums encodes the per-slot lifecycle directly: every slot
//! moves through `alloc_slot -> take_for_run -> reinstall* -> finalize ->
//! free_one`. Each transition is encapsulated by a single atomic mutator below.
//!
//! ## Invariants this module enforces
//!
//! **Index-space coherence.** `alloc_slot` is the only path that picks
//! an index: it either pops `free_list` (recycle: overwrites `slots[i]`)
//! or extends `slots` by one. A second vector that has to stay in lockstep
//! no longer exists — the prior `nodes` / `results` pair collapsed into
//! `SlotState`'s variants.
//!
//! **Type-encoded presence and lifecycle.** `slots` is wrapped in
//! [`SlotVec<T>`], which only impls `Index<NodeId>` / `IndexMut<NodeId>` —
//! raw `usize` indexing is unreachable. `SlotState`'s four variants
//! (`PreRun` / `Running` / `Done` / `Free`) make the lifecycle explicit:
//! `take_for_run` only matches `PreRun`, `finalize` only matches `Running`,
//! `free_one` overwrites any state with `Free`. The previously-ambiguous
//! `(nodes=None, results=None)` state — which conflated "running" with
//! "freed" — is no longer expressible.
//!
//! **Terminal-write invariant.** `finalize(id, output)` is the only path
//! that lands a `SlotState::Done(_)` in a slot. The outer
//! `Scheduler::finalize` pairs this write with the notify-walk via
//! `DepGraph::drain_notify`.
//!
//! **Reclaim invariant.** `free_one(id)` is the only path that stores
//! `SlotState::Free` and pushes onto `free_list`. The cascade-free walk
//! in `Scheduler::free` invokes `free_one` per slot and consults
//! `DepGraph::owned_children` for recursion — the two sub-structs stay
//! independent, with `Scheduler::free` orchestrating across them.

use std::ops::{Index, IndexMut};
use std::rc::Rc;

use crate::machine::core::{CallArena, Scope};
use crate::machine::core::kfunction::KFunction;
use crate::machine::NodeId;
use crate::machine::model::KObject;
use crate::machine::KError;

use super::super::nodes::{LiftState, Node, NodeOutput, NodeWork};

/// `Vec`-backed slot store keyed by [`NodeId`]. Exposes `push` / `len` /
/// `is_empty` / `get` and impls `Index<NodeId>` / `IndexMut<NodeId>` only —
/// raw `usize` indexing is unreachable, so the "this index names a live
/// slot" claim is carried by `NodeId`'s type. Constructors hand out
/// `NodeId` only via [`NodeStore::alloc_slot`].
struct SlotVec<T>(Vec<T>);

impl<T> SlotVec<T> {
    fn new() -> Self { Self(Vec::new()) }
    fn push(&mut self, v: T) { self.0.push(v); }
    fn len(&self) -> usize { self.0.len() }
    fn is_empty(&self) -> bool { self.0.is_empty() }
    fn get(&self, id: NodeId) -> Option<&T> { self.0.get(id.index()) }
}

impl<T> Index<NodeId> for SlotVec<T> {
    type Output = T;
    fn index(&self, id: NodeId) -> &T { &self.0[id.index()] }
}

impl<T> IndexMut<NodeId> for SlotVec<T> {
    fn index_mut(&mut self, id: NodeId) -> &mut T { &mut self.0[id.index()] }
}

/// Per-slot lifecycle state. Replaces the pre-refactor
/// `(Option<Node>, Option<NodeOutput>)` pair whose `(None, None)` state
/// conflated "running" with "freed". Transitions are constrained to the
/// `NodeStore` mutators: only `alloc_slot` produces `PreRun`, only
/// `take_for_run` produces `Running`, only `finalize` produces `Done`,
/// only `free_one` produces `Free`.
enum SlotState<'a> {
    /// Active node payload, awaiting its first (or next) run.
    PreRun(Node<'a>),
    /// Slot is mid-execution. The node payload was moved out by
    /// `take_for_run`; the matching `reinstall` (Replace) or `finalize`
    /// (Done) restores the slot to `PreRun` / `Done`.
    Running,
    /// Slot has finalized with a terminal `NodeOutput`. Parents read
    /// through `read_result`; the cascade-free walk reclaims via
    /// `free_one`.
    Done(NodeOutput<'a>),
    /// Slot index is in `free_list` and available for recycling. Distinct
    /// from `Running` so the cascade-free walk's idempotency guard can be
    /// precise about "already freed."
    Free,
}

/// Slot-table state for the scheduler. A single `slots` vector and a
/// `free_list`; all mutation goes through the named methods below so the
/// recycle/extend choice, take/reinstall pairing, terminal-write, and
/// reclaim are each a single atomic body.
pub(super) struct NodeStore<'a> {
    /// Lifecycle state per slot.
    slots: SlotVec<SlotState<'a>>,
    /// Reclaimed slot indices. `alloc_slot` pulls from here before
    /// extending `slots`, so transient-node reclamation gives constant
    /// scheduler memory across tail-recursive bodies.
    free_list: Vec<NodeId>,
}

impl<'a> NodeStore<'a> {
    pub(super) fn new() -> Self {
        Self {
            slots: SlotVec::new(),
            free_list: Vec::new(),
        }
    }

    /// The only path that picks an index. Pops `free_list` (recycle: overwrites
    /// the slot's `Free` state with `PreRun(node)`) or extends `slots` by one.
    /// Returns the chosen `NodeId`; the recycle vs. extend choice is invisible
    /// to the caller — `DepGraph::install_for_slot` branches on
    /// `consumer.index() < notify_list.len()` to mirror it on the dep side.
    pub(super) fn alloc_slot(&mut self, node: Node<'a>) -> NodeId {
        match self.free_list.pop() {
            Some(id) => {
                self.slots[id] = SlotState::PreRun(node);
                id
            }
            None => {
                let id = NodeId(self.slots.len());
                self.slots.push(SlotState::PreRun(node));
                id
            }
        }
    }

    /// Take the node payload for execution. Transitions `PreRun -> Running`;
    /// the matching `reinstall` / `finalize` / `free_one` exits the
    /// `Running` state.
    pub(super) fn take_for_run(&mut self, id: NodeId) -> Node<'a> {
        match std::mem::replace(&mut self.slots[id], SlotState::Running) {
            SlotState::PreRun(node) => node,
            _ => panic!("scheduler must not revisit a completed node"),
        }
    }

    /// Replace the node payload in place — the tail-call path. `NodeStep::Replace`
    /// rewrites the slot's work + frame + function without bumping the index.
    pub(super) fn reinstall(&mut self, id: NodeId, node: Node<'a>) {
        self.slots[id] = SlotState::PreRun(node);
    }

    /// Replace the node payload **with a fresh per-call frame**, re-anchoring the frame's
    /// per-call [`Scope`] to `'a` (the slot-storage lifetime). The owner of the
    /// `'a`-anchored claim is therefore this module, not the caller: invokers (today,
    /// `Scheduler::execute`'s `Replace` arm) no longer need to carry the SAFETY paragraph
    /// for the `&Scope<'_> → &'a Scope<'_>` re-anchor.
    ///
    /// SAFETY: `frame` is about to be stored in `self.slots[id]`, whose live span equals
    /// `'a` — the same lifetime the slot's scope reference is anchored to. So
    /// re-anchoring `frame.scope()` from its receiver-bound borrow to `'a` is witnessed
    /// by the store itself: the `Rc<CallArena>` stays in the same node payload as the
    /// `&'a Scope<'a>` it produces, so the arena heap-pinning that backs `scope_ptr`
    /// outlives every read through this `'a` reference. The previous frame held in
    /// `self.slots[id]` (if any) must have been removed by a prior `take_for_run`;
    /// callers are responsible for dropping it before invoking this entry point.
    pub(super) fn reinstall_with_frame(
        &mut self,
        id: NodeId,
        frame: Rc<CallArena>,
        work: NodeWork<'a>,
        function: Option<&'a KFunction<'a>>,
    ) {
        let scope: &'a Scope<'a> = unsafe {
            std::mem::transmute::<&Scope<'_>, &'a Scope<'a>>(frame.scope())
        };
        self.slots[id] = SlotState::PreRun(Node { work, scope, frame: Some(frame), function });
    }

    /// Terminal write. Transitions `Running -> Done(output)` — the only
    /// path that lands a `Done` variant. Outer `Scheduler::finalize` pairs
    /// this with the notify-walk so consumers wake atomically with the
    /// write.
    pub(super) fn finalize(&mut self, id: NodeId, output: NodeOutput<'a>) {
        self.slots[id] = SlotState::Done(output);
    }

    /// Reclaim a single slot. Sets `slots[id] = Free` and pushes the index
    /// onto `free_list`. Idempotent on already-`Free` slots when paired
    /// with the cascade-free walk's `is_reclaimed` guard; `Scheduler::free`
    /// also skips slots in `PreRun` (still scheduled) via `is_live`.
    pub(super) fn free_one(&mut self, id: NodeId) {
        self.slots[id] = SlotState::Free;
        self.free_list.push(id);
    }

    /// True iff slot `id` holds a terminal result. Used by parents'
    /// short-circuit checks in `run_bind` / `run_combine`.
    pub(super) fn is_result_ready(&self, id: NodeId) -> bool {
        matches!(self.slots.get(id), Some(SlotState::Done(_)))
    }

    /// Retrieve the resolved result for a slot. Only safe on IDs whose slot
    /// has been finalized; internal slots may have been eagerly freed by
    /// their parent.
    pub(super) fn read_result(&self, id: NodeId) -> Result<&'a KObject<'a>, &KError> {
        match &self.slots[id] {
            SlotState::Done(NodeOutput::Value(v)) => Ok(v),
            SlotState::Done(NodeOutput::Err(e)) => Err(e),
            _ => panic!("result must be ready by the time it's read"),
        }
    }

    /// Convenience wrapper for the value-only path: panics on `Err`.
    pub(super) fn read(&self, id: NodeId) -> &'a KObject<'a> {
        match self.read_result(id) {
            Ok(v) => v,
            Err(e) => panic!("read called on errored node: {e}"),
        }
    }

    /// Slot count (live + reclaimed). Mirrors `Vec::len`.
    pub(super) fn len(&self) -> usize {
        self.slots.len()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// `Scheduler::free`'s live-slot guard: skip slots that haven't been
    /// `take`n yet (the slot is still in the work queue and runnable).
    pub(super) fn is_live(&self, id: NodeId) -> bool {
        matches!(self.slots[id], SlotState::PreRun(_))
    }

    /// `Scheduler::free`'s already-reclaimed guard: paired with
    /// `DepGraph::is_dep_edges_empty` to detect a slot whose terminal was
    /// cleared and whose edges were drained, so the iterative walk doesn't
    /// double-push onto `free_list`. Returns true for any non-`Done`
    /// state, preserving the prior `results[id].is_none()` semantics now
    /// that `is_live` has already excluded `PreRun` upstream.
    pub(super) fn is_reclaimed(&self, id: NodeId) -> bool {
        !matches!(self.slots[id], SlotState::Done(_))
    }

    /// Notify-walk transition for the Lift two-state shape: if the consumer slot's
    /// work is `Lift(Pending(from))` with `from == producer`, stamp it to
    /// `Lift(Ready(_))` by cloning the producer's just-finalized terminal out of
    /// `slots[producer]`. The clone matches the previous `run_lift` read-side
    /// behavior (Value copies the `&'a KObject`; Err calls `clone_for_propagation`)
    /// but happens once at stamp time rather than on every Lift pop.
    ///
    /// No-op when the consumer isn't a Lift, when its work is already `Ready`, or
    /// when its `from` doesn't name this producer — invariants of the notify-walk
    /// pair imply at most one of those branches fires for each woken consumer, but
    /// the body stays defensive so a future call site can stamp speculatively.
    pub(super) fn stamp_lift_ready(&mut self, consumer: NodeId, producer: NodeId) {
        let is_lift_pending = matches!(
            &self.slots[consumer],
            SlotState::PreRun(node)
                if matches!(&node.work, NodeWork::Lift(LiftState::Pending(from)) if *from == producer),
        );
        if !is_lift_pending {
            return;
        }
        let stamped = match &self.slots[producer] {
            &SlotState::Done(NodeOutput::Value(v)) => NodeOutput::Value(v),
            SlotState::Done(NodeOutput::Err(e)) => NodeOutput::Err(e.clone_for_propagation()),
            _ => panic!("producer just finalized"),
        };
        if let SlotState::PreRun(node) = &mut self.slots[consumer] {
            if let NodeWork::Lift(state) = &mut node.work {
                *state = LiftState::Ready(stamped);
            }
        }
    }

    // --- Test-only helpers for synthetic-state setup. ---

    #[cfg(test)]
    pub(super) fn clear_node(&mut self, id: NodeId) {
        self.slots[id] = SlotState::Running;
    }

    #[cfg(test)]
    pub(super) fn set_result(&mut self, id: NodeId, output: NodeOutput<'a>) {
        self.slots[id] = SlotState::Done(output);
    }

    #[cfg(test)]
    pub(super) fn result_is_some(&self, id: NodeId) -> bool {
        matches!(self.slots[id], SlotState::Done(_))
    }

    #[cfg(test)]
    pub(super) fn result_is_none(&self, id: NodeId) -> bool {
        !matches!(self.slots[id], SlotState::Done(_))
    }

    #[cfg(test)]
    pub(super) fn free_list_snapshot(&self) -> Vec<NodeId> {
        self.free_list.clone()
    }

    #[cfg(test)]
    pub(super) fn free_list_len(&self) -> usize {
        self.free_list.len()
    }
}
