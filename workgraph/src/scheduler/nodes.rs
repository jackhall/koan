//! The generic per-node work the scheduler stores: a node's [`NodeWork`] — its deps and the
//! one-shot continuation that runs over them. Parametric over the [`Workload`]; the scheduler stores
//! it and hands it back but inspects no field beyond the dep wiring.
//!
//! `NodeWork` is the **live** pre-install currency: an embedder builds one at its own lifetime and
//! hands it to an install path, which seals it into the scheduler-internal [`StoredWork`] against the
//! slot's anchor. The continuation therefore never rests without the pin covering it, and no
//! embedder call site can mispair the two.

use std::rc::Rc;

use super::{Reattachable, ResolvedDeps, Workload};
use crate::witnessed::SealedPinned;

/// What a scheduler node will run: wait on `deps`, then run `continuation` over their resolved
/// terminals. `deps` is a [`ResolvedDeps`] — a `[park_producers..., owned_subs...]` layout the
/// scheduler owns (assembled only through the [`Deps`](super::Deps) builder): parks install `Notify`
/// edges (kept alive), owned deps install `Owned` (cascade-freed at success). `carrier` is the
/// deadlock-report sample (a workload-supplied expression summary, else `None`). The continuation is
/// held opaquely (`W::Continuation`) and handed back to run once; the node itself never branches and
/// names no workload type.
pub struct NodeWork<'a, W: Workload> {
    pub deps: ResolvedDeps,
    /// The slot's continuation, live at the construction site's lifetime. The scheduler seals it
    /// with the slot's anchor pin at install ([`SealedPinned::erase`]), so a droppable continuation
    /// never rests without its glue and its pin.
    pub continuation: <W::Continuation as Reattachable>::At<'a>,
    pub carrier: Option<String>,
}

impl<'a, W: Workload> NodeWork<'a, W> {
    /// Build node work from a **live** continuation. It stays live here; the install path seals it
    /// against the slot's anchor.
    pub fn new(
        deps: ResolvedDeps,
        continuation: <W::Continuation as Reattachable>::At<'a>,
        carrier: Option<String>,
    ) -> Self {
        NodeWork {
            deps,
            continuation,
            carrier,
        }
    }
}

/// A node's work in its resting form: the same fields, with the continuation sealed on the owned
/// tier against the slot's anchor `Rc`. The anchor transitively holds the storage chain the
/// continuation reads, so the seal's bundled pin is the liveness the step open is bounded by —
/// carried by the seal itself rather than supplied externally at each open.
///
/// The fields are scheduler-internal: an embedder receives one from
/// [`take_for_run`](super::Scheduler::take_for_run) and decomposes it only through
/// [`into_run_parts`](Self::into_run_parts), so the sealed continuation reaches a step open without
/// ever being re-paired with a pin by hand.
pub struct StoredWork<W: Workload> {
    pub(in crate::scheduler) deps: ResolvedDeps,
    pub(in crate::scheduler) continuation: SealedPinned<W::Continuation, Rc<W::Frame>>,
    pub(in crate::scheduler) carrier: Option<String>,
}

impl<W: Workload> StoredWork<W> {
    /// Decompose a popped node's work by value for the run loop: the resolved dep list (read in
    /// delivery order), the sealed continuation, and the deadlock-summary carrier.
    // The (deps, continuation, carrier) triple reads clearer inline than split into a named alias.
    #[allow(clippy::type_complexity)]
    pub fn into_run_parts(
        self,
    ) -> (
        ResolvedDeps,
        SealedPinned<W::Continuation, Rc<W::Frame>>,
        Option<String>,
    ) {
        (self.deps, self.continuation, self.carrier)
    }
}

/// The single erase door: every install path routes through here with the slot's **effective**
/// anchor, so the continuation's seal and the node's pin are minted in one act.
pub(in crate::scheduler) fn seal_work<W: Workload>(
    work: NodeWork<'_, W>,
    anchor: &Rc<W::Frame>,
) -> StoredWork<W> {
    StoredWork {
        deps: work.deps,
        continuation: SealedPinned::erase(work.continuation, Rc::clone(anchor)),
        carrier: work.carrier,
    }
}
