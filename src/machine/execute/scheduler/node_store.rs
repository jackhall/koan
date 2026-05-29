//! Slot-table state pulled out of `Scheduler<'a>`. A single `slots` vector of
//! [`SlotState`] enums encodes the per-slot lifecycle: every slot moves
//! through `alloc_slot -> take_for_run -> reinstall* -> finalize -> free_one`.
//!
//! ## Invariants
//!
//! - **Index-space coherence.** `alloc_slot` is the only path that picks an
//!   index (recycle from `free_list` or extend `slots`).
//! - **Type-encoded indexing.** `slots` is wrapped in [`SlotVec<T>`], which
//!   only impls `Index<NodeId>` / `IndexMut<NodeId>`; raw `usize` indexing is
//!   unreachable, so a `NodeId` always names a live slot.
//! - **Lifecycle by variant.** `take_for_run` only matches `PreRun`,
//!   `finalize` only produces `Done`, `free_one` only produces `Free`.
//! - **Terminal-write / reclaim pairing.** `finalize` is the sole producer of
//!   `Done`; `free_one` is the sole producer of `Free` and the sole pusher
//!   onto `free_list`. Outer `Scheduler` orchestrates the notify-walk and
//!   cascade-free across this store and `DepGraph`.

use std::ops::{Index, IndexMut};
use std::rc::Rc;

use crate::machine::core::{CallArena, LexicalFrame, Scope};
use crate::machine::core::kfunction::KFunction;
use crate::machine::NodeId;
use crate::machine::model::KObject;
use crate::machine::model::Parseable;
use crate::machine::KError;

use super::super::nodes::{LiftState, Node, NodeOutput, NodeWork};

/// `Vec`-backed slot store keyed by [`NodeId`]. Only impls
/// `Index<NodeId>` / `IndexMut<NodeId>`, so raw `usize` indexing is
/// unreachable and a `NodeId` always names a live slot. `NodeId`s are
/// minted only by [`NodeStore::alloc_slot`].
struct SlotVec<T>(Vec<T>);

impl<T> SlotVec<T> {
    fn new() -> Self { Self(Vec::new()) }
    fn push(&mut self, v: T) { self.0.push(v); }
    fn len(&self) -> usize { self.0.len() }
    fn is_empty(&self) -> bool { self.0.is_empty() }
    fn get(&self, id: NodeId) -> Option<&T> { self.0.get(id.index()) }
    fn iter(&self) -> impl Iterator<Item = &T> { self.0.iter() }
}

impl<T> Index<NodeId> for SlotVec<T> {
    type Output = T;
    fn index(&self, id: NodeId) -> &T { &self.0[id.index()] }
}

impl<T> IndexMut<NodeId> for SlotVec<T> {
    fn index_mut(&mut self, id: NodeId) -> &mut T { &mut self.0[id.index()] }
}

/// Per-slot lifecycle state. Transitions are constrained to the
/// `NodeStore` mutators: only `alloc_slot` produces `PreRun`, only
/// `take_for_run` produces `Running`, only `finalize` produces `Done`,
/// only `free_one` produces `Free`.
enum SlotState<'a> {
    PreRun(Node<'a>),
    /// Node payload has been moved out by `take_for_run`. A matching
    /// `reinstall*` / `finalize` / `free_one` exits this state.
    Running,
    Done(NodeOutput<'a>),
    /// Slot index is in `free_list`. Distinct from `Running` so the
    /// cascade-free walk's idempotency guard can be precise about
    /// "already freed".
    Free,
}

pub(super) struct NodeStore<'a> {
    slots: SlotVec<SlotState<'a>>,
    /// Reclaimed slot indices. `alloc_slot` pulls from here before
    /// extending `slots`, so transient-node reclamation gives constant
    /// scheduler memory across tail-recursive bodies.
    free_list: Vec<NodeId>,
    /// Per-consumer side-channel: producers that have fired since this
    /// slot's last poll. Populated by `push_recent_wake` from the
    /// notify-walk in `Scheduler::finalize` only when the consumer's
    /// work is a `NodeWork::Dispatch` (any `DispatchState` variant);
    /// drained by `take_recent_wakes` on entry to
    /// `run_dispatch`. Indexed by `NodeId` in lockstep with
    /// `slots`; grown by `alloc_slot` (extend arm) and cleared by
    /// `free_one` (so recycled slots inherit an empty Vec while
    /// retaining capacity from the prior owner's wake pattern).
    /// Step 2 of `roadmap/dispatch_fix/stateful-dispatch-02-recent-wakes.md`.
    recent_wakes: SlotVec<Vec<NodeId>>,
}

impl<'a> NodeStore<'a> {
    pub(super) fn new() -> Self {
        Self {
            slots: SlotVec::new(),
            free_list: Vec::new(),
            recent_wakes: SlotVec::new(),
        }
    }

