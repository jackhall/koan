use crate::runtime::machine::{CombineFinish, NodeId, Scope};
use crate::ast::KExpression;

use super::super::nodes::{Node, NodeWork};
use super::dep_graph::work_owned_edges;
use super::Scheduler;

/// Walk `scope` and its outer chain, looking for a function in `functions[expr.untyped_key()]`
/// whose `pre_run` extractor returns `Some(name)` for `expr`. The first such name wins.
/// Submission-time install lets a later sibling park on the placeholder before the producer
/// slot is popped from the FIFO.
fn extract_pre_run_name<'a>(expr: &KExpression<'a>, scope: &'a Scope<'a>) -> Option<String> {
    let key = expr.untyped_key();
    let mut current: Option<&Scope<'a>> = Some(scope);
    while let Some(s) = current {
        let functions = s.functions.borrow();
        if let Some(bucket) = functions.get(&key) {
            for f in bucket.iter() {
                if let Some(extractor) = f.pre_run {
                    if let Some(name) = extractor(expr) {
                        return Some(name);
                    }
                }
            }
        }
        drop(functions);
        current = s.outer;
    }
    None
}

impl<'a> Scheduler<'a> {
    /// Submit an unresolved expression for the scheduler to dispatch + execute against
    /// `scope`. The only public way to add work; `Bind`/`Combine` are internal scaffolding
    /// spawned during a `Dispatch` node's run.
    pub fn add_dispatch(&mut self, expr: KExpression<'a>, scope: &'a Scope<'a>) -> NodeId {
        self.add(NodeWork::Dispatch(expr), scope)
    }

    /// Schedule a `Combine` slot. See `SchedulerHandle::add_combine`.
    pub fn add_combine(
        &mut self,
        deps: Vec<NodeId>,
        scope: &'a Scope<'a>,
        finish: CombineFinish<'a>,
    ) -> NodeId {
        self.add(NodeWork::Combine { deps, finish }, scope)
    }

    pub(in crate::runtime::machine::execute) fn add(&mut self, work: NodeWork<'a>, scope: &'a Scope<'a>) -> NodeId {
        let owned_edges = work_owned_edges(&work);
        let no_deps = owned_edges.is_empty();
        // Submission-time install lets a later sibling park on the placeholder before the
        // producer slot is popped from the FIFO. No-op for non-Dispatch work and for
        // Dispatch shapes whose picked function has no `pre_run`.
        let placeholder_install: Option<String> = match &work {
            NodeWork::Dispatch(expr) => extract_pre_run_name(expr, scope),
            _ => None,
        };
        // Inherit the active slot's frame so sub-slots spawned during a user-fn body's run
        // keep that body's per-call arena alive until they finalize.
        let frame = self.active_frame.clone();
        // Pre-filter owned-edge producers to those not yet terminal — only those
        // need a wake installed. Already-terminal producers are skipped because
        // their notify-walk has already happened. `DepGraph` stays oblivious to
        // results storage; the filter lives at the call site.
        let pending_producers: Vec<NodeId> = owned_edges
            .iter()
            .map(|e| e.node_id())
            .filter(|p| !self.is_result_ready(*p))
            .collect();
        let idx = match self.free_list.pop() {
            Some(i) => {
                self.nodes[i] = Some(Node { work, scope, frame, function: None });
                self.results[i] = None;
                self.deps.reset_slot_deps(NodeId(i), owned_edges, &pending_producers);
                i
            }
            None => {
                let i = self.nodes.len();
                self.nodes.push(Some(Node { work, scope, frame, function: None }));
                self.results.push(None);
                self.deps.extend_for_new_slot(NodeId(i), owned_edges, &pending_producers);
                i
            }
        };
        // Install before enqueueing: the queued slot's `run_dispatch` will idempotently
        // re-install. A failure here (e.g. `Rebind` collision) is surfaced later by
        // `install_dispatch_placeholder` rather than aborting `add`.
        if let Some(name) = placeholder_install {
            let _ = scope.install_placeholder(name, NodeId(idx));
        }
        if pending_producers.is_empty() {
            if self.active_frame.is_none() && no_deps {
                self.queues.push_fresh(idx);
            } else {
                self.queues.push_in_flight_submit(idx);
            }
        }
        NodeId(idx)
    }
}
