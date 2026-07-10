//! The generic per-node work the scheduler stores: a node's [`NodeWork`] — its deps and the
//! one-shot continuation that runs over them. Parametric over the [`Workload`]; the scheduler stores
//! it and hands it back but inspects no field beyond the dep wiring.

use super::{Erased, Reattachable, ResolvedDeps, Workload};

/// What a scheduler node will run: wait on `deps`, then run `cont` over their resolved terminals.
/// `deps` is a [`ResolvedDeps`] — a `[park_producers..., owned_subs...]` layout the scheduler owns
/// (assembled only through the [`Deps`](super::Deps) builder): parks install `Notify` edges (kept
/// alive), owned deps install `Owned` (cascade-freed at success). `carrier` is the deadlock-report
/// sample (a workload-supplied expression summary, else `None`). The continuation is stored opaquely
/// (`W::Continuation`) and handed back to run once; the node itself never branches and names no
/// workload type.
pub struct NodeWork<W: Workload> {
    pub deps: ResolvedDeps,
    /// The slot's continuation, stored erased to `'static` (`Erased<W::Continuation>`) so the node it
    /// sits on pins no borrow. Handed back to run once; never inspected — the workload re-anchors it
    /// once per step via the consuming externally-witnessed
    /// [`SealedExtern::open`](crate::witnessed::SealedExtern::open), against the held anchor `Rc`.
    pub continuation: Erased<W::Continuation>,
    pub carrier: Option<String>,
}

impl<W: Workload> NodeWork<W> {
    /// Build node work from a **live** continuation, erasing it to `'static` for storage here — the
    /// scheduler owns the erase (peer of `finalize`'s value erase). The continuation is handed back
    /// re-anchored to run once; the scheduler never inspects it.
    pub fn new(
        deps: ResolvedDeps,
        continuation: <W::Continuation as Reattachable>::At<'_>,
        carrier: Option<String>,
    ) -> Self {
        NodeWork {
            deps,
            continuation: Erased::erase(continuation),
            carrier,
        }
    }

    /// Decompose a popped node's work by value for the run loop: the resolved dep list (read in
    /// delivery order), the erased continuation, and the deadlock-summary carrier.
    pub fn into_run_parts(self) -> (ResolvedDeps, Erased<W::Continuation>, Option<String>) {
        (self.deps, self.continuation, self.carrier)
    }
}