    /// The only path that picks an index. Recycles from `free_list` if
    /// non-empty, otherwise extends `slots`. `DepGraph::install_for_slot`
    /// mirrors the recycle-vs.-extend choice via
    /// `consumer.index() < notify_list.len()`.
    pub(super) fn alloc_slot(&mut self, node: Node<'a>) -> NodeId {
        match self.free_list.pop() {
            Some(id) => {
                self.slots[id] = SlotState::PreRun(node);
                // `recent_wakes[id]` was cleared by `free_one` when the
                // previous owner was reclaimed; the inner Vec is already
                // empty (capacity retained for the new owner).
                id
            }
            None => {
                let id = NodeId(self.slots.len());
                self.slots.push(SlotState::PreRun(node));
                // Grow the wake side-channel in lockstep with `slots` so
                // every live `NodeId` indexes a valid inner Vec.
                self.recent_wakes.push(Vec::new());
                id
            }
        }
    }

    /// `PreRun -> Running`. Panics if the slot wasn't `PreRun`.
    pub(super) fn take_for_run(&mut self, id: NodeId) -> Node<'a> {
        match std::mem::replace(&mut self.slots[id], SlotState::Running) {
            SlotState::PreRun(node) => node,
            _ => panic!("scheduler must not revisit a completed node"),
        }
    }

    /// Tail-call path: rewrite the slot's payload in place without
    /// allocating a new index.
    pub(super) fn reinstall(&mut self, id: NodeId, node: Node<'a>) {
        self.slots[id] = SlotState::PreRun(node);
    }

    /// Replace the node payload **with a fresh per-call frame**, re-anchoring
    /// the frame's per-call [`Scope`] to `'a` (the slot-storage lifetime).
    /// Owning the `'a`-anchored claim here means callers do not have to.
    ///
    /// SAFETY: `frame` is about to be stored in `self.slots[id]`, whose live
    /// span equals `'a`. Re-anchoring `frame.scope()` from its receiver-bound
    /// borrow to `'a` is witnessed by the store itself: the `Rc<CallArena>`
    /// stays in the same node payload as the `&'a Scope<'a>` it produces, so
    /// the arena heap-pinning that backs `scope_ptr` outlives every read
    /// through this `'a` reference. Any previous frame in `self.slots[id]`
    /// must have been removed by a prior `take_for_run`.
    pub(super) fn reinstall_with_frame(
        &mut self,
        id: NodeId,
        frame: Rc<CallArena>,
        reserve_frame: Option<Rc<CallArena>>,
        work: NodeWork<'a>,
        function: Option<&'a KFunction<'a>>,
        chain: Rc<LexicalFrame>,
    ) {
        let scope: &'a Scope<'a> = unsafe {
            std::mem::transmute::<&Scope<'_>, &'a Scope<'a>>(frame.scope())
        };
        self.slots[id] =
            SlotState::PreRun(Node { work, scope, frame: Some(frame), reserve_frame, function, chain });
    }

    /// Terminal write: the only path that produces `Done`. Callers must
    /// pair this with the dep-graph notify-walk so consumers wake
    /// atomically with the write.
    pub(super) fn finalize(&mut self, id: NodeId, output: NodeOutput<'a>) {
        self.slots[id] = SlotState::Done(output);
    }

    /// Reclaim a single slot. Idempotent on already-`Free` slots when
    /// paired with the cascade-free walk's `is_reclaimed` guard.
    ///
    /// Clears the wake side-channel in O(1); inner Vec capacity is
    /// retained so a slot recycled through `alloc_slot` inherits the
    /// prior allocation. Pairs with the `notify_list[id]` /
    /// `dep_edges[id]` free-time clears in `DepGraph`.
    pub(super) fn free_one(&mut self, id: NodeId) {
        self.slots[id] = SlotState::Free;
        self.recent_wakes[id].clear();
        self.free_list.push(id);
    }

    /// True iff slot `id` holds a terminal result.
    pub(super) fn is_result_ready(&self, id: NodeId) -> bool {
        matches!(self.slots.get(id), Some(SlotState::Done(_)))
    }

    /// Retrieve the resolved result. Only safe on IDs whose slot has been
    /// finalized; internal slots may have been eagerly freed by their
    /// parent.
    pub(super) fn read_result(&self, id: NodeId) -> Result<&'a KObject<'a>, &KError> {
        match &self.slots[id] {
            SlotState::Done(NodeOutput::Value(v)) => Ok(v),
            SlotState::Done(NodeOutput::Err(e)) => Err(e),
            _ => panic!("result must be ready by the time it's read"),
        }
    }

    /// Value-only convenience wrapper; panics on `Err`.
    pub(super) fn read(&self, id: NodeId) -> &'a KObject<'a> {
        match self.read_result(id) {
            Ok(v) => v,
            Err(e) => panic!("read called on errored node: {e}"),
        }
    }

    /// Scan for slots still parked (`PreRun`) after the work queues drained — each
    /// is a node waiting on a dependency that can no longer fire (a dependency
    /// cycle). Returns `(count, sample)` where `sample` summarizes the first such
    /// node, or `None` when every slot is terminal (`Done`) or reclaimed (`Free`).
    pub(super) fn unresolved(&self) -> Option<(usize, String)> {
        let mut count = 0usize;
        // Prefer a `Dispatch`/`Bind` sample — it carries the source expression a
        // reader can act on. Fall back to a generic label only if every parked node
        // is scaffolding (Combine/Catch/Lift).
        let mut expr_sample: Option<String> = None;
        let mut fallback_sample: Option<String> = None;
        for slot in self.slots.iter() {
            if let SlotState::PreRun(node) = slot {
                count += 1;
                match &node.work {
                    NodeWork::Dispatch { expr, state } if expr_sample.is_none() => {
                        // Parked `Keyworded` slots null out `expr` once a
                        // Track installs; the working expression lives on
                        // the state. Prefer the state-carried carrier when
                        // present, fall back to `expr` otherwise.
                        let carrier = state.parked_carrier_expr().unwrap_or(expr);
                        expr_sample = Some(carrier.summarize());
                    }
                    NodeWork::Combine { .. } if fallback_sample.is_none() => {
                        fallback_sample = Some("<combine>".to_string());
                    }
                    NodeWork::Catch { .. } if fallback_sample.is_none() => {
                        fallback_sample = Some("<catch>".to_string());
                    }
                    NodeWork::Lift(_) if fallback_sample.is_none() => {
                        fallback_sample = Some("<lift>".to_string());
                    }
                    _ => {}
                }
            }
        }
        if count == 0 {
            return None;
        }
        Some((count, expr_sample.or(fallback_sample).unwrap_or_default()))
    }

    /// Slot count (live + reclaimed).
    pub(super) fn len(&self) -> usize {
        self.slots.len()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Cascade-free live-slot guard: slot is still scheduled (`PreRun`) and
    /// must not be reclaimed yet.
    pub(super) fn is_live(&self, id: NodeId) -> bool {
        matches!(self.slots[id], SlotState::PreRun(_))
    }

    /// Cascade-free already-reclaimed guard. Returns true for any non-`Done`
    /// state, so the iterative walk does not double-push onto `free_list`.
    /// Assumes `is_live` has already excluded `PreRun` upstream.
    pub(super) fn is_reclaimed(&self, id: NodeId) -> bool {
        !matches!(self.slots[id], SlotState::Done(_))
    }

    /// Notify-walk transition for the Lift two-state shape: if the consumer
    /// slot is `Lift(Pending(from))` with `from == producer`, stamp it to
    /// `Lift(Ready(_))` by cloning the producer's terminal — Value copies
    /// the `&'a KObject`; Err goes through `clone_for_propagation`. No-op
    /// when the consumer isn't a Pending-Lift naming this producer.
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

    /// Notify-walk side-channel: record that `producer` has just
    /// terminalized into the consumer slot's `recent_wakes`. No-op
    /// unless `consumer` is in `PreRun` and carries `NodeWork::Dispatch`
    /// (any `DispatchState` variant) — `Bind` / `Combine` / `Catch`
    /// run a fixed closure on counter-zero and don't need per-edge
    /// wake attribution, so they skip the append.
    ///
    /// Mirrors the peek-then-mutate shape of `stamp_lift_ready` so the
    /// scheduler-level loop body in `Scheduler::finalize` stays uniform
    /// across the two notify-time transitions.
    pub(super) fn push_recent_wake(&mut self, consumer: NodeId, producer: NodeId) {
        let is_dispatch_prerun = matches!(
            &self.slots[consumer],
            SlotState::PreRun(node) if matches!(&node.work, NodeWork::Dispatch { .. }),
        );
        if !is_dispatch_prerun {
            return;
        }
        self.recent_wakes[consumer].push(producer);
    }

    /// Drain `recent_wakes[consumer]` and return the producers that
    /// fired since the slot's last poll. The slot's Vec resets to
    /// `Vec::new()` — capacity retention happens between owners at
    /// `free_one` time, not across a single owner's polls. Called by
    /// `run_dispatch` on entry; non-`Dispatch` work paths
    /// never call it (the side-channel stays empty for them by
    /// construction in `push_recent_wake`).
    pub(super) fn take_recent_wakes(&mut self, consumer: NodeId) -> Vec<NodeId> {
        std::mem::take(&mut self.recent_wakes[consumer])
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

    /// Test-only chain peek. Returns `None` if the slot has already terminalized
    /// (no payload to read) — every `PreRun` slot has a chain set at submission
    /// time.
    #[cfg(test)]
    pub(super) fn chain_of(&self, id: NodeId) -> Option<Rc<LexicalFrame>> {
        match self.slots.get(id) {
            Some(SlotState::PreRun(node)) => Some(node.chain.clone()),
            _ => None,
        }
    }
}
