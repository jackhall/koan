//! How a run cell finds the arena node it runs under: what its asker hands the drain, and what the
//! drain does with it before the cell is born.
//!
//! An asker that calls a body hands a [`Seek`]: the arena node the callee was born under, and, for
//! a body keyed on its bindings whose content the asker read, the key and the image of the types
//! its binding carries. The drain looks the key up in the [arena](super::arena) under that parent:
//! a live hit is stamped, and a miss builds the settlement, relays the image's nodes into the
//! agent tier, and evicts to the budget once the frame borrows it. So a frame is born under its
//! settlement and parks for nothing. A frame with no key runs under the parent itself. A tail hop
//! of the same callee under a live settlement stamps it too. A parent that is no longer live
//! births the frame cold, under the root. Any other cell runs under what its asker runs under. See
//! [build/README.md § Steps and the drain](../build/README.md#steps-and-the-drain).

use crate::values::{ArenaRef, BindingImage, SettlementKey};

/// What an asker hands the drain to find a frame's arena node: the one the callee was born under,
/// and the settlement under it the frame seeks, if any.
pub struct Seek<'graph> {
    pub parent: ArenaRef,
    /// A keyed body's key under `parent` and its binding's image: `None` for a body that keys no
    /// settlement or a call whose content at a keyed name the asker could not read, whose frame
    /// runs under `parent`.
    pub settlement: Option<(SettlementKey, BindingImage<'graph>)>,
}

/// What a request's cell runs under, resolved by the drain at its creation.
pub(super) enum Lending<'graph> {
    /// What its asker runs under, as an evaluation, a block or a hand-off does.
    Parent,
    /// A call's frame: the arena node its [`Seek`] finds or builds.
    Call(Seek<'graph>),
    /// The settlement an `ArenaRef` names, as a tail hop of the same callee runs under its
    /// frame's, stamped; the root, cold, where that one is no longer live.
    Under(ArenaRef),
}
