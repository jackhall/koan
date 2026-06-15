//! [`NodeId`] — the stable handle to a node in the scheduler's DAG, used crate-wide. The
//! scheduler's write primitives are inherent methods on `machine::execute::Scheduler`; the
//! `AwaitDeps`/`Catch` finish aliases live in `machine::execute::outcome` (they return the
//! execute-private `Outcome`).

/// Stable handle to a node in the scheduler's DAG.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub usize);

impl NodeId {
    pub fn index(self) -> usize {
        self.0
    }
}
