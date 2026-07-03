//! [`NodeId`] — the stable handle to a node in the scheduler's DAG.

/// Stable handle to a node in the scheduler's DAG. Minted only by the node store
/// (`alloc_slot`) and used to name a slot for the lifetime of a run.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub usize);

impl NodeId {
    pub fn index(self) -> usize {
        self.0
    }
}
