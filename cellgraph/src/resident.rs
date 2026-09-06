//! [`Resident`] — the carrier at rest: a value put down in its home cell's region between steps,
//! with no lifetime of its own. The third of the three states a value with reach passes through
//! ([design/cellgraph.md](../design/cellgraph.md) § The contract: two embedder types), and the
//! only one an embedder may hold across an `enter` scope.
//!
//! A resident carries **no reach**. Its mask lives in its home cell's resident table, where the
//! seal transition can rewrite it as the slab bit it names becomes a sealed id; the resident names
//! that entry by a private key and nothing else. So an embedder cannot pair a value with a reach
//! from outside — there is nothing pairable — and a mask a resident depends on cannot go stale,
//! because it never left the table.

use crate::handle::Handle;
use crate::mask::Mask;
use crate::reattach::{DropFree, Erased, Reattachable};

/// The at-rest carrier: a value erased to its lifetime-free form, plus the key naming its reach.
///
/// Opaque: it has no method at all. [`StepContext::redeem`] is the only door out, and it refuses
/// unless the executing cell is entitled to the storage the reach names.
///
/// `Copy` when the family's erased form is, for the same reason [`Sealed`] is: the erased value
/// names region bytes it does not own, and the key is two words.
///
/// [`StepContext::redeem`]: crate::StepContext::redeem
/// [`Sealed`]: crate::Sealed
pub struct Resident<T: Reattachable + DropFree> {
    value: Erased<T>,
    key: ResidentKey,
}

impl<T: Reattachable + DropFree> Resident<T> {
    pub(crate) fn new(value: Erased<T>, key: ResidentKey) -> Self {
        Resident { value, key }
    }

    pub(crate) fn into_parts(self) -> (Erased<T>, ResidentKey) {
        (self.value, self.key)
    }
}

impl<T: Reattachable + DropFree> Clone for Resident<T>
where
    Erased<T>: Copy,
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Reattachable + DropFree> Copy for Resident<T> where Erased<T>: Copy {}

/// Which entry of which cell's resident table holds one resident's reach.
///
/// Crate-private, like the mask itself: an embedder cannot name an entry, so it cannot hand a
/// value a reach that is not its own. The handle is the home cell as it stood at the
/// [`keep`](crate::StepContext::keep) — a generation the redeem re-checks, and the key the
/// relocation map is looked up under once that cell has left the slab.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct ResidentKey {
    pub(crate) home: Handle,
    pub(crate) index: u32,
}

/// One cell's resident table: the reach of every value kept in its region, indexed by the position
/// a [`keep`](crate::StepContext::keep) handed out.
///
/// This is the **only** durable habitat of a mask on the slab side, so the seal transition's step 1
/// rewrites exactly this collection per holder and the work is bounded by the holders' resident
/// counts. Entries are never removed — an index is a name — so a table only grows, and a cell's
/// whole table goes when its slot recycles.
#[derive(Default)]
pub(crate) struct Residents {
    masks: Vec<Mask>,
}

impl Residents {
    /// Take a reach in, handing back the index that names it from here on.
    pub(crate) fn push(&mut self, reach: Mask) -> u32 {
        let index = self.masks.len() as u32;
        self.masks.push(reach);
        index
    }

    /// Overwrite one entry — how a re-stored continuation replaces the reach of the one before it
    /// rather than growing the table each step.
    pub(crate) fn set(&mut self, index: u32, reach: Mask) {
        self.masks[index as usize] = reach;
    }

    pub(crate) fn get(&self, index: u32) -> Option<&Mask> {
        self.masks.get(index as usize)
    }

    pub(crate) fn get_mut(&mut self, index: u32) -> Option<&mut Mask> {
        self.masks.get_mut(index as usize)
    }

    pub(crate) fn len(&self) -> u32 {
        self.masks.len() as u32
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.masks.is_empty()
    }

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut Mask> {
        self.masks.iter_mut()
    }

    #[cfg(test)]
    pub(crate) fn iter(&self) -> impl Iterator<Item = &Mask> {
        self.masks.iter()
    }

    /// Take the entries out whole, leaving the table empty — how a cell absorbed into another
    /// hands its residents over.
    pub(crate) fn take(&mut self) -> Vec<Mask> {
        std::mem::take(&mut self.masks)
    }
}
