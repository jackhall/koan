//! The **slot array**: a fixed run of three-state binding cells in one bump allocation, addressed
//! by index rather than by key. The layout-addressed counterpart of [`BumpBackedMap`], for a table
//! whose key set is fixed before its first write — a per-call frame's value bindings, sized by the
//! body's own [`SlotLayout`](crate::machine::model::SlotLayout).
//!
//! A cell is [`SlotState`]: `Empty`, `Claimed` on the in-flight binder's producer, or `Bound` to a
//! payload. One cell answers both of a name's questions — "is it bound?" and "is a binder for it in
//! flight?" — so a channel storing claims in its cells needs no second structure keyed on the same
//! name, and the transitions between the three states live on the cell itself so a keyed table over
//! the same cell type rules on a write exactly as this array does.
//!
//! Both type parameters are the embedder's: the array is the shape, and what a bound slot *holds*
//! is a choice made where it is instantiated.
//!
//! **Drop-freeness.** The buffer wears its own `ManuallyDrop` for [`BumpVec`]'s reason — its bytes
//! are bump memory the region releases whole, so the vec's destructor would only hand a
//! bump-owned buffer back to an allocator that frees nothing. That wrapper would also swallow the
//! element proof, so [`SlotArray::new`] restates it as a `const` assert against the cell type
//! directly: a payload bringing drop glue with it fails the build at the instantiation site.

use std::mem::ManuallyDrop;

use super::substrate::{BumpAllocator, BumpVec};

/// One binding cell: unwritten, claimed by an in-flight binder, or bound.
///
/// `Claimed` and `Bound` are exclusive by construction rather than by a checked order — a commit
/// *replaces* the claim it satisfies — which is what lets one probe answer a name's whole state.
pub enum SlotState<V, P> {
    Empty,
    Claimed(P),
    Bound(V),
}

/// What a cell already holds when a write cannot proceed. Carries the standing producer on the
/// `Claimed` arm so a caller can rule on a same-producer re-entry without a second read.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SlotConflict<P> {
    Claimed(P),
    Bound,
}

impl<V, P: Copy> SlotState<V, P> {
    /// The standing producer, if a binder for this cell is still in flight.
    pub fn claimed_by(&self) -> Option<P> {
        match self {
            SlotState::Claimed(producer) => Some(*producer),
            SlotState::Empty | SlotState::Bound(_) => None,
        }
    }

    /// The bound payload, if the cell is committed.
    pub fn bound(&self) -> Option<&V> {
        match self {
            SlotState::Bound(payload) => Some(payload),
            SlotState::Empty | SlotState::Claimed(_) => None,
        }
    }

    /// Stamp `producer`'s claim on an unwritten cell. `Ok(true)` says a claim was added — what a
    /// live-claim counter beside the cells reads. A cell already claimed or bound conflicts, and
    /// the caller rules on whether a standing claim is a re-entry of the same binder.
    pub fn claim(&mut self, producer: P) -> Result<bool, SlotConflict<P>> {
        match self {
            SlotState::Empty => {
                *self = SlotState::Claimed(producer);
                Ok(true)
            }
            SlotState::Claimed(standing) => Err(SlotConflict::Claimed(*standing)),
            SlotState::Bound(_) => Err(SlotConflict::Bound),
        }
    }

    /// Commit `payload`, **retiring the cell's own claim** by replacing it. `Ok(true)` says a claim
    /// went away with the write. Binding is once: a committed cell conflicts.
    pub fn bind(&mut self, payload: V) -> Result<bool, SlotConflict<P>> {
        match self {
            SlotState::Bound(_) => Err(SlotConflict::Bound),
            SlotState::Empty | SlotState::Claimed(_) => {
                let retired = matches!(self, SlotState::Claimed(_));
                *self = SlotState::Bound(payload);
                Ok(retired)
            }
        }
    }

    /// Drop an unsatisfied claim — what a binder that terminalizes without committing leaves
    /// behind. `true` if a claim was standing. A bound cell is untouched: the commit already
    /// retired the claim it satisfied.
    pub fn retire_claim(&mut self) -> bool {
        match self {
            SlotState::Claimed(_) => {
                *self = SlotState::Empty;
                true
            }
            SlotState::Empty | SlotState::Bound(_) => false,
        }
    }
}

/// `len` binding cells in one bump allocation, beside the count of those currently claimed.
///
/// The counter is what makes "no binder is still in flight here" an O(1) read rather than a walk —
/// the half of a copy-readiness gate this channel owns.
pub struct SlotArray<'a, V, P> {
    cells: ManuallyDrop<BumpVec<'a, SlotState<V, P>>>,
    claimed: usize,
}

impl<'a, V, P: Copy> SlotArray<'a, V, P> {
    /// `len` empty cells over `alloc`'s bump — one allocation, sized exactly, never grown.
    pub fn new(alloc: BumpAllocator<'a>, len: usize) -> Self {
        const {
            assert!(
                !std::mem::needs_drop::<SlotState<V, P>>(),
                "a bump-hosted slot cell must carry no drop glue: the bump runs no destructor",
            )
        };
        let mut cells = BumpVec::with_capacity_in(len, alloc);
        cells.resize_with(len, || SlotState::Empty);
        SlotArray {
            cells: ManuallyDrop::new(cells),
            claimed: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// The cell at `slot`.
    pub fn get(&self, slot: usize) -> &SlotState<V, P> {
        &self.cells[slot]
    }

    /// [`SlotState::claim`] at `slot`, keeping the live-claim count.
    pub fn claim(&mut self, slot: usize, producer: P) -> Result<(), SlotConflict<P>> {
        self.claimed += usize::from(self.cells[slot].claim(producer)?);
        Ok(())
    }

    /// [`SlotState::bind`] at `slot`, keeping the live-claim count.
    pub fn bind(&mut self, slot: usize, payload: V) -> Result<(), SlotConflict<P>> {
        self.claimed -= usize::from(self.cells[slot].bind(payload)?);
        Ok(())
    }

    /// [`SlotState::retire_claim`] at `slot`, keeping the live-claim count.
    pub fn retire_claim(&mut self, slot: usize) {
        self.claimed -= usize::from(self.cells[slot].retire_claim());
    }

    /// How many binders are still in flight into this array — one field read, no walk.
    pub fn claimed_count(&self) -> usize {
        self.claimed
    }

    /// Every cell in slot order.
    pub fn iter(&self) -> impl Iterator<Item = (usize, &SlotState<V, P>)> {
        self.cells.iter().enumerate()
    }
}

#[cfg(test)]
mod tests;
