//! [`Dormant`] — the carrier at rest: a value put down in its home cell's region between steps,
//! with no lifetime of its own. The least live of the three states a value with reach passes
//! through ([design/cellgraph.md](../design/cellgraph.md) § The contract: two embedder types), and
//! the only one an embedder may hold across an `enter` scope.
//!
//! A dormant carrier carries **no reach**. Its mask lives in its home cell's reach table, where
//! the seal transition can rewrite it as the slab bit it names becomes a sealed id; the dormant
//! carrier names that entry by a private key and nothing else. So an embedder cannot pair a value
//! with a reach from outside — there is nothing pairable — and a mask a dormant carrier depends on
//! cannot go stale, because it never left the reach table. Two dormant carriers that reach the
//! same thing name one entry: the reach table interns on content, so a cell kept into every step
//! of a run holds one mask per distinct reach rather than one per keep.
//!
//! It carries no *live value* either, and that is what separates this state from the in-step one. A
//! dormant carrier outlives the step that built it, so by the time one is redeemed its home's
//! storage may be gone — reclaimed, or retired with the sealed cell it sealed into. A reference
//! into freed chunks is an invalid value the moment it is moved, whether or not anything reads
//! through it, so the value rests here as **bytes**: parked at the `keep`, reconstituted only once
//! the redeem door has established a claim on the storage it names.

use std::mem::MaybeUninit;

use crate::handle::CellHandle;
use crate::reach::GraphReach;
use crate::reattach::{DropFree, Erased, Reattachable};

/// The at-rest carrier: a value's bytes, parked, plus the key naming its reach.
///
/// Opaque: it has no method at all. [`StepContext::redeem`] is the only door out, and it refuses
/// unless the executing cell is entitled to the storage the reach names.
///
/// `Copy` when the family's erased form is, for the same reason [`Ready`] is: the parked value
/// names region bytes it does not own, and the key is two words.
///
/// [`StepContext::redeem`]: crate::StepContext::redeem
/// [`Ready`]: crate::Ready
pub struct Dormant<T: Reattachable + DropFree> {
    /// The value parked. `MaybeUninit` is the whole point rather than an implementation detail: a
    /// carrier at rest must be movable after its home's storage is gone, and a `T::At<'static>`
    /// holding a reference into freed chunks is not. Parking asserts nothing about the referents,
    /// so a dormant carrier whose home has been reclaimed is an ordinary value the door refuses.
    ///
    /// Nothing is lost by never reconstituting one: [`DropFree`] is what the value doors bound on,
    /// and the assertion below is the check that the family really runs no destructor.
    value: MaybeUninit<Erased<T>>,
    key: DormantKey,
}

