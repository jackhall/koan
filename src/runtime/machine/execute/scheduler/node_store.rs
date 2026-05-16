//! Slot-table state pulled out of `Scheduler<'a>`. The three private
//! vectors — `nodes`, `results`, `free_list` — share an index space and a
//! lifecycle that nothing in the type system enforced before this module:
//! every slot moves through `alloc_slot -> take_for_run -> reinstall* ->
//! finalize -> free_one`. Each transition is encapsulated by a single
//! atomic mutator below.
//!
//! ## Invariants this module enforces
//!
//! **Index-space coherence.** `alloc_slot` is the only path that picks
//! an index: it either pops `free_list` (recycle: rewrite `nodes[i]` and
//! `results[i]` in lockstep) or extends both `nodes` and `results` by one.
//! A future call site cannot grow `nodes` without `results` (or vice versa)
//! because the field access is gated behind the wrapper's surface.
//!
//! **Type-encoded presence.** `nodes` and `results` are wrapped in
//! [`SlotVec<T>`], which only impls `Index<NodeId>` / `IndexMut<NodeId>`.
//! Raw `usize` indexing is unreachable inside the module — the "this index
//! names a live slot" claim is carried by the `NodeId` type. The only way
//! to obtain a `NodeId` aimed at this store is via `alloc_slot`, so the
//! "missing-arg check above guarantees presence" pattern becomes the
//! `NodeId`'s existence.
//!
//! **Run-window invariant.** `take_for_run(id)` takes `nodes[id]`,
//! leaving it `None` for the duration of a slot's run. The matching exit
//! is either `reinstall(id, node)` (Replace), `finalize(id, output)`
//! (Done), or — if the slot is being freed — `free_one(id)`. The
//! intermediate `nodes[id] == None` state is internal to this module.
//!
//! **Terminal-write invariant.** `finalize(id, output)` is the only path
//! that writes `Some(NodeOutput::Value(_) | NodeOutput::Err(_))` into
//! `results[id]`. The outer `Scheduler::finalize` pairs this write with
//! the notify-walk via `DepGraph::drain_notify`.
//!
//! **Reclaim invariant.** `free_one(id)` is the only path that clears
//! `results[id]` and pushes onto `free_list`. The cascade-free walk in
//! `Scheduler::free` invokes `free_one` per slot and consults
//! `DepGraph::owned_children` for recursion — the two sub-structs stay
//! independent, with `Scheduler::free` orchestrating across them.

use std::ops::{Index, IndexMut};
use std::rc::Rc;

use crate::runtime::machine::core::{CallArena, Scope};
use crate::runtime::machine::core::kfunction::KFunction;
use crate::runtime::machine::NodeId;
use crate::runtime::machine::model::KObject;
use crate::runtime::machine::KError;

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

/// Slot-table state for the scheduler. Three private vectors sharing an
/// index space; all mutation goes through the named methods below so the
/// recycle/extend choice, take/reinstall pairing, terminal-write, and
/// reclaim are each a single atomic body.
pub(super) struct NodeStore<'a> {
    /// Active node payloads. `Some` while a slot is live and not currently
    /// running; `None` between `take_for_run` and the matching
    /// `reinstall` / `finalize` / `free_one`.
    nodes: SlotVec<Option<Node<'a>>>,
    /// Terminal results. `Some(NodeOutput::Value(_))` or
    /// `Some(NodeOutput::Err(_))` once a slot has finalized; `None`
    /// otherwise (either pre-run or post-free).
    results: SlotVec<Option<NodeOutput<'a>>>,
    /// Reclaimed slot indices. `alloc_slot` pulls from here before
    /// extending the vecs, so transient-node reclamation gives constant
    /// scheduler memory across tail-recursive bodies.
    free_list: Vec<NodeId>,
}

impl<'a> NodeStore<'a> {
    pub(super) fn new() -> Self {
        Self {
            nodes: SlotVec::new(),
            results: SlotVec::new(),
            free_list: Vec::new(),
        }
    }

    /// The only path that picks an index. Pops `free_list` (recycle: writes
    /// both `nodes[id]` and `results[id]` in lockstep) or extends both vecs
    /// by one. Returns the chosen `NodeId`; the recycle vs. extend choice is
    /// invisible to the caller — `DepGraph::install_for_slot` branches on
    /// `consumer.index() < notify_list.len()` to mirror it on the dep side.
    pub(super) fn alloc_slot(&mut self, node: Node<'a>) -> NodeId {
        match self.free_list.pop() {
            Some(id) => {
                self.nodes[id] = Some(node);
                self.results[id] = None;
                id
            }
            None => {
                let id = NodeId(self.nodes.len());
                self.nodes.push(Some(node));
                self.results.push(None);
                id
            }
        }
    }

