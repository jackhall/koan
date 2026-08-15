//! The generic per-node work the scheduler stores: a node's [`NodeWork`] — the one-shot
//! continuation that runs over the slot's delivered deps, plus its deadlock-report carrier.
//! Parametric over the [`Workload`]; the scheduler stores it and hands it back but inspects no
//! field. The slot's realized dep list is not part of the work: the install doors write it onto the
//! slot's own dep row, and [`Scheduler::drain`](super::Scheduler::drain) reads the residents off it
//! at step start.
//!
//! `NodeWork` is the **live** pre-install currency: an embedder builds one at its own lifetime and
//! hands it to an install path, which seals it into the scheduler-internal [`StoredWork`] against the
//! slot's anchor. The continuation therefore never rests without the pin covering it, and no
//! embedder call site can mispair the two.

use std::rc::Rc;

use super::{Reattachable, Workload};
use crate::witnessed::SealedPinned;

/// What a scheduler node will run: the one-shot continuation, invoked over the delivered residents
/// of the dep edges an install door wired onto the slot. `carrier` is the deadlock-report sample (a
/// workload-supplied expression summary, else `None`). The continuation is held opaquely
/// (`W::Continuation`) and handed back to run once; the node itself never branches and names no
/// workload type. Fresh work is always dep-free — a slot's deps exist only as the edges the install
/// doors mint, never as a field the embedder fills.
pub struct NodeWork<'a, W: Workload> {
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
        continuation: <W::Continuation as Reattachable>::At<'a>,
        carrier: Option<String>,
    ) -> Self {
        NodeWork {
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
/// Scheduler-internal: [`Scheduler::drain`](super::Scheduler::drain) decomposes it and hands the
/// step callback the sealed continuation directly, so it reaches a step open without ever being
/// re-paired with a pin by hand.
pub(in crate::scheduler) struct StoredWork<W: Workload> {
    pub(in crate::scheduler) continuation: SealedPinned<W::Continuation, Rc<W::Frame>>,
    pub(in crate::scheduler) carrier: Option<String>,
}

/// The single erase door: every install path routes through here with the slot's **effective**
/// anchor, so the continuation's seal and the node's pin are minted in one act.
pub(in crate::scheduler) fn seal_work<W: Workload>(
    work: NodeWork<'_, W>,
    anchor: &Rc<W::Frame>,
) -> StoredWork<W> {
    StoredWork {
        continuation: SealedPinned::erase(work.continuation, Rc::clone(anchor)),
        carrier: work.carrier,
    }
}
