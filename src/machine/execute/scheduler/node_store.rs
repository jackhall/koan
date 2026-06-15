//! Slot-table state pulled out of `Scheduler<'run>`. A single `slots` vector of
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
use crate::machine::KError;
use crate::machine::NodeId;

use super::super::nodes::{CallFrame, Node, NodeScope, NodeWork};

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

enum SlotState<'run> {
    PreRun(Node<'run>),
    /// Node payload has been moved out by `take_for_run`. A matching
    /// `reinstall*` / `finalize` / `free_one` exits this state.
    Running,
    Done(Result<Carried<'run>, KError>),
    /// A bare-name forward spliced out: this slot's result *is* `producer`'s. `read_result` /
    /// `is_result_ready` follow the alias through to `producer` (which holds the sole copy). The
    /// slot's consumers were moved onto `producer`'s notify list at splice time, so `producer`'s
    /// fire wakes them directly. See [`Outcome::Forward`](super::super::outcome::Outcome).
    Aliased(NodeId),
    /// Distinct from `Running` so the cascade-free walk's idempotency
    /// guard can be precise about "already freed".
    Free,
}

/// The drain-end deadlock-sample contribution of one parked/pending slot's work.
/// `unresolved` shows the first `Preferred` (a real source expression) across all stuck slots,
/// falling back to the first `Fallback` (a generic work-shape tag) only when no slot carries an
/// expression — so a stuck `(foo bar)` always out-renders a bare `<combine>`.
enum DeadlockSample {
    Preferred(String),
    Fallback(&'static str),
}

/// Map a stuck slot's `work` to its deadlock-sample contribution. A `Some`-carrier `Wait` (a
/// dispatch decide) carries a renderable expression summary (`Preferred`); a carrier-less wait
/// (combine / catch) carries only a generic tag (`Fallback`).
fn work_deadlock_sample<'run>(work: &NodeWork<'run>) -> DeadlockSample {
    let NodeWork { carrier, .. } = work;
    match carrier {
        Some(carrier) => DeadlockSample::Preferred(carrier.clone()),
        None => DeadlockSample::Fallback("<wait>"),
    }
}

pub(in crate::machine::execute::scheduler) struct NodeStore<'run> {
    slots: SlotVec<SlotState<'run>>,
    /// Reclaimed slot indices. `alloc_slot` pulls from here before
    /// extending `slots`, giving constant scheduler memory across
    /// tail-recursive bodies.
    free_list: Vec<NodeId>,
}

impl<'run> NodeStore<'run> {
    pub(super) fn new() -> Self {
        Self {
            slots: SlotVec::new(),
            free_list: Vec::new(),
        }
    }

    /// The only path that picks an index. `DepGraph::install_for_slot`
    /// mirrors the recycle-vs.-extend choice via
    /// `consumer.index() < notify_list.len()`.
    pub(super) fn alloc_slot(&mut self, node: Node<'run>) -> NodeId {
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

    /// Panics if the slot wasn't `PreRun`.
    pub(super) fn take_for_run(&mut self, id: NodeId) -> Node<'run> {
        match std::mem::replace(&mut self.slots[id], SlotState::Running) {
            SlotState::PreRun(node) => node,
            _ => panic!("scheduler must not revisit a completed node"),
        }
    }

    /// Tail-call path: reuse the slot index for a new node payload.
    pub(super) fn reinstall(&mut self, id: NodeId, node: Node<'run>) {
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
        work: NodeWork<'run>,
        contract: Option<ErasedContract>,
        chain: Rc<LexicalFrame>,
    ) {
        // The tail-replace slot's scope is always this `cart`'s own child, so store it as a
        // payload-less `NodeScope::Yoked` and let the read boundary re-project it from the
        // co-located `cart` each step — no persisted `&'run` to dangle across a TCO reset.
        self.slots[id] = SlotState::PreRun(Node {
            work,
            scope: NodeScope::Yoked,
            frame: CallFrame {
                cart,
                reserve,
                contract,
            },
            chain,
        });
    }