    /// Take the node payload for execution. Leaves `nodes[id] == None`
    /// until the matching `reinstall` / `finalize` / `free_one`.
    pub(super) fn take_for_run(&mut self, id: NodeId) -> Node<'a> {
        self.nodes[id]
            .take()
            .expect("scheduler must not revisit a completed node")
    }

    /// Replace the node payload in place — the tail-call path. `NodeStep::Replace`
    /// rewrites the slot's work + frame + function without bumping the index.
    pub(super) fn reinstall(&mut self, id: NodeId, node: Node<'a>) {
        self.nodes[id] = Some(node);
    }

    /// Replace the node payload **with a fresh per-call frame**, re-anchoring the frame's
    /// per-call [`Scope`] to `'a` (the slot-storage lifetime). The owner of the
    /// `'a`-anchored claim is therefore this module, not the caller: invokers (today,
    /// `Scheduler::execute`'s `Replace` arm) no longer need to carry the SAFETY paragraph
    /// for the `&Scope<'_> → &'a Scope<'_>` re-anchor.
    ///
    /// SAFETY: `frame` is about to be stored in `self.nodes[id]`, whose live span equals
    /// `'a` — the same lifetime the slot's scope reference is anchored to. So
    /// re-anchoring `frame.scope()` from its receiver-bound borrow to `'a` is witnessed
    /// by the store itself: the `Rc<CallArena>` stays in the same node payload as the
    /// `&'a Scope<'a>` it produces, so the arena heap-pinning that backs `scope_ptr`
    /// outlives every read through this `'a` reference. The previous frame held in
    /// `self.nodes[id]` (if any) must have been removed by a prior `take_for_run`;
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
        self.nodes[id] = Some(Node { work, scope, frame: Some(frame), function });
    }

    /// Terminal write. The only path that lands a `NodeOutput` in
    /// `results[id]`. Outer `Scheduler::finalize` pairs this with the
    /// notify-walk so consumers wake atomically with the write.
    pub(super) fn finalize(&mut self, id: NodeId, output: NodeOutput<'a>) {
        self.results[id] = Some(output);
    }

    /// Reclaim a single slot. The only path that clears `results[id]`
    /// and pushes onto `free_list`. `nodes[id]` is already `None` by the
    /// time this runs — `Scheduler::free`'s guard ensures live slots are
    /// skipped before `free_one` is called.
    pub(super) fn free_one(&mut self, id: NodeId) {
        self.results[id] = None;
        self.free_list.push(id);
    }

    /// True iff slot `id` holds a terminal result. An errored slot counts
    /// as ready — parents short-circuit on it in `run_bind` / `run_combine`.
    pub(super) fn is_result_ready(&self, id: NodeId) -> bool {
        matches!(
            self.results.get(id).and_then(|o| o.as_ref()),
            Some(NodeOutput::Value(_)) | Some(NodeOutput::Err(_))
        )
    }

    /// Retrieve the resolved result for a slot. Only safe on IDs whose slot
    /// has been finalized; internal slots may have been eagerly freed by
    /// their parent.
    pub(super) fn read_result(&self, id: NodeId) -> Result<&'a KObject<'a>, &KError> {
        match self.results[id]
            .as_ref()
            .expect("result must be ready by the time it's read")
        {
            NodeOutput::Value(v) => Ok(v),
            NodeOutput::Err(e) => Err(e),
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
        self.nodes.len()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// `Scheduler::free`'s live-slot guard: skip slots that haven't been
    /// `take`n yet (the slot is still in the work queue or actively running).
    pub(super) fn is_live(&self, id: NodeId) -> bool {
        self.nodes[id].is_some()
    }

    /// `Scheduler::free`'s already-reclaimed guard: paired with
    /// `DepGraph::is_dep_edges_empty` to detect a slot whose result was
    /// cleared and whose edges were drained, so the iterative walk doesn't
    /// double-push onto `free_list`.
    pub(super) fn is_reclaimed(&self, id: NodeId) -> bool {
        self.results[id].is_none()
    }

    /// Notify-walk transition for the Lift two-state shape: if the consumer slot's
    /// work is `Lift(Pending(from))` with `from == producer`, stamp it to
    /// `Lift(Ready(_))` by cloning the producer's just-finalized terminal out of
    /// `results[producer]`. The clone matches the previous `run_lift` read-side
    /// behavior (Value copies the `&'a KObject`; Err calls `clone_for_propagation`)
    /// but happens once at stamp time rather than on every Lift pop.
    ///
    /// No-op when the consumer isn't a Lift, when its work is already `Ready`, or
    /// when its `from` doesn't name this producer — invariants of the notify-walk
    /// pair imply at most one of those branches fires for each woken consumer, but
    /// the body stays defensive so a future call site can stamp speculatively.
    pub(super) fn stamp_lift_ready(&mut self, consumer: NodeId, producer: NodeId) {
        let is_lift_pending = matches!(
            self.nodes[consumer].as_ref().map(|n| &n.work),
            Some(NodeWork::Lift(LiftState::Pending(from))) if *from == producer,
        );
        if !is_lift_pending {
            return;
        }
        let stamped = match self.results[producer]
            .as_ref()
            .expect("producer just finalized")
        {
            &NodeOutput::Value(v) => NodeOutput::Value(v),
            NodeOutput::Err(e) => NodeOutput::Err(e.clone_for_propagation()),
        };
        let node = self.nodes[consumer]
            .as_mut()
            .expect("checked is_lift_pending above");
        if let NodeWork::Lift(state) = &mut node.work {
            *state = LiftState::Ready(stamped);
        }
    }

    // --- Test-only helpers for synthetic-state setup. ---

    #[cfg(test)]
    pub(super) fn clear_node(&mut self, id: NodeId) {
        self.nodes[id] = None;
    }

    #[cfg(test)]
    pub(super) fn set_result(&mut self, id: NodeId, output: NodeOutput<'a>) {
        self.results[id] = Some(output);
    }

    #[cfg(test)]
    pub(super) fn result_is_some(&self, id: NodeId) -> bool {
        self.results[id].is_some()
    }

    #[cfg(test)]
    pub(super) fn result_is_none(&self, id: NodeId) -> bool {
        self.results[id].is_none()
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
