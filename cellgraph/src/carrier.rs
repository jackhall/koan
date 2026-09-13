//! The two carrier states that carry a lifetime beside `'graph`. A value with reach passes through
//! three, in order of liveness: [`Dormant`](crate::Dormant), at rest and free of every step brand,
//! which lives in [`dormant`](crate::dormant); [`Ready`], the in-step form a door hands back; and
//! [`Active`], the in-use form a step reads it out at. See [../README.md](../README.md) § The
//! contract: two embedder types.
//!
//! [`Ready`] bundles the value with the mask describing what it reaches; [`Active`] is the value
//! alone, at the lifetime it is used at. **A value and its reach are never separable**: `Ready`'s
//! constructor is crate-private and the mask type is crate-private too, so a caller cannot assemble
//! a loose value-plus-mask pair to hand a mint, and cannot re-pair a value with a mask that is not
//! its own. `Active` holds no reach, so its constructor is public: there is nothing in it to forge.
//!
//! Every state carries `'graph`, the storage outliving the graph a family's form may borrow through,
//! and no door retypes it.
//!
//! `'home` is the **home brand**: the cell whose region stores this value is live, and its storage
//! fixed-address, for all of `'home`. Every read rests on it, which is why none takes a proof of
//! liveness. The brand a step's doors hand out is the step's own, so a carrier dies with the step
//! that made it — the substrate's "reachable only inside an `enter` scope" rule, as a lifetime.

use std::marker::PhantomData;

use crate::reach::GraphReach;
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

/// The ready carrier: a value erased to its form at `'graph`, bundled with its reach.
///
/// Opaque by construction — it exposes no read of its own. [`StepContext::read`] is the only door
/// out, and it hands back an [`Active`] anchored at the reading borrow.
///
/// [`StepContext::read`]: crate::StepContext::read
pub struct Ready<'graph, 'home, T: Reattachable<'graph> + DropFree, const W: usize = 1> {
    value: Erased<'graph, T>,
    reach: GraphReach<W>,
    /// The cell whose region stores this value: the one a [`keep`](crate::StepContext::keep)
    /// registers the reach under. For a door-built carrier that is the cell the value was placed
    /// into; for one redeemed out of a sealed cell it is the slab cell whose hold set names the
    /// sealed cell — the executing cell, or its root when the step is running in a tree cell.
    home: CellHome,
    _home: PhantomData<&'home ()>,
}

impl<'graph, T: Reattachable<'graph> + DropFree, const W: usize> Ready<'graph, '_, T, W> {
    /// Bundle a value the graph itself just wrote into a region with the reach it composed for it.
    /// Crate-private, so the value-to-reach pairing is only ever the one a door established.
    pub(crate) fn new(value: Erased<'graph, T>, reach: GraphReach<W>, home: CellHome) -> Self {
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
    /// The bound is on the **erased** form, not on `T::At<'graph>`. A `T::At<'graph>: Copy` bound
    /// in scope makes rustc prove `Copy` for a re-anchored `T::At<'cell>` by unifying `'cell` with
    /// `'graph`, since a projection carries no variance; bounding `Erased<'graph, T>` keeps the live
    /// form move-only and the re-anchor's lifetime free.
    pub(crate) fn erased(&self) -> Erased<'graph, T>
    where
        Erased<'graph, T>: Copy,
    {
        self.value
    }

    /// The cell whose region stores this value — what the ancestry rule classifies by, and what a
    /// [`keep`](crate::StepContext::keep) registers under.
    pub(crate) fn home(&self) -> CellHome {
        self.home
    }

    /// Split the carrier into the three things a [`keep`](crate::StepContext::keep) needs: the
    /// erased value, the reach the graph takes over, and the cell whose reach table takes it.
    pub(crate) fn into_parts(self) -> (Erased<'graph, T>, GraphReach<W>, CellHome) {
        (self.value, self.reach, self.home)
    }
}

/// Duplicating a carrier duplicates no ownership: the value names region bytes it does not own,
/// and the reach is a word copy. Two holders of the same value name the same reach, which is what
/// keeps the mint idempotent.
impl<'graph, T: Reattachable<'graph> + DropFree, const W: usize> Clone for Ready<'graph, '_, T, W>
where
    Erased<'graph, T>: Copy,
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

/// The value alone, at the lifetime it is used at. It reaches this state two ways: a
/// [`read`](crate::StepContext::read) re-anchors a carrier's value at the reading borrow, which the
/// borrow checker keeps inside the step; and a placement's build closure hands the value it built
/// back in one, at the destination region's brand.
///
/// The reach stays behind — in the reach table for a read, and in the placement that composes it for
/// a build — so an `Active` carries nothing a door could take as evidence, and constructing one
/// forges nothing. A build returns one rather than the bare form because the build is quantified
/// over `'cell`: a bare `T::At<'cell>` cannot be normalized there under `'graph: 'cell`, and this
/// type's where-clause is what carries that bound. Bounded only by [`Reattachable`], since a
/// continuation's family is no different to it and rests in its cell's slot rather than a region,
/// where drop glue is fine.
pub struct Active<'graph, 'cell, T: Reattachable<'graph>>
where
    'graph: 'cell,
{
    value: T::At<'cell>,
}

impl<'graph, 'cell, T: Reattachable<'graph>> Active<'graph, 'cell, T> {
    /// Hold a family value at `'cell` — what a build closure ends in.
    pub fn new(value: T::At<'cell>) -> Self {
        Active { value }
    }

    /// The value.
    pub fn value(&self) -> T::At<'cell>
    where
        T::At<'cell>: Copy,
    {
        self.value
    }

    /// The value, consuming the `Active` — the by-move twin of [`value`](Active::value) for a family
    /// whose live form is not `Copy`.
    pub fn into_value(self) -> T::At<'cell> {
        self.value
    }
}
