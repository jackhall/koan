use crate::machine::model::{Carried, KObject};
use crate::machine::{KError, NodeId, TraceFrame};

use super::super::dispatch::{propagate_dep_error, SchedulerView};
use super::super::nodes::{DispatchCombineFinish, LiftState, NodeOutput, NodeStep};
use super::super::{CatchFinish, CombineFinish};
use super::Scheduler;

impl<'run> Scheduler<'run> {
    /// Success-path eager free; the error path leaves deps for chain-free
    /// at slot drop. Inv-B is what makes `dep_edges[idx].clear()` sound
    /// here — see [design/execution-model.md § Dependency graph invariants](../../../../design/execution-model.md#dependency-graph-invariants).
    fn reclaim_deps(&mut self, idx: usize, dep_indices: Vec<usize>) {
        self.deps.clear_dep_edges(idx);
        for d in dep_indices {
            self.free(d);
        }
    }

    /// Only the `deps[park_count..]` owned-sub suffix is eagerly freed on the
    /// success path; the `[..park_count]` park-producer prefix is kept alive
    /// (sibling producers the Combine merely read at finish-time). The error
    /// path leaves edges in `dep_edges[idx]` for chain-free at slot drop.
    pub(super) fn run_combine(
        &mut self,
        deps: Vec<NodeId>,
        park_count: usize,
        finish: CombineFinish<'run>,
        idx: usize,
    ) -> NodeStep<'run> {
        // The finish closure carries its own framing (e.g. "<list>", "<dict>");
        // this generic frame is used only for dep-error propagation.
        let make_frame = || TraceFrame::bare("<combine>", "combine");
        for dep in &deps {
            if let Err(e) = self.read_result(*dep) {
                return NodeStep::Done(NodeOutput::Err(propagate_dep_error(e, Some(make_frame()))));
            }
        }
        // Pre-collect carriers so `finish` (which takes `&mut self`) doesn't reborrow for
        // reads. A type-resolving dep arrives as `Carried::Type`; the finish closure
        // narrows each arm it expects.
        let values: Vec<Carried<'run>> = deps.iter().map(|d| self.read(*d)).collect();
        let owned_indices: Vec<usize> = deps[park_count..].iter().map(|d| d.index()).collect();
        let outcome = finish(&SchedulerView::new(self), &values);
        self.reclaim_deps(idx, owned_indices);
        self.apply_outcome(outcome, idx)
    }

    /// Dispatch-side dual of [`Self::run_combine`]. Same dep short-circuit and owned-dep
    /// reclaim; both finishes return an [`Outcome`] the harness applies, but the dispatch finish
    /// runs through [`super::super::dispatch::run_dispatch_combine_finish`] (which builds the
    /// read-only view), and its deps are reclaimed *before* the finish runs — a dispatch finish
    /// installs its own park/replace edges on `idx`, which a post-finish `clear_dep_edges` would
    /// wrongly wipe. Dep-error propagation is frameless; the resumed dispatch attaches its own
    /// frame.
    pub(super) fn run_dispatch_combine(
        &mut self,
        deps: Vec<NodeId>,
        park_count: usize,
        finish: DispatchCombineFinish<'run>,
        dep_error_frame: Option<TraceFrame>,
        idx: usize,
    ) -> NodeStep<'run> {
        for dep in &deps {
            if let Err(e) = self.read_result(*dep) {
                return NodeStep::Done(NodeOutput::Err(propagate_dep_error(
                    e,
                    dep_error_frame.clone(),
                )));
            }
        }
        let values: Vec<Carried<'run>> = deps.iter().map(|d| self.read(*d)).collect();
        let owned_indices: Vec<usize> = deps[park_count..].iter().map(|d| d.index()).collect();
        self.reclaim_deps(idx, owned_indices);
        super::super::dispatch::run_dispatch_combine_finish(self, finish, &values, idx)
    }

    /// Unlike Combine, an errored `from` does not short-circuit; the finish
    /// closure decides whether to recover or re-raise. `from` is freed on both paths.
    pub(super) fn run_catch(
        &mut self,
        from: NodeId,
        finish: CatchFinish<'run>,
        idx: usize,
    ) -> NodeStep<'run> {
        let result: Result<&'run KObject<'run>, KError> = match self.read_result(from) {
            Ok(v) => Ok(v.object()),
            // Frameless: the recovery-site dispatch attaches its own frame; adding
            // one here would double-frame.
            Err(e) => Err(propagate_dep_error(e, None)),
        };
        let outcome = finish(&SchedulerView::new(self), result);
        self.reclaim_deps(idx, vec![from.index()]);
        self.apply_outcome(outcome, idx)
    }

    /// Consume the stamped Lift state. By pop time the notify-walk has
    /// transitioned `Pending → Ready`; the `Pending` arm is a wake-misfire
    /// panic. See [design/execution-model.md § Lift: push/notify single-producer
    /// model](../../../../design/execution-model.md#lift-pushnotify-single-producer-model).
    pub(super) fn run_lift(state: LiftState<'run>) -> NodeOutput<'run> {
        match state {
            LiftState::Ready(output) => output,
            LiftState::Pending(_) => {
                panic!("scheduler invariant: notify-walk must stamp Lift to Ready before enqueue",)
            }
        }
    }
}
