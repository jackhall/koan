//! `ProducerId` — identity of the act that will produce a value koan is waiting on.
//!
//! A still-finalizing binder occupies its destination name until it delivers, and a consumer
//! that reads the name meanwhile has to name *what it is waiting for* — not a value, which
//! does not exist yet, but the producer of one. So name resolution's miss answer is a set of
//! producers, and a slot that misses parks on them.
//!
//! Unlike declaration identity, koan cannot mint this one: the producer is a submission the
//! scheduler is already running, so the only faithful name for it is the scheduler's. What
//! koan can do is name it *as a producer*. A `ProducerId` has no verbs — no wiring, no
//! delivery, no slab access. The binding tables, the scope registry, and the type resolver
//! store one, compare two for equality, and pass one along; none of them can ask the
//! scheduler anything with it, because there is nothing on this type to ask with. It converts
//! back to a scheduler name only at the one event that consumes a producer:
//! [`scheduler_edge`](ProducerId::scheduler_edge), entering it into a `Deps` as a park.
//!
//! That is what keeps the dependency one-directional. koan spells `crate::scheduler::EdgeId`
//! in this file and at `Deps` boundaries, and nowhere in between — so workgraph stays free to
//! change what an edge is, and koan's own layers speak a koan concept.

use crate::scheduler::EdgeId;

/// Who will produce the value a name does not hold yet: the still-finalizing binder occupying
/// that name, identified by the edge its submission installed. Opaque — koan stores it,
/// compares it, and hands it back.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProducerId(EdgeId);

impl ProducerId {
    /// Name the submission that just installed `edge` as the producer for a binder's
    /// destination.
    pub(crate) fn from_scheduler_edge(edge: EdgeId) -> ProducerId {
        ProducerId(edge)
    }

    /// The scheduler name, for the one act that consumes a producer: entering it into a `Deps`
    /// as a park, so the parking slot blocks until this producer delivers.
    pub(crate) fn scheduler_edge(self) -> EdgeId {
        self.0
    }

    /// Fabricate a producer for a white-box test that drives no scheduler — a binding table
    /// asserted on directly. Mirrors `EdgeId::for_test`, and gated so it cannot reach
    /// production code.
    #[cfg(test)]
    pub(crate) fn for_test(index: usize) -> ProducerId {
        ProducerId(EdgeId::for_test(index))
    }
}
