use crate::runtime::model::{KObject, Parseable};
use crate::runtime::machine::{BodyResult, CombineFinish, Frame, KError, KFuture, NodeId, Scope};
use crate::ast::{ExpressionPart, KExpression};

use super::super::nodes::{NodeOutput, NodeStep, NodeWork};
use super::super::scheduler::Scheduler;

impl<'a> Scheduler<'a> {
    pub(in crate::runtime::machine::execute) fn run_bind(
        &mut self,
        mut expr: KExpression<'a>,
        subs: Vec<(usize, NodeId)>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> Result<NodeStep<'a>, KError> {
        // Sub slots stay in `dep_edges[idx]` on the error path so chain-free at
        // finalize reclaims them; eager free is the success-path optimization.
        for (_, dep_id) in &subs {
            if let Err(e) = self.read_result(*dep_id) {
                let frame = Frame {
                    function: "<bind>".to_string(),
                    expression: expr.summarize(),
                };
                let propagated = e.clone_for_propagation().with_frame(frame);
                return Ok(NodeStep::Done(NodeOutput::Err(propagated)));
            }
        }
        let dep_indices: Vec<usize> = subs.iter().map(|(_, d)| d.index()).collect();
        for (part_idx, dep_id) in subs {
            let value = self.read(dep_id);
            expr.parts[part_idx] = ExpressionPart::Future(value);
        }
        // Spliced `Future(&'a KObject)` references survive `results[dep] = None`
        // because the objects live in arenas tied to lexical scope. Reclaim happens
        // before `scope.dispatch` so the dispatched body's `add()` calls can recycle
        // the indices immediately.
        self.reclaim_deps(idx, dep_indices);
        let future = scope.dispatch(expr)?;
        Ok(self.invoke_to_step(future, scope, idx))
    }

    /// Success-path eager free; the error path leaves deps for chain-free at slot drop.
    /// `dep_edges[idx].clear()` is sound here: Bind / Combine slots at reclaim time hold
    /// only `Owned` edges (their `subs` / `deps`, all spawned by this slot). Notify
    /// edges land only on Dispatch slots via the bare-name short-circuit / replay-park
    /// in `run_dispatch`, never on Bind /
    /// Combine, so clearing the list cannot drop a wake intent.
    fn reclaim_deps(&mut self, idx: usize, dep_indices: Vec<usize>) {
        self.deps.clear_dep_edges(idx);
        for d in dep_indices {
            self.free(d);
        }
    }

    /// Run a `Combine` slot: short-circuit on the first errored dep using the same
    /// frame-attached propagation as `run_bind`, then call `finish` over the dep values
    /// and decode the returned `BodyResult` (Value, Tail, or Err) into a `NodeStep`
    /// using the same dispatch as `invoke_to_step`. Deps are eagerly freed on the
    /// success path; the error path leaves them in `dep_edges[idx]` for
    /// chain-free at slot drop.
    pub(in crate::runtime::machine::execute) fn run_combine(
        &mut self,
        deps: Vec<NodeId>,
        finish: CombineFinish<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> NodeStep<'a> {
        // The closure carries its own framing context (e.g. "<list>", "<dict>") via its
        // capture; the Combine machinery only handles dep-error propagation, which uses
        // the generic "<combine>" frame to match `run_bind`'s "<bind>" convention.
        let make_frame = || Frame {
            function: "<combine>".to_string(),
            expression: "combine".to_string(),
        };
        for dep in &deps {
            if let Err(e) = self.read_result(*dep) {
                let propagated = e.clone_for_propagation().with_frame(make_frame());
                return NodeStep::Done(NodeOutput::Err(propagated));
            }
        }
        // Pre-collect refs so `finish` (which holds `&mut self` via the trait object)
        // doesn't reborrow `self` for reads.
        let values: Vec<&'a KObject<'a>> = deps.iter().map(|d| self.read(*d)).collect();
        let dep_indices: Vec<usize> = deps.iter().map(|d| d.index()).collect();
        let body = finish(scope, self, &values);
        self.reclaim_deps(idx, dep_indices);
        match body {
            BodyResult::Value(v) => NodeStep::Done(NodeOutput::Value(v)),
            BodyResult::Tail { expr, frame, function } => NodeStep::Replace {
                work: NodeWork::Dispatch(expr),
                frame,
                function,
            },
            BodyResult::DeferTo(id) => self.defer_to_lift(idx, id),
            BodyResult::Err(e) => NodeStep::Done(NodeOutput::Err(e)),
        }
    }

    /// Returns a fresh `NodeOutput` referencing `results[from]`'s terminal value. The
    /// `&KObject<'a>` is the same reference the producer wrote, not a clone — the arena
    /// lifetime contract must hold across notify-wake and re-run. The execute loop's
    /// Done arm handles frame-aware deep-cloning into the outer arena.
    ///
    /// Invariant: when notify-walk wakes a Lift, `results[from]` is `Some` (Value or Err).
    /// A `None` would mean the wake fired without a terminal write, which is impossible
    /// by construction.
    pub(in crate::runtime::machine::execute) fn run_lift(&self, from: NodeId) -> NodeOutput<'a> {
        match self.store.result_slot(from) {
            NodeOutput::Value(v) => NodeOutput::Value(v),
            NodeOutput::Err(e) => NodeOutput::Err(e.clone_for_propagation()),
        }
    }

    /// `BodyResult::Tail` rewrites the current slot's work in place — this is what gives
    /// recursion constant scheduler memory. `BodyResult::DeferTo(id)` rewrites to a Lift
    /// off `id`, so the slot's terminal becomes whatever `id` produces; matches
    /// `defer_to_lift`'s post-Bind shape but for body-driven combinator planning (MODULE
    /// and SIG body wrap-up via `add_combine`).
    ///
    /// `idx` is the executing slot. Needed so the `DeferTo` arm can install an
    /// `Owned` edge for `id` via `defer_to_lift` (which calls `DepGraph::add_owned_edge`)
    /// before returning the `Replace` — without that install, the Replace gate's
    /// `pending_count(idx)` read sees zero and re-enqueues the Lift before the
    /// producer runs.
    pub(in crate::runtime::machine::execute) fn invoke_to_step(
        &mut self,
        future: KFuture<'a>,
        scope: &'a Scope<'a>,
        idx: usize,
    ) -> NodeStep<'a> {
        match future.function.invoke(scope, self, future.bundle) {
            BodyResult::Value(v) => NodeStep::Done(NodeOutput::Value(v)),
            BodyResult::Tail { expr, frame, function } => NodeStep::Replace {
                work: NodeWork::Dispatch(expr),
                frame,
                function,
            },
            BodyResult::DeferTo(id) => self.defer_to_lift(idx, id),
            BodyResult::Err(e) => NodeStep::Done(NodeOutput::Err(e)),
        }
    }
}
