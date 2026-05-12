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
//! **Run-window invariant.** `take_for_run(idx)` takes `nodes[idx]`,
//! leaving it `None` for the duration of a slot's run. The matching exit
//! is either `reinstall(idx, node)` (Replace), `finalize(idx, output)`
//! (Done), or — if the slot is being freed — `free_one(idx)`. The
//! intermediate `nodes[idx] == None` state is internal to this module.
//!
//! **Terminal-write invariant.** `finalize(idx, output)` is the only path
//! that writes `Some(NodeOutput::Value(_) | NodeOutput::Err(_))` into
//! `results[idx]`. The outer `Scheduler::finalize` pairs this write with
//! the notify-walk via `DepGraph::drain_notify`.
//!
//! **Reclaim invariant.** `free_one(idx)` is the only path that clears
//! `results[idx]` and pushes onto `free_list`. The cascade-free walk in
//! `Scheduler::free` invokes `free_one` per slot and consults
//! `DepGraph::owned_children` for recursion — the two sub-structs stay
//! independent, with `Scheduler::free` orchestrating across them.

use crate::runtime::machine::NodeId;
use crate::runtime::model::KObject;
use crate::runtime::machine::KError;

use super::super::nodes::{Node, NodeOutput};

/// Slot-table state for the scheduler. Three private vectors sharing an
/// index space; all mutation goes through the named methods below so the
/// recycle/extend choice, take/reinstall pairing, terminal-write, and
/// reclaim are each a single atomic body.
pub(super) struct NodeStore<'a> {
    /// Active node payloads. `Some` while a slot is live and not currently
    /// running; `None` between `take_for_run` and the matching
    /// `reinstall` / `finalize` / `free_one`.
    nodes: Vec<Option<Node<'a>>>,
    /// Terminal results. `Some(NodeOutput::Value(_))` or
    /// `Some(NodeOutput::Err(_))` once a slot has finalized; `None`
    /// otherwise (either pre-run or post-free).
    results: Vec<Option<NodeOutput<'a>>>,
    /// Reclaimed slot indices. `alloc_slot` pulls from here before
    /// extending the vecs, so transient-node reclamation gives constant
    /// scheduler memory across tail-recursive bodies.
    free_list: Vec<usize>,
}

impl<'a> NodeStore<'a> {
    pub(super) fn new() -> Self {
        Self {
            nodes: Vec::new(),
            results: Vec::new(),
            free_list: Vec::new(),
        }
    }

    /// The only path that picks an index. Pops `free_list` (recycle: writes
    /// both `nodes[i]` and `results[i]` in lockstep) or extends both vecs
    /// by one. Returns the chosen index; the recycle vs. extend choice is
    /// invisible to the caller — `DepGraph::install_for_slot` branches on
    /// `consumer.index() < notify_list.len()` to mirror it on the dep side.
    pub(super) fn alloc_slot(&mut self, node: Node<'a>) -> usize {
        match self.free_list.pop() {
            Some(i) => {
                self.nodes[i] = Some(node);
                self.results[i] = None;
                i
            }
            None => {
                let i = self.nodes.len();
                self.nodes.push(Some(node));
                self.results.push(None);
                i
            }
        }
    }

    /// Take the node payload for execution. Leaves `nodes[idx] == None`
    /// until the matching `reinstall` / `finalize` / `free_one`.
    pub(super) fn take_for_run(&mut self, idx: usize) -> Node<'a> {
        self.nodes[idx]
            .take()
            .expect("scheduler must not revisit a completed node")
    }

    /// Replace the node payload in place — the tail-call path. `NodeStep::Replace`
    /// rewrites the slot's work + frame + function without bumping the index.
    pub(super) fn reinstall(&mut self, idx: usize, node: Node<'a>) {
        self.nodes[idx] = Some(node);
    }

    /// Terminal write. The only path that lands a `NodeOutput` in
    /// `results[idx]`. Outer `Scheduler::finalize` pairs this with the
    /// notify-walk so consumers wake atomically with the write.
    pub(super) fn finalize(&mut self, idx: usize, output: NodeOutput<'a>) {
        self.results[idx] = Some(output);
    }

    /// Reclaim a single slot. The only path that clears `results[idx]`
    /// and pushes onto `free_list`. `nodes[idx]` is already `None` by the
    /// time this runs — `Scheduler::free`'s guard ensures live slots are
    /// skipped before `free_one` is called.
    pub(super) fn free_one(&mut self, idx: usize) {
        self.results[idx] = None;
        self.free_list.push(idx);
    }

    /// True iff slot `id` holds a terminal result. An errored slot counts
    /// as ready — parents short-circuit on it in `run_bind` / `run_combine`.
    pub(super) fn is_result_ready(&self, id: NodeId) -> bool {
        matches!(
            self.results.get(id.index()).and_then(|o| o.as_ref()),
            Some(NodeOutput::Value(_)) | Some(NodeOutput::Err(_))
        )
    }

    /// Retrieve the resolved result for a slot. Only safe on IDs whose slot
    /// has been finalized; internal slots may have been eagerly freed by
    /// their parent.
    pub(super) fn read_result(&self, id: NodeId) -> Result<&'a KObject<'a>, &KError> {
        match self.results[id.index()]
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
    pub(super) fn is_live(&self, idx: usize) -> bool {
        self.nodes[idx].is_some()
    }

    /// `Scheduler::free`'s already-reclaimed guard: paired with
    /// `DepGraph::is_dep_edges_empty` to detect a slot whose result was
    /// cleared and whose edges were drained, so the iterative walk doesn't
    /// double-push onto `free_list`.
    pub(super) fn is_reclaimed(&self, idx: usize) -> bool {
        self.results[idx].is_none()
    }

    /// Direct projection of the terminal result for `run_lift`. The
    /// `expect` here pins the invariant: Lift only runs after notify-walk
    /// observes a terminal write on `from`. `scheduler/finish.rs::run_lift`
    /// reaches this via `pub(super)` because both files sit under
    /// `scheduler::`.
    pub(super) fn result_slot(&self, from: NodeId) -> &NodeOutput<'a> {
        self.results[from.index()]
            .as_ref()
            .expect("Lift only runs after notify wakes it from `from`'s terminal write")
    }

    // --- Test-only helpers for synthetic-state setup. ---

    #[cfg(test)]
    pub(super) fn clear_node(&mut self, idx: usize) {
        self.nodes[idx] = None;
    }

    #[cfg(test)]
    pub(super) fn set_result(&mut self, idx: usize, output: NodeOutput<'a>) {
        self.results[idx] = Some(output);
    }

    #[cfg(test)]
    pub(super) fn result_is_some(&self, idx: usize) -> bool {
        self.results[idx].is_some()
    }

    #[cfg(test)]
    pub(super) fn result_is_none(&self, idx: usize) -> bool {
        self.results[idx].is_none()
    }

    #[cfg(test)]
    pub(super) fn free_list_snapshot(&self) -> Vec<usize> {
        self.free_list.clone()
    }

    #[cfg(test)]
    pub(super) fn free_list_len(&self) -> usize {
        self.free_list.len()
    }
}
