//! How a frame finds the specialization it borrows: what its asker hands the drain, and what the
//! drain does with it before the frame is born.
//!
//! An asker that calls a body keyed on its bindings hands a [`Seek`]: the build cell the callee was
//! born under, the key, the types its binding carries, and the specialization's own work. The drain
//! reads that parent's table through a peek between steps; a live hit is the specialization, its
//! entry stamped as a birth, and a miss creates one under the parent, lays the binding's nodes in its
//! branch from the asker's region, and runs its one step at once, so the frame is born borrowing it
//! and parks for nothing. A miss on a full table first releases the parent's least recently born
//! child with its subtree, and the new child takes its entry. A tail hop of the same callee under a
//! live specialization stamps its entry too.
//! A parent that is no longer live births the frame cold: a `Fresh` frame borrowing its parent's
//! scratch as any other tree cell does, a `Shares` frame borrowing nothing. Either placement borrows
//! what it finds, a `Fresh` frame as its tree cell's lend and a `Shares` frame as its tenant's own.
//! See [build/README.md § Steps and the drain](../build/README.md#steps-and-the-drain).

use crate::scheduler::continuation::{StepBundle, Work};
use crate::values::{BuildRef, SpecBinding, SpecKey};

/// What an asker hands the drain to find a frame's specialization: the build cell the callee was
/// born under, the key in its table, the types its binding carries, and the work a missed
/// specialization is born to run, at `'graph` since it reports to nobody.
pub struct Seek<'graph, B: StepBundle<'graph>> {
    pub parent: BuildRef,
    pub key: SpecKey,
    pub binding: SpecBinding,
    pub work: Work<'graph, 'graph, B>,
}

/// What a request's cell borrows, resolved by the drain at its creation.
pub(super) enum Lending<'graph, B: StepBundle<'graph>> {
    /// What its asker reads, as an evaluation, a block or a hand-off does: a tree cell borrows its
    /// parent's scratch, and a tenant copies its parent's lend.
    Parent,
    /// A call's frame: the specialization a [`Seek`] finds or builds, or, with no `Seek` or with
    /// none found, what a cold frame borrows: its parent's scratch for a tree cell, nothing for a
    /// tenant.
    Call(Option<Seek<'graph, B>>),
    /// The specialization a `BuildRef` names, as a tail hop of the same callee borrows its
    /// frame's; a cold frame's lend where that one is no longer live.
    Under(BuildRef),
}
