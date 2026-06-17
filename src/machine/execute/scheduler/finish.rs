use crate::machine::model::Carried;
use crate::machine::{KError, NodeId};

use super::super::dispatch::{current_scope, SchedulerView};
use super::super::lift::NodeLift;
use super::super::nodes::NodeStep;
use super::super::outcome::deps_at_step;
use super::super::runtime::KoanRuntime;
use super::super::NodeCont;
use super::{Scheduler, Workload};

impl<W: Workload> Scheduler<W> {
    /// Success-path eager free; the error path leaves deps for chain-free
    /// at slot drop. Inv-B is what makes `dep_edges[idx].clear()` sound
    /// here — see [design/execution-model.md § Dependency graph invariants](../../../../design/execution-model.md#dependency-graph-invariants).
    pub(in crate::machine::execute::scheduler) fn reclaim_deps(
        &mut self,
        idx: usize,
        dep_indices: Vec<usize>,
    ) {
        self.deps.clear_dep_edges(idx);
        for d in dep_indices {
            self.free(d);
        }
    }
}

impl<'run> KoanRuntime<'run> {
    /// The unified node handler: collect the resolved dep terminals (as owned `Result`s — an
    /// errored dep is handed through, the continuation decides), run `cont` against a read-only
    /// [`SchedulerView`], reclaim the owned-dep suffix, then apply. The continuation issues no
    /// graph write, so the reclaim lands after it and before the apply that installs the
    /// continuation's edges. Carried values survive the reclaim (they live in arenas, not slots).
    pub(in crate::machine::execute::scheduler) fn run_wait(
        &mut self,
        deps: Vec<NodeId>,
        park_count: usize,
        cont: NodeCont<'run>,
        idx: usize,
    ) -> NodeStep<'run> {
        // Consumer-pull: lift each dep's terminal out of its producer frame into this consumer's
        // arena, so the value dies with the consumer and the producer keeps no surviving copy that
        // would outlive its own dying frame. A frameless / run-arena terminal already survives and
        // is forwarded as-is.
        let dest = current_scope(&self.sched).arena;
        let results: Vec<Result<Carried<'run>, KError>> = deps
            .iter()
            .map(|d| match self.sched.read_result_with_frame(*d) {
                // SAFETY: the slot's co-stored frame Rc / run arena pins the value; read is transient.
                Ok((value, Some(frame))) => {
                    Ok(self.lift(unsafe { value.reattach() }, &frame, dest))
                }
                // SAFETY: the slot's co-stored frame Rc / run arena pins the value; read is transient.
                Ok((value, None)) => Ok(unsafe { value.reattach() }),
                Err(e) => Err(e.clone()),
            })
            .collect();
        let owned_indices: Vec<usize> = deps[park_count..].iter().map(|d| d.index()).collect();
        // The pull-lifted values die with this consumer's frame; deliver them at that `'s`.
        let outcome = cont(
            &SchedulerView::new(&self.sched),
            deps_at_step(&results),
            idx,
        );
        self.sched.reclaim_deps(idx, owned_indices);
        self.apply_outcome(outcome, idx)
    }
}
