//! Slot-table state pulled out of `Scheduler<'a>`. A single `slots` vector of
//! [`SlotState`] enums encodes the per-slot lifecycle: every slot moves
//! through `alloc_slot -> take_for_run -> reinstall* -> finalize -> free_one`.
//!
//! ## Invariants
//!
//! - `alloc_slot` is the only path that picks an index (recycle from
//!   `free_list` or extend `slots`).
//! - `slots` is wrapped in [`SlotVec<T>`], which only impls `Index<NodeId>` /
//!   `IndexMut<NodeId>`, so a `NodeId` always names a live slot.
//! - `free_one` is the sole pusher onto `free_list`. Outer `Scheduler`
//!   orchestrates the notify-walk and cascade-free across this store and
//!   `DepGraph`.

use std::ops::{Index, IndexMut};
use std::rc::Rc;

use crate::machine::core::kfunction::body::ErasedContract;
use crate::machine::core::{CallArena, LexicalFrame};
use crate::machine::model::Carried;
use crate::machine::model::Parseable;
use crate::machine::KError;
use crate::machine::NodeId;

use super::super::nodes::{Frame, LiftState, Node, NodeOutput, NodeScope, NodeWork};

/// `Vec`-backed slot store keyed by [`NodeId`]. `NodeId`s are minted only
/// by [`NodeStore::alloc_slot`].
struct SlotVec<T>(Vec<T>);

impl<T> SlotVec<T> {
    fn new() -> Self {
        Self(Vec::new())
    }
    fn push(&mut self, v: T) {
        self.0.push(v);
    }
    fn len(&self) -> usize {
        self.0.len()
    }
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    fn get(&self, id: NodeId) -> Option<&T> {
        self.0.get(id.index())
    }
    fn iter(&self) -> impl Iterator<Item = &T> {
        self.0.iter()
    }
}

impl<T> Index<NodeId> for SlotVec<T> {
    type Output = T;
    fn index(&self, id: NodeId) -> &T {
        &self.0[id.index()]
    }
}

impl<T> IndexMut<NodeId> for SlotVec<T> {
    fn index_mut(&mut self, id: NodeId) -> &mut T {
        &mut self.0[id.index()]
    }
}

