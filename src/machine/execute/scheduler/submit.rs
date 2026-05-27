use std::rc::Rc;

use crate::machine::{BindingIndex, CatchFinish, CombineFinish, LexicalFrame, NodeId, Scope};
use crate::machine::model::ast::KExpression;

use super::super::nodes::{Node, NodeWork, work_park_producers};
use super::dep_graph::work_owned_edges;
use super::Scheduler;

/// Submission-time placeholder install info. Walks `scope` and its outer chain looking
/// for a function in `functions[expr.untyped_key()]` whose `pre_run` extractor returns
/// `Some(name)` for `expr`. The first such name wins; the picked function's
/// `is_nominal_binder` flag rides through too so the install at `add_with_chain` can
/// stamp the matching D7 visibility carve-out on the [`BindingIndex`]. Submission-time
/// install lets a later sibling park on the placeholder before the producer slot is
/// popped from the FIFO.
struct PreRunInstall {
    name: String,
    is_nominal_binder: bool,
}

fn extract_pre_run_install<'a>(
    expr: &KExpression<'a>,
    scope: &'a Scope<'a>,
) -> Option<PreRunInstall> {
    let key = expr.untyped_key();
    let mut current: Option<&Scope<'a>> = Some(scope);
    while let Some(s) = current {
        let functions_guard = s.bindings().functions();
        if let Some(bucket) = functions_guard.get(&key) {
            for (f, _) in bucket.iter() {
                if let Some(extractor) = f.pre_run {
                    if let Some(name) = extractor(expr) {
                        return Some(PreRunInstall {
                            name,
                            is_nominal_binder: f.is_nominal_binder,
                        });
                    }
                }
            }
        }
        drop(functions_guard);
        current = s.outer;
    }
    None
}

impl<'a> Scheduler<'a> {
    /// Submit an unresolved expression for the scheduler to dispatch + execute against
    /// `scope`. The only public way to add work; `Bind`/`Combine` are internal scaffolding
    /// spawned during a `Dispatch` node's run.
    ///
    /// Routes through [`Self::add`]; the ambient-vs-root chain decision lives there.
    pub fn add_dispatch(&mut self, expr: KExpression<'a>, scope: &'a Scope<'a>) -> NodeId {
        self.add(NodeWork::Dispatch(expr), scope)
    }

    /// Schedule a `Combine` slot. `owned_subs` are sub-Dispatches this Combine
    /// allocated itself (cascade-freed on success); `park_producers` are
    /// existing sibling slots whose values it splices but does not own (kept
    /// alive past the Combine's success via `Notify` edges). The Combine's
    /// finish closure sees results in `[park_producers..., owned_subs...]`
    /// order. See `SchedulerHandle::add_combine`.
    pub fn add_combine(
        &mut self,
        owned_subs: Vec<NodeId>,
        park_producers: Vec<NodeId>,
        scope: &'a Scope<'a>,
        finish: CombineFinish<'a>,
    ) -> NodeId {
        let park_count = park_producers.len();
        let mut deps = park_producers;
        deps.extend(owned_subs);
        self.add(NodeWork::Combine { deps, park_count, finish }, scope)
    }

    /// Schedule a `Catch` slot. See `SchedulerHandle::add_catch`.
    pub fn add_catch(
        &mut self,
        from: NodeId,
        scope: &'a Scope<'a>,
        finish: CatchFinish<'a>,
    ) -> NodeId {
        self.add(NodeWork::Catch { from, finish }, scope)
    }

    /// Inherit-from-ambient entry point. Sub-dispatches inside a builtin body (CONS-
    /// head, FN signature subs, list/dict literal items, ...) route here so they
    /// pick up the executing slot's `active_chain` without each call site naming
    /// it. Block-entry sites that compute their own chain call
    /// [`Self::add_with_chain`] directly.
    ///
    /// When there is no ambient chain (REPL-style submission, test fixtures
    /// poking at scheduler internals), synthesize a detached chain so the visibility
    /// predicate treats every scope as "complete" and the submission reads through
    /// to every previously-bound name. This is what lets a fixture call
    /// `run(scope, "(LET x = 1)")` then `run_one(scope, parse_one("x"))` and have
    /// the second submission see `x`.
    pub(super) fn add(&mut self, work: NodeWork<'a>, scope: &'a Scope<'a>) -> NodeId {
        if self.active_chain.is_some() {
            self.add_with_chain(work, scope, None)
        } else {
            self.add_with_chain(work, scope, Some(LexicalFrame::detached()))
        }
    }

    /// Single funnel for node creation. `explicit_chain` is `Some` for
    /// `enter_block`-routed submissions (top-level, MODULE / SIG body, TRY body
    /// success-as-block, FN body invoke), `None` for everything else (which then
    /// inherits `self.active_chain`).
    ///
    /// Debug-asserts that *some* chain is in scope — top-level routes through
    /// `enter_block(root.id, ...)` so the only path that hits this with both
    /// `explicit_chain: None` and `active_chain: None` is a bug.
    pub(super) fn add_with_chain(
        &mut self,
        work: NodeWork<'a>,
        scope: &'a Scope<'a>,
        explicit_chain: Option<Rc<LexicalFrame>>,
    ) -> NodeId {
        let owned_edges = work_owned_edges(&work);
        let no_owned = owned_edges.is_empty();
        let placeholder_install: Option<PreRunInstall> = match &work {
            NodeWork::Dispatch(expr) => extract_pre_run_install(expr, scope),
            _ => None,
        };
        let frame = self.active_frame.clone();
        let chain = explicit_chain
            .or_else(|| self.active_chain.clone())
            .expect(
                "every dispatched node has a chain — submission outside enter_block / \
                 ambient active_chain is a bug",
            );
        // Stamp the placeholder with the SAME `BindingIndex` the eventual `register_*`
        // call at finalize will install — `idx` is this slot's lexical position; the
        // D7 nominal-binder carve-out (true for STRUCT / named UNION / SIG / FUNCTOR /
        // MODULE) rides on the picked binder's `is_nominal_binder` flag.
        let bind_index_for_placeholder = placeholder_install.as_ref().map(|p| BindingIndex {
            idx: chain.index,
            nominal_binder: p.is_nominal_binder,
        });
        let pending_owned: Vec<NodeId> = owned_edges
            .iter()
            .map(|e| e.node_id())
            .filter(|p| !self.is_result_ready(*p))
            .collect();
        let pending_park: Vec<NodeId> = work_park_producers(&work)
            .iter()
            .copied()
            .filter(|p| !self.is_result_ready(*p))
            .collect();
        let no_park = work_park_producers(&work).is_empty();
        let id = self
            .store
            .alloc_slot(Node { work, scope, frame, function: None, chain });
        self.deps.install_for_slot(id, owned_edges, &pending_owned);
        for p in &pending_park {
            self.deps.add_park_edge(*p, id);
        }
        if let Some(install) = placeholder_install {
            // `bind_index_for_placeholder` is `Some` whenever `placeholder_install` is.
            let bind_index = bind_index_for_placeholder.unwrap_or(BindingIndex::BUILTIN);
            let _ = scope.install_placeholder(install.name, id, bind_index);
        }
        if pending_owned.is_empty() && pending_park.is_empty() {
            if self.active_frame.is_none() && no_owned && no_park {
                self.queues.push_fresh(id.index());
            } else {
                self.queues.push_in_flight_submit(id.index());
            }
        }
        id
    }
}
