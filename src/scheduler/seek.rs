//! How a frame finds the specialization it borrows: what its asker hands the drain, and what the
//! drain does with it before the frame is born.
//!
//! An asker that calls a body keyed on its bindings hands a [`Seek`]: the build cell the callee was
//! born under, the key, and the specialization's own work. The drain reads that parent's table
//! through a peek between steps; a live hit is the specialization, and a miss creates one under the
//! parent and runs its one step at once, so the frame is born borrowing it and parks for nothing.
//! A parent that is no longer live births the frame cold, borrowing its parent's scratch as any
//! other frame does. See [build/README.md § Steps and the drain](../build/README.md#steps-and-the-drain).

use crate::scheduler::continuation::{StepBundle, Work};
use crate::values::{BuildRef, SpecKey};

/// What an asker hands the drain to find a frame's specialization: the build cell the callee was
/// born under, the key in its table, and the work a missed specialization is born to run, at
/// `'graph` since it reports to nobody.
pub struct Seek<'graph, B: StepBundle<'graph>> {
    pub parent: BuildRef,
    pub key: SpecKey,
    pub work: Work<'graph, 'graph, B>,
}

/// What a request's cell borrows, resolved by the drain at its creation. Read only for a `Fresh`
/// placement: a tenant borrows through its host.
pub(super) enum Lending<'graph, B: StepBundle<'graph>> {
    /// Its tree parent's scratch, as every frame born under no binding does.
    Parent,
    /// The specialization a [`Seek`] finds or builds.
    Seek(Seek<'graph, B>),
    /// The specialization a `BuildRef` names, as a tail hop of the same callee borrows its
    /// frame's; its parent's scratch where that one is no longer live.
    Under(BuildRef),
}
