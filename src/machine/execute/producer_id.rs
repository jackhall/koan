//! `ProducerId` — identity of the act that will produce a value koan is waiting on.
//!
//! A still-finalizing binder occupies its destination name until it delivers, and a consumer
//! that reads the name meanwhile has to name *what it is waiting for* — not a value, which
//! does not exist yet, but the producer of one. So name resolution's miss answer is a set of
//! producers, and a slot that misses parks on them.
//!
//! Unlike declaration identity, koan cannot mint this one: the producer is a submission the
//! scheduler is already running, so the only faithful name for it is the scheduler's. Both
//! conversions are `pub(in crate::machine::execute)`, so a layer that stores a producer,
//! compares two for equality, and passes one along still *cannot* open one — the field is
//! unreachable from there, so a leak is a compile error rather than a review catch. The type
//! lives under `execute` because that is the layer owning the scheduler, and so the only
//! ancestor visibility naming the drive loop and nothing below it.
//!
//! A layer that cannot open a producer still needs to wait on one, which is what [`deps_on`]
//! and [`extend_deps_on`] are for: the only crossing that turns producers into scheduler
//! currency, so "where does an edge escape?" has one answer everywhere outside this layer.

use crate::scheduler::{Deps, EdgeId};

/// Who will produce the value a name does not hold yet: the still-finalizing binder occupying
/// that name, identified by the edge its submission installed. Opaque — koan stores it,
/// compares it, and hands it back.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProducerId(EdgeId);

impl ProducerId {
    pub(in crate::machine::execute) fn from_scheduler_edge(edge: EdgeId) -> ProducerId {
        ProducerId(edge)
    }

    pub(in crate::machine::execute) fn scheduler_edge(self) -> EdgeId {
        self.0
    }

    /// Fabricate a producer for a white-box test that drives no scheduler — a binding table
    /// asserted on directly. Mirrors `EdgeId::for_test`.
    #[cfg(test)]
    pub(crate) fn for_test(index: usize) -> ProducerId {
        ProducerId(EdgeId::for_test(index))
    }
}

/// Depend on `producers` — with [`extend_deps_on`], the only crossing that spends a producer
/// from outside this layer.
pub(crate) fn deps_on<T>(producers: impl IntoIterator<Item = ProducerId>) -> Deps<T> {
    let mut deps = Deps::new();
    extend_deps_on(&mut deps, producers);
    deps
}

/// Append `producers` to a list already under construction — the same crossing as [`deps_on`],
/// for a dep list that interleaves producers with sub-work requests.
pub(crate) fn extend_deps_on<T>(
    deps: &mut Deps<T>,
    producers: impl IntoIterator<Item = ProducerId>,
) {
    for producer in producers {
        deps.on(producer.scheduler_edge());
    }
}
