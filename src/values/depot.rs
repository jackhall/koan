//! What the build depot's books are bounded by, and how code above the scheduler names a depot
//! slot.
//!
//! A [`BuildBudget`] bounds the bytes of the depot slots no run cell holds. A [`DepotRef`] is how
//! code above the scheduler holds what a frame runs under without naming a slot: only `values` and
//! `scheduler` open one. See [the build depot](../scheduler/README.md#the-build-depot).

use std::num::NonZeroUsize;

use crate::memory::TreeHandle;

/// How many bytes the build depot's unheld depot slots may hold: past it, a miss evicts the least
/// recently used unheld leaves.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BuildBudget {
    pub bytes: NonZeroUsize,
}

impl Default for BuildBudget {
    fn default() -> Self {
        BuildBudget {
            bytes: NonZeroUsize::new(4 << 20).expect("4 MiB is not zero"),
        }
    }
}

/// What a function was born under and a frame runs under: the root, the frozen load; or a depot
/// slot, by the slot and the slot's generation then. Opaque above the scheduler, which alone opens
/// one, so a step can hand the drain only what the scheduler named to it. A handle whose slot has
/// since been evicted names nothing.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct DepotRef(Placed);

/// What a [`DepotRef`] names: see its documentation.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Placed {
    Root,
    Slot { slot: u32, generation: u32 },
}

impl DepotRef {
    /// The root: the frozen load.
    pub const ROOT: DepotRef = DepotRef(Placed::Root);

    /// The composition at `slot` while the slot is at `generation`.
    pub(crate) fn of(slot: u32, generation: u32) -> Self {
        DepotRef(Placed::Slot { slot, generation })
    }

    /// The slot and generation this names, or `None` for the root.
    pub(crate) fn opened(self) -> Option<(u32, u32)> {
        match self.0 {
            Placed::Slot { slot, generation } => Some((slot, generation)),
            Placed::Root => None,
        }
    }
}

/// A tree cell a fault keeps, beside the depot slot it borrows: opaque and brand-free, so a fault's
/// link may name it from any region. Only the scheduler makes or opens one, as it does an
/// [`DepotRef`] ([scheduler/README.md](../scheduler/README.md#faults)).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct KeptCell {
    cell: TreeHandle,
    depot: DepotRef,
}

impl KeptCell {
    /// The kept tree cell `cell`, which borrows `depot`.
    pub(crate) fn of(cell: TreeHandle, depot: DepotRef) -> Self {
        KeptCell { cell, depot }
    }

    /// The kept tree cell and the depot slot it borrows.
    pub(crate) fn opened(self) -> (TreeHandle, DepotRef) {
        (self.cell, self.depot)
    }
}