impl<T: Reattachable + DropFree> Dormant<T> {
    pub(crate) fn new(value: Erased<T>, key: DormantKey) -> Self {
        // A parked value is never dropped, so a family with drop glue would leak whatever it owns
        // whenever a dormant carrier goes unredeemed. `DropFree` declares the absence; this is the
        // check.
        const { assert!(!std::mem::needs_drop::<T::At<'static>>()) };
        Dormant {
            value: MaybeUninit::new(value),
            key,
        }
    }

    /// Which entry of which cell's reach table holds this value's reach. Readable without
    /// disturbing the parked value, which is what lets the redeem door decide before it
    /// reconstitutes anything.
    pub(crate) fn key(&self) -> DormantKey {
        self.key
    }

    /// Reconstitute the parked value.
    ///
    /// # Safety
    ///
    /// The storage the value's referents name must still be there. The redeem door establishes
    /// exactly that before it calls: the key's home resolves to a live slab slot or a present
    /// sealed cell, and the executing cell holds it. A dormant carrier whose home resolves to
    /// neither must be refused rather than opened — the bytes are still bytes, but the references
    /// in them are not.
    pub(crate) unsafe fn take(self) -> Erased<T> {
        // SAFETY: `new` is the only constructor and it always initializes; the caller's contract
        // is what makes the referents in those bytes valid again.
        unsafe { self.value.assume_init() }
    }
}

impl<T: Reattachable + DropFree> Clone for Dormant<T>
where
    Erased<T>: Copy,
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Reattachable + DropFree> Copy for Dormant<T> where Erased<T>: Copy {}

/// Which entry of which cell's reach table holds one dormant carrier's reach.
///
/// Crate-private, like the mask itself: an embedder cannot name an entry, so it cannot hand a
/// value a reach that is not its own. The home is the cell as it stood at the
/// [`keep`](crate::StepContext::keep) — a generation the redeem re-checks, and the key the
/// relocation map is looked up under once a slab cell has left the slab, or the tombstone chain is
/// followed from once a tree cell has died.
///
/// A tree home names no reach-table entry: a value homed in a tree cell reaches its root and
/// nothing else, so the index is zero and the redeem derives the reach from the root rather than
/// reading it back.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct DormantKey {
    pub(crate) home: CellHandle,
    pub(crate) index: u32,
}

/// One cell's reach table: the reach of every value kept in its cell region, indexed by the
/// position a [`keep`](crate::StepContext::keep) handed out.
///
/// This is the **only** durable habitat of a mask on the slab side, so the seal transition's step 1
/// rewrites exactly this collection per holder and the work is bounded by the holders' entry
/// counts. It holds *reaches*, not [`Dormant`]s — a dormant carrier carries no reach of its own and
/// names an entry here by a private key, which is what keeps a value and its reach unpairable from
/// outside. Entries are **interned on content**, which is what bounds that count: a reach table
/// holds one entry per *distinct* reach ever kept into the cell, not one per keep, so a cell kept
/// into every step for a whole run settles at the handful of shapes its keeps take. Nothing is ever
/// removed — an index is a name — and a cell's whole reach table goes when its slot recycles.
///
/// Interning is sound because an entry is immutable content: no door writes one by index, and the
/// only rewrites are the uniform ones the seal transition and a merge apply to every entry alike,
/// which carry equal masks to equal masks. The continuation is no exception — it interns its reach
/// like any other keep and repoints, rather than owning an entry it overwrites.
#[derive(Default)]
pub(crate) struct ReachTable<const W: usize> {
    masks: Vec<GraphReach<W>>,
}

impl<const W: usize> ReachTable<W> {
    /// Take a reach in, handing back the index that names it from here on — the entry that already
    /// holds an equal mask when there is one, so a cell kept into repeatedly with the same reach
    /// takes one entry rather than one per keep.
    ///
    /// The scan is linear in the reach table, which interning is what keeps small: the cost is
    /// the number of distinct reaches the cell has ever been kept into, and every hit is an entry
    /// the reach table did not grow by.
    pub(crate) fn intern(&mut self, reach: GraphReach<W>) -> u32 {
        match self.masks.iter().position(|mask| *mask == reach) {
            Some(index) => index as u32,
            None => self.append(reach),
        }
    }

    /// Add an entry without consulting the existing ones — how a merge moves a departed cell's
    /// reach table in, where the block's position is what forwards its keys and an intern hit would
    /// put an entry at the wrong offset.
    pub(crate) fn append(&mut self, reach: GraphReach<W>) -> u32 {
        let index = self.masks.len() as u32;
        self.masks.push(reach);
        index
    }

    pub(crate) fn get(&self, index: u32) -> Option<&GraphReach<W>> {
        self.masks.get(index as usize)
    }

    pub(crate) fn len(&self) -> u32 {
        self.masks.len() as u32
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.masks.is_empty()
    }

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut GraphReach<W>> {
        self.masks.iter_mut()
    }

    #[cfg(test)]
    pub(crate) fn iter(&self) -> impl Iterator<Item = &GraphReach<W>> {
        self.masks.iter()
    }

    /// Take the entries out whole, leaving the reach table empty — how a cell absorbed into
    /// another hands its reaches over.
    pub(crate) fn take(&mut self) -> Vec<GraphReach<W>> {
        std::mem::take(&mut self.masks)
    }
}
