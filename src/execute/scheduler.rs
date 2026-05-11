use std::collections::VecDeque;
use std::rc::Rc;

use crate::dispatch::{
    CallArena, CombineFinish, KError, KObject, NodeId, Scope, SchedulerHandle,
};
use crate::parse::KExpression;

use super::nodes::{DepEdge, Node, NodeOutput, NodeWork};

mod execute;
mod invoke;
mod submit;
#[cfg(test)]
mod tests;

pub(crate) use invoke::substitute_params;

/// A dynamic DAG of dispatch and execution work. The parser submits `Dispatch` nodes for each
/// top-level expression; running a `Dispatch` may add child `Dispatch`/`Bind`/`Combine`
/// nodes, and builtin bodies holding `&mut dyn SchedulerHandle` can also add `Dispatch` nodes.
///
/// The execute loop drains two queues: internal `ready_set` (populated by the notify-walk
/// when a producer's terminal write decrements every dependent's `pending_deps` to zero) and
/// the top-level FIFO `queue` (submission order for top-level dispatches). Cycles are
/// statically prevented because every new node's `NodeId` is strictly greater than every
/// node it can depend on.
///
/// Each node carries the scope it should run against (`Node::scope`). Sub-nodes default to
/// the spawning node's scope; user-fn invocation installs a per-call child scope via
/// `NodeStep::Replace`.
///
/// See design/execution-model.md and design/memory-model.md.
pub struct Scheduler<'a> {
    pub(super) nodes: Vec<Option<Node<'a>>>,
    pub(super) results: Vec<Option<NodeOutput<'a>>>,
    /// Top-level dispatches submitted via `add_dispatch`. Internal Bind/Combine slots
    /// arrive on `ready_set` instead.
    pub(super) queue: VecDeque<usize>,
    /// Drained ahead of `queue` so internal work is consumed before the next top-level
    /// submission is dispatched.
    pub(super) ready_set: VecDeque<usize>,
    /// 1:1 with `nodes`: forward edges (producer -> consumer slot indices). Cleared on
    /// `free()` so a reused slot doesn't inherit phantom edges.
    pub(super) notify_list: Vec<Vec<usize>>,
    /// 1:1 with `nodes`: count of deps whose terminal result hasn't yet been observed by
    /// this slot's notify-decrement. Reaches zero -> slot pushed onto `ready_set`.
    pub(super) pending_deps: Vec<usize>,
    /// 1:1 with `nodes`: backward edges (consumer -> producer slots), tagged by kind.
    /// `DepEdge::Owned` marks a sub-slot this slot is responsible for reclaiming
    /// (Bind subs, Combine deps, Lift's `from`); `DepEdge::Notify` marks a sibling
    /// producer this slot only parked on for wake notification (§1 single-Identifier
    /// short-circuit, §8 replay-park). `notify_list` is the forward analogue;
    /// `free()` walks this sidecar but recurses only into `Owned` so park edges can
    /// never transit the reclaim walk into unrelated slots. Cleared by `run_bind` /
    /// `run_combine` after they eagerly free their deps on the success path.
    pub(super) dep_edges: Vec<Vec<DepEdge>>,
    /// Reclaimed slot indices. `add()` pulls from here before extending the vecs, so
    /// transient-node reclamation gives constant scheduler memory across tail-recursive
    /// bodies.
    pub(super) free_list: Vec<usize>,
    /// Frame Rc of the slot currently being executed. Read via `SchedulerHandle::current_frame`
    /// so frame-creating builtins (MATCH) can chain it onto their new frame; see
    /// [memory-model.md § Per-call-frame chaining](../../design/memory-model.md#per-call-frame-chaining-for-builtin-built-frames).
    pub(super) active_frame: Option<Rc<CallArena>>,
}

impl<'a> Scheduler<'a> {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            results: Vec::new(),
            queue: VecDeque::new(),
            ready_set: VecDeque::new(),
            notify_list: Vec::new(),
            pending_deps: Vec::new(),
            dep_edges: Vec::new(),
            free_list: Vec::new(),
            active_frame: None,
        }
    }

    pub fn len(&self) -> usize { self.nodes.len() }
    pub fn is_empty(&self) -> bool { self.nodes.is_empty() }

    /// True iff slot `id` holds a terminal result. An errored sub counts as ready — the
    /// parent short-circuits on it in `run_bind`/`run_combine`.
    pub(in crate::execute) fn is_result_ready(&self, id: NodeId) -> bool {
        matches!(
            self.results.get(id.index()).and_then(|o| o.as_ref()),
            Some(NodeOutput::Value(_)) | Some(NodeOutput::Err(_))
        )
    }

    /// Retrieve the resolved result for a top-level dispatch. Only safe on IDs returned by
    /// `add_dispatch`; internal slots may have been eagerly freed by their parent.
    pub fn read_result(&self, id: NodeId) -> Result<&'a KObject<'a>, &KError> {
        match self.results[id.index()]
            .as_ref()
            .expect("result must be ready by the time it's read")
        {
            NodeOutput::Value(v) => Ok(v),
            NodeOutput::Err(e) => Err(e),
        }
    }

    /// Convenience wrapper for the value-only path: panics on `Err`.
    pub fn read(&self, id: NodeId) -> &'a KObject<'a> {
        match self.read_result(id) {
            Ok(v) => v,
            Err(e) => panic!("read called on errored node: {e}"),
        }
    }
}

impl<'a> Default for Scheduler<'a> {
    fn default() -> Self { Self::new() }
}

impl<'a> SchedulerHandle<'a> for Scheduler<'a> {
    fn add_dispatch(&mut self, expr: KExpression<'a>, scope: &'a Scope<'a>) -> NodeId {
        Scheduler::add(self, NodeWork::Dispatch(expr), scope)
    }

    fn add_combine(
        &mut self,
        deps: Vec<NodeId>,
        scope: &'a Scope<'a>,
        finish: CombineFinish<'a>,
    ) -> NodeId {
        Scheduler::add_combine(self, deps, scope, finish)
    }

    /// Active slot's frame `Rc<CallArena>`, set by `execute` for the duration of each
    /// slot's run. Frame-creating builtins (MATCH) clone this Rc into the new frame so the
    /// call-site arena stays alive while the new frame is in use.
    fn current_frame(&self) -> Option<Rc<CallArena>> {
        self.active_frame.clone()
    }
}
