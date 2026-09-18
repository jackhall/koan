//! The receipt run: a fixed-width run of slots a cell parks on, laid down by the substrate in the
//! cell's write home scratch habitat, filled by other cells' steps and drained by the owning cell.
//! The producer half of a push — see [../README.md](../README.md) § Passing values between cells.
//!
//! A run holds **nothing alive**. Its slots carry `Dormant`s, which carry no reach, and `Erased`
//! values, which are bytes; no mint, no verdict, no price and no table entry stand behind one. What
//! it does hold is the reset of the bump it lives in, exactly as a scratch continuation does: the
//! run is one of the things at rest that names those bytes.
//!
//! The two families a run's slots store reach the graph as one [`Delivery`] bundle rather than as
//! two parameters of their own. The bundle carries the [`DropFree`] bound both need — a delivered
//! value lands in a bump — so no such bound propagates into the graph, the cells or the pools, and
//! it keeps [`ReceiptRun`]'s own family at one parameter, which the
//! [`reattachable!`](crate::reattachable) macro's generic arm already expresses.

use std::cell::Cell;
use std::marker::PhantomData;

use crate::dormant::Dormant;
use crate::reattach::{DropFree, Erased, Reattachable};

/// What a graph's cells deliver: the family of a value a producer builds in a consumer's scratch
/// habitat, and the family of a carrier it files there at rest.
///
/// One bundle per graph, so a run is one slot type over two named positions rather than two runs
/// that would separate nothing: a [`Dormant`] carries no region brand, so both kinds rest at
/// `'scratch`, and a consumer parked on several producers awaits a mix, which makes completeness a
/// conjunction a single run answers in one read.
///
/// The bundle outlives the graph, which is what lets a run's spine rest behind a borrow at any
/// brand the graph hands out: `'graph` outlives every one of them, so a bundle that outlives
/// `'graph` outlives them too. A bundle is a marker naming two families and holds nothing, so this
/// costs an implementor nothing.
pub trait Delivery<'graph>: 'graph {
    /// A value a producer builds through the consumer's own scratch writer, operand-free.
    type Scratch: Reattachable<'graph> + DropFree;
    /// A carrier a producer already holds and files at rest, redeemed by the consumer's drain.
    type Carrier: Reattachable<'graph> + DropFree;
}

/// A graph whose cells deliver nothing: the default bundle, and its own family at both positions.
///
/// Its form is `()`, which has no drop glue — what the lay-down's `needs_drop` assert demands of
/// every graph, whether or not that graph ever registers a run. Inhabited rather than uninhabited
/// so a default graph can still fill a slot, with nothing in it.
pub struct NoDelivery;

crate::reattachable!(NoDelivery => ());
impl DropFree for NoDelivery {}

impl<'graph> Delivery<'graph> for NoDelivery {
    type Scratch = NoDelivery;
    type Carrier = NoDelivery;
}

/// One slot of a receipt run: what a producer filled it with, or nothing.
pub(crate) enum ReceiptSlot<'graph, D: Delivery<'graph>> {
    Empty,
    /// A value built in the consumer's own scratch habitat, erased for rest.
    Value(Erased<'graph, D::Scratch>),
    /// A carrier the producer put to rest. Carries no reach, like every [`Dormant`].
    Carrier(Dormant<'graph, D::Carrier>),
}

/// A cell's receipt run, at the brand of whatever named the scratch bump it lives in.
///
/// Two shared references into that bump, so the run is `Copy`: a step reads the at-rest copy,
/// re-anchors it, and writes through the same cells the copy at rest names — a drained slot is
/// drained for both.
pub(crate) struct Receipts<'graph, 'cell, D: Delivery<'graph>> {
    /// How many slots are filled. What answers "is the run complete" in one read, and what refuses
    /// a registration over a run still holding receipts.
    filled: &'cell Cell<u32>,
    slots: &'cell [Cell<ReceiptSlot<'graph, D>>],
}

impl<'graph, 'cell, D: Delivery<'graph>> Receipts<'graph, 'cell, D> {
    /// Bundle a count with the slots it counts. The two land in the bump together, at the step end
    /// that lays the run down.
    pub(crate) fn new(
        filled: &'cell Cell<u32>,
        slots: &'cell [Cell<ReceiptSlot<'graph, D>>],
    ) -> Self {
        Receipts { filled, slots }
    }

    /// How many slots the run has.
    pub(crate) fn width(self) -> usize {
        self.slots.len()
    }

    /// Whether every slot is empty — what lets a registration replace this run, and what a drained
    /// run reads as.
    pub(crate) fn drained(self) -> bool {
        self.filled.get() == 0
    }

    /// Whether every slot is filled.
    pub(crate) fn complete(self) -> bool {
        self.filled.get() as usize == self.slots.len()
    }

    /// Whether the slot at `index` is empty, or `None` when the run is not that wide.
    ///
    /// The read goes out through the cell and back, since a slot holds a family's form and nothing
    /// bounds it `Copy`. What comes out goes back as it was: this is the check a fill makes before
    /// it writes a byte, and a refused fill leaves the run exactly as it found it.
    pub(crate) fn vacant(self, index: usize) -> Option<bool> {
        let slot = self.slots.get(index)?;
        let held = slot.replace(ReceiptSlot::Empty);
        let empty = matches!(held, ReceiptSlot::Empty);
        slot.set(held);
        Some(empty)
    }

    /// Fill the slot at `index`, which the caller has established is vacant, and answer whether the
    /// run is now complete.
    pub(crate) fn fill(self, index: usize, receipt: ReceiptSlot<'graph, D>) -> Delivered {
        self.slots[index].set(receipt);
        self.filled.set(self.filled.get() + 1);
        if self.complete() {
            Delivered::Complete
        } else {
            Delivered::Outstanding
        }
    }

    /// Take the slot at `index`, leaving it empty. `None` when the run is not that wide.
    pub(crate) fn take(self, index: usize) -> Option<ReceiptSlot<'graph, D>> {
        let taken = self.slots.get(index)?.replace(ReceiptSlot::Empty);
        if !matches!(taken, ReceiptSlot::Empty) {
            self.filled.set(self.filled.get() - 1);
        }
        Some(taken)
    }
}

/// Duplicating a run duplicates no ownership: it is two shared borrows of bump bytes it does not
/// own. Hand-written, since a derive would demand `D: Copy` of a bundle that is never a value.
impl<'graph, D: Delivery<'graph>> Clone for Receipts<'graph, '_, D> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'graph, D: Delivery<'graph>> Copy for Receipts<'graph, '_, D> {}

/// The receipt run's family, so a run rests in a cell's slot free of every step brand.
pub(crate) struct ReceiptRun<D>(PhantomData<fn() -> D>);

crate::reattachable!(ReceiptRun<D: crate::Delivery<'graph>> => Receipts<'graph, 'cell, D>);

/// What a fill answered: whether the run it landed in is now complete.
///
/// The one thing a producer learns about its consumer. It is a read of the count the fill just
/// moved, not a wake-up: nothing about scheduling is the substrate's.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Delivered {
    /// Every slot of the run is filled.
    Complete,
    /// At least one slot is still empty.
    Outstanding,
}