enum SlotState<'a> {
    PreRun(Node<'a>),
    /// Node payload has been moved out by `take_for_run`. A matching
    /// `reinstall*` / `finalize` / `free_one` exits this state.
    Running,
    Done(NodeOutput<'a>),
    /// Distinct from `Running` so the cascade-free walk's idempotency
    /// guard can be precise about "already freed".
    Free,
}

pub(in crate::machine::execute::scheduler) struct NodeStore<'a> {
    slots: SlotVec<SlotState<'a>>,
    /// Reclaimed slot indices. `alloc_slot` pulls from here before
    /// extending `slots`, giving constant scheduler memory across
    /// tail-recursive bodies.
    free_list: Vec<NodeId>,
    /// Per-consumer side-channel: producers that have fired since this
    /// slot's last poll. Populated only for `NodeWork::Dispatch`
    /// consumers; drained on entry to `run_dispatch`. Indexed in
    /// lockstep with `slots`.
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

    /// The only path that picks an index. `DepGraph::install_for_slot`
    /// mirrors the recycle-vs.-extend choice via
    /// `consumer.index() < notify_list.len()`.
    pub(super) fn alloc_slot(&mut self, node: Node<'a>) -> NodeId {
        match self.free_list.pop() {
            Some(id) => {
                self.slots[id] = SlotState::PreRun(node);
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

    /// Panics if the slot wasn't `PreRun`.
    pub(super) fn take_for_run(&mut self, id: NodeId) -> Node<'a> {
        match std::mem::replace(&mut self.slots[id], SlotState::Running) {
            SlotState::PreRun(node) => node,
            _ => panic!("scheduler must not revisit a completed node"),
        }
    }

    /// Tail-call path: reuse the slot index for a new node payload.
    pub(super) fn reinstall(&mut self, id: NodeId, node: Node<'a>) {
        self.slots[id] = SlotState::PreRun(node);
    }

    /// Replace the node payload with a fresh per-call frame; the slot stores its scope as a
    /// payload-less [`NodeScope::Yoked`] re-projected from the co-located `frame` cart. See
    /// [per-call-arena-protocol.md § Slot-table scope handle](../../../../design/per-call-arena-protocol.md#slot-table-scope-handle).
    pub(super) fn reinstall_with_frame(
        &mut self,
        id: NodeId,
        cart: Rc<CallArena>,
        reserve: Option<Rc<CallArena>>,
        work: NodeWork<'a>,
        contract: Option<ErasedContract>,
        chain: Rc<LexicalFrame>,
    ) {
        // The tail-replace slot's scope is always this `cart`'s own child, so store it as a
        // payload-less `NodeScope::Yoked` and let the read boundary re-project it from the
        // co-located `cart` each step — no persisted `&'a` to dangle across a TCO reset.
        self.slots[id] = SlotState::PreRun(Node {
            work,
            scope: NodeScope::Yoked,
            frame: Some(Frame {
                cart,
                reserve,
                contract,
            }),
            chain,
        });
    }

    /// Callers must pair this with the dep-graph notify-walk so consumers
    /// wake atomically with the write.
    pub(super) fn finalize(&mut self, id: NodeId, output: NodeOutput<'a>) {
        self.slots[id] = SlotState::Done(output);
    }

    /// Idempotent on already-`Free` slots when paired with the cascade-free
    /// walk's `is_reclaimed` guard. Pairs with the `notify_list[id]` /
    /// `dep_edges[id]` free-time clears in `DepGraph`.
    pub(super) fn free_one(&mut self, id: NodeId) {
        self.slots[id] = SlotState::Free;
        self.recent_wakes[id].clear();
        self.free_list.push(id);
    }

    pub(super) fn is_result_ready(&self, id: NodeId) -> bool {
        matches!(self.slots.get(id), Some(SlotState::Done(_)))
    }

    /// Only safe on IDs whose slot has been finalized; internal slots may
    /// have been eagerly freed by their parent.
    pub(super) fn read_result(&self, id: NodeId) -> Result<Carried<'a>, &KError> {
        match &self.slots[id] {
            &SlotState::Done(NodeOutput::Value(c)) => Ok(c),
            SlotState::Done(NodeOutput::Err(e)) => Err(e),
            _ => panic!("result must be ready by the time it's read"),
        }
    }

    pub(super) fn read(&self, id: NodeId) -> Carried<'a> {
        match self.read_result(id) {
            Ok(c) => c,
            Err(e) => panic!("read called on errored node: {e}"),
        }
    }

    /// Scan for slots still parked (`PreRun`) after the work queues drained — each
    /// is a node waiting on a dependency that can no longer fire (a dependency
    /// cycle). Returns `(count, sample)` where `sample` summarizes the first such
    /// node, or `None` when every slot is terminal (`Done`) or reclaimed (`Free`).
    pub(super) fn unresolved(&self) -> Option<(usize, String)> {
        let mut count = 0usize;
        let mut expr_sample: Option<String> = None;
        let mut fallback_sample: Option<String> = None;
        for slot in self.slots.iter() {
            if let SlotState::PreRun(node) = slot {
                count += 1;
                match &node.work {
                    NodeWork::Dispatch { expr, state } if expr_sample.is_none() => {
                        // Parked `Keyworded` slots null out `expr` once a
                        // Track installs; the working expression lives on
                        // the state.
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

    pub(super) fn len(&self) -> usize {
        self.slots.len()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub(super) fn is_live(&self, id: NodeId) -> bool {
        matches!(self.slots[id], SlotState::PreRun(_))
    }

    /// Returns true for any non-`Done` state so the cascade-free walk does
    /// not double-push onto `free_list`. Assumes `is_live` has already
    /// excluded `PreRun` upstream.
    pub(super) fn is_reclaimed(&self, id: NodeId) -> bool {
        !matches!(self.slots[id], SlotState::Done(_))
    }

    /// Notify-walk transition: if `consumer` is `Lift(Pending(producer))`,
    /// stamp it to `Lift(Ready(_))` by cloning the producer's terminal.
    /// `Err` goes through `clone_for_propagation`. No-op otherwise.
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

    /// Record that `producer` just terminalized into the consumer slot's
    /// `recent_wakes`. No-op unless `consumer` is `PreRun` with
    /// `NodeWork::Dispatch` — `Combine` / `Catch` / `Lift` run a fixed
    /// closure on counter-zero and don't need per-edge wake attribution.
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

    /// Drain the producers that fired since the slot's last poll. Called by
    /// `run_dispatch` on entry via `Scheduler::take_recent_wakes`; the
    /// side-channel stays empty for non-`Dispatch` work by construction in
    /// `push_recent_wake`.
    pub(in crate::machine::execute::scheduler) fn take_recent_wakes(
        &mut self,
        consumer: NodeId,
    ) -> Vec<NodeId> {
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

    /// Returns `None` if the slot has already terminalized.
    #[cfg(test)]
    pub(super) fn chain_of(&self, id: NodeId) -> Option<Rc<LexicalFrame>> {
        match self.slots.get(id) {
            Some(SlotState::PreRun(node)) => Some(node.chain.clone()),
            _ => None,
        }
    }
}
