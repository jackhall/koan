//! [`NodeId`] — the stable handle to a node in the scheduler's DAG, used crate-wide. The
//! `SchedulerHandle` trait and its `Combine`/`Catch` finish aliases live in
//! `machine::execute::scheduler_handle` (their finish aliases return the execute-private `Outcome`).

/// Stable handle to a node in the scheduler's DAG.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub usize);

impl NodeId {
    pub fn index(self) -> usize {
        self.0
    }
}
