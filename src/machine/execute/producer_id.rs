//! `ProducerId` — identity of the act that will produce a value koan is waiting on.
//!
//! A still-finalizing binder occupies its destination name until it delivers, and a consumer
//! that reads the name meanwhile has to name *what it is waiting for* — not a value, which
//! does not exist yet, but the producer of one. So name resolution's miss answer is a set of
//! producers, and a slot that misses parks on them.
//!
//! Unlike declaration identity, koan cannot mint this one: the producer is a submission the
//! scheduler is already running, so the only faithful name for it is the scheduler's. What
//! koan can do is name it *as a producer*, and confine who may read the scheduler name back
//! out. A `ProducerId` has no verbs, and both conversions are private to this layer — so the
//! binding tables, the scope registry, the type resolver, and the builtins can store one,
//! compare two for equality, and pass one along, and *cannot* open one. Not by convention: the
//! field is unreachable from there, so a leak is a compile error rather than a review catch.
//!
//! It lives under `execute` because `execute` is the layer that owns the scheduler. That is
//! what makes the confinement expressible at all — `pub(in crate::machine::execute)` names an
//! ancestor of the drive loop and of nothing below it.
//!
//! A layer that cannot open a producer still needs to wait on one, which is what [`deps_on`]
//! is for: the single verb that turns producers into scheduler currency, so "where does an
//! edge escape?" has one answer everywhere outside this layer.

use crate::scheduler::{Deps, EdgeId};

/// Who will produce the value a name does not hold yet: the still-finalizing binder occupying
/// that name, identified by the edge its submission installed. Opaque — koan stores it,
/// compares it, and hands it back.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProducerId(EdgeId);

impl ProducerId {
    /// Name the submission that just installed `edge` as the producer for a binder's
    /// destination.
    pub(in crate::machine::execute) fn from_scheduler_edge(edge: EdgeId) -> ProducerId {
        ProducerId(edge)
    }

    /// The scheduler name behind a producer. Confined to the drive loop, which is the only
    /// layer holding a scheduler to spend it on.
    pub(in crate::machine::execute) fn scheduler_edge(self) -> EdgeId {
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

/// Depend on `producers`: the one act that spends a producer, and the only way to spend one
/// from outside this layer. A builtin declaring an `AwaitDeps` builds its dep vector through
/// here and then appends any sub-dispatch requests, so the list stays in the order its finish
/// reads results back in.
pub(crate) fn deps_on<T>(producers: impl IntoIterator<Item = ProducerId>) -> Deps<T> {
    let mut deps = Deps::new();
    extend_deps_on(&mut deps, producers);
    deps
}

/// Append `producers` to a list already under construction — the same crossing as [`deps_on`], for a
/// builder that interleaves producers it holds with sub-work it spawns and needs each entry's index
/// as it goes.
pub(crate) fn extend_deps_on<T>(
    deps: &mut Deps<T>,
    producers: impl IntoIterator<Item = ProducerId>,
) {
    for producer in producers {
        deps.on(producer.scheduler_edge());
    }
}
