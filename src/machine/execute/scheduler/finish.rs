use crate::machine::model::Carried;
use crate::machine::{KError, NodeId};

use super::super::dispatch::SchedulerView;
use super::super::nodes::{LiftState, NodeOutput, NodeStep};
use super::super::NodeCont;
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

    /// The unified node handler: collect the resolved dep terminals (as owned `Result`s — an
    /// errored dep is handed through, the continuation decides), run `cont` against a read-only
    /// [`SchedulerView`], reclaim the owned-dep suffix, then apply. The continuation issues no
    /// graph write, so the reclaim lands after it and before the apply that installs the
    /// continuation's edges. Carried values survive the reclaim (they live in arenas, not slots).
    pub(super) fn run_wait(
        &mut self,
        deps: Vec<NodeId>,
        park_count: usize,
        cont: NodeCont<'run>,
        idx: usize,
    ) -> NodeStep<'run> {
        let results: Vec<Result<Carried<'run>, KError>> = deps
            .iter()
            .map(|d| self.read_result(*d).map_err(|e| e.clone()))
            .collect();
        let owned_indices: Vec<usize> = deps[park_count..].iter().map(|d| d.index()).collect();
        let outcome = cont(&SchedulerView::new(self), &results, idx);
        self.reclaim_deps(idx, owned_indices);
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
