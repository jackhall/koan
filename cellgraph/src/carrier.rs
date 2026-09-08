//! The two carrier states that carry a lifetime. A value with reach passes through three, in order
//! of liveness: [`Dormant`](crate::Dormant), at rest and lifetime-free, which lives in
//! [`dormant`](crate::dormant); [`Ready`], the in-step form a door hands back; and [`Active`], the
//! in-use form a step reads it out at. See [design/cellgraph.md](../design/cellgraph.md) § The
//! contract: two embedder types.
//!
//! [`Ready`] bundles the value with the mask describing what it reaches; [`Active`] is the value
//! alone at the reading borrow. **A value and its reach are never separable**: every constructor
//! here is crate-private and the mask type is crate-private too, so a caller cannot assemble a
//! loose value-plus-mask pair to hand a mint, and cannot re-pair a value with a mask that is not
//! its own.
//!
//! `'home` is the **home brand**: the cell whose region stores this value is live, and its storage
//! fixed-address, for all of `'home`. Every read rests on it, which is why none takes a proof of
//! liveness. The brand a step's doors hand out is the step's own, so a carrier dies with the step
//! that made it — the substrate's "reachable only inside an `enter` scope" rule, as a lifetime.

use std::marker::PhantomData;

use crate::mask::GraphReach;
use crate::reattach::{DropFree, Erased, Reattachable};

/// Which region a carrier's value was written into: a slab slot, or a tree cell's pool index.
///
/// Crate-private and paired with the value, like the mask beside it. A tree home is not a mask bit
/// — no relation names a tree cell — so the two kinds are a sum here rather than one number.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CellHome {
    Slab(u32),
    Tree(u32),
}

/// The ready carrier: a value erased to its lifetime-free form, bundled with its reach.
///
/// Opaque by construction — it exposes no read of its own. [`StepContext::read`] is the only door
/// out, and it hands back an [`Active`] anchored at the reading borrow.
///
/// [`StepContext::read`]: crate::StepContext::read
pub struct Ready<'home, T: Reattachable + DropFree, const W: usize = 1> {
    value: Erased<T>,
    reach: GraphReach<W>,
    /// The cell whose region stores this value: the one a [`keep`](crate::StepContext::keep)
    /// registers the reach under. For a door-built carrier that is the cell the value was placed
    /// into; for one redeemed out of a sealed cell it is the slab cell whose hold set names the
    /// sealed cell — the executing cell, or its root when the step is running in a tree cell.
    home: CellHome,
    _home: PhantomData<&'home ()>,
}

impl<T: Reattachable + DropFree, const W: usize> Ready<'_, T, W> {
    /// Bundle a value the table itself just wrote into a region with the reach it composed for it.
    /// Crate-private, so the value-to-reach pairing is only ever the one a door established.
    pub(crate) fn new(value: Erased<T>, reach: GraphReach<W>, home: CellHome) -> Self {
        Ready {
            value,
            reach,
            home,
            _home: PhantomData,
        }
    }

    /// The value's reach — which cells' region storage its borrows read. Crate-private, like the
    /// mask itself: the only reach a value travels with is the one a door composed for it.
    pub(crate) fn reach(&self) -> &GraphReach<W> {
        &self.reach
    }

    /// Copy the erased form out without consuming the carrier — the read door's first half.
    ///
    /// The bound is on the **erased** form, not on `T::At<'static>`. A `T::At<'static>: Copy`
    /// bound in scope makes rustc prove `Copy` for a re-anchored `T::At<'r>` by unifying `'r` with
    /// `'static`, since a projection carries no variance; bounding `Erased<T>` keeps the live form
    /// move-only and the re-anchor's lifetime free.
    pub(crate) fn erased(&self) -> Erased<T>
    where
        Erased<T>: Copy,
    {
        self.value
    }

    /// The cell whose region stores this value — what the ancestry rule classifies by, and what a
    /// [`keep`](crate::StepContext::keep) registers under.
    pub(crate) fn home(&self) -> CellHome {
        self.home
    }

    /// Split the carrier into the three things a [`keep`](crate::StepContext::keep) needs: the
    /// erased value, the reach the table takes over, and the cell whose table takes it.
    pub(crate) fn into_parts(self) -> (Erased<T>, GraphReach<W>, CellHome) {
        (self.value, self.reach, self.home)
    }
}

/// Duplicating a carrier duplicates no ownership: the value names region bytes it does not own,
/// and the reach is a word copy. Two holders of the same value name the same reach, which is what
/// keeps the mint idempotent.
impl<T: Reattachable + DropFree, const W: usize> Clone for Ready<'_, T, W>
where
    Erased<T>: Copy,
{
    fn clone(&self) -> Self {
        Ready {
            value: self.value,
            reach: self.reach.clone(),
            home: self.home,
            _home: PhantomData,
        }
    }
}

/// The in-use carrier: the value re-anchored at the reading borrow `'r`. The borrow checker keeps
/// it inside `'r`, so it cannot outlive the step that read it.
///
/// The reach stays behind in the table: it is the substrate's bookkeeping, and the borrow the
/// reader gets is already bounded by the cell's life. Bounded only by [`Reattachable`], since a
/// continuation comes back through this state too and rests in its cell's slot rather than a
/// region, where drop glue is fine.
pub struct Active<'r, T: Reattachable> {
    value: T::At<'r>,
}

impl<'r, T: Reattachable> Active<'r, T> {
    pub(crate) fn new(value: T::At<'r>) -> Self {
        Active { value }
    }

    /// The re-anchored value.
    pub fn value(&self) -> T::At<'r>
    where
        T::At<'r>: Copy,
    {
        self.value
    }

    /// The re-anchored value, consuming the open — the by-move twin of [`value`](Active::value)
    /// for a family whose live form is not `Copy`.
    pub fn into_value(self) -> T::At<'r> {
        self.value
    }
}