    /// Callers must pair this with the dep-graph notify-walk so consumers
    /// wake atomically with the write.
    pub(super) fn finalize(&mut self, id: NodeId, output: Result<Carried<'run>, KError>) {
        self.slots[id] = SlotState::Done(output);
    }

    /// Idempotent on already-`Free` slots when paired with the cascade-free
    /// walk's `is_reclaimed` guard. Pairs with the `notify_list[id]` /
    /// `dep_edges[id]` free-time clears in `DepGraph`.
    pub(super) fn free_one(&mut self, id: NodeId) {
        self.slots[id] = SlotState::Free;
        self.free_list.push(id);
    }

    /// The alias target of a spliced-out bare-name forward, or `None`. The single follow step the
    /// `Scheduler`-level [`resolve_alias`](super::Scheduler::resolve_alias) walks; resolution lives
    /// there (with `DepGraph`), not in the store. See [`scheduler::splice`](super::splice).
    pub(super) fn alias_target(&self, id: NodeId) -> Option<NodeId> {
        match self.slots.get(id) {
            Some(SlotState::Aliased(to)) => Some(*to),
            _ => None,
        }
    }

    /// Raw readiness — callers pass an already alias-resolved id.
    pub(super) fn is_result_ready(&self, id: NodeId) -> bool {
        matches!(self.slots.get(id), Some(SlotState::Done(_)))
    }

    /// Only safe on IDs whose slot has been finalized; internal slots may have been eagerly freed by
    /// their parent. Raw — callers pass an already alias-resolved id.
    pub(super) fn read_result(&self, id: NodeId) -> Result<Carried<'run>, &KError> {
        match &self.slots[id] {
            &SlotState::Done(Ok(c)) => Ok(c),
            SlotState::Done(Err(e)) => Err(e),
            _ => panic!("result must be ready by the time it's read"),
        }
    }

    pub(super) fn read(&self, id: NodeId) -> Carried<'run> {
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
                match work_deadlock_sample(&node.work) {
                    DeadlockSample::Preferred(s) if expr_sample.is_none() => expr_sample = Some(s),
                    DeadlockSample::Fallback(s) if fallback_sample.is_none() => {
                        fallback_sample = Some(s.to_string());
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

    /// Splice a bare-name forward out: the running slot becomes an alias of `producer` (a
    /// downstream real producer). `read_result` / `is_result_ready` follow the alias; the slot's
    /// consumers were already moved onto `producer`'s notify list, so this just records the
    /// redirect. See [`Outcome::Forward`](super::super::outcome::Outcome).
    pub(super) fn alias(&mut self, id: NodeId, producer: NodeId) {
        self.slots[id] = SlotState::Aliased(producer);
    }

    // --- Test-only helpers for synthetic-state setup. ---

    #[cfg(test)]
    pub(super) fn clear_node(&mut self, id: NodeId) {
        self.slots[id] = SlotState::Running;
    }

    #[cfg(test)]
    pub(super) fn set_result(&mut self, id: NodeId, output: Result<Carried<'run>, KError>) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_wait<'r>(carrier: Option<String>) -> NodeWork<'r> {
        NodeWork {
            deps: Vec::new(),
            park_count: 0,
            cont: Box::new(|_view, _results, _idx| unreachable!("sample test never runs")),
            carrier,
        }
    }

    #[test]
    fn some_carrier_wait_prefers_the_carrier() {
        let work = sample_wait(Some("PARKED-EXPR".to_string()));
        assert!(
            matches!(work_deadlock_sample(&work), DeadlockSample::Preferred(s) if s.contains("PARKED")),
            "a Some-carrier Wait (a dispatch decide) must surface its carrier",
        );
    }

    #[test]
    fn carrier_less_wait_falls_back_to_a_tag() {
        let work = sample_wait(None);
        assert!(
            matches!(
                work_deadlock_sample(&work),
                DeadlockSample::Fallback("<wait>")
            ),
            "a carrier-less Wait must surface a generic tag, not an empty sample",
        );
    }
}
