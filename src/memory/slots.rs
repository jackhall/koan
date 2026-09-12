//! The **slot array**: a fixed run of three-state binding cells laid down in a cell's region,
//! addressed by index rather than by key — for a table whose key set is fixed before its first
//! write, such as a call's value bindings, sized by the body's own
//! `parse`'s `SlotLayout`.
//!
//! A slot is [`SlotState`]: `Empty`, `Claimed` on the in-flight binder's producer, or `Bound` to a
//! payload. One slot answers both of a name's questions — "is it bound?" and "is a binder for it in
//! flight?" — so a channel storing claims in its slots needs no second structure keyed on the same
//! name, and the transitions between the three states live on the state itself.
//!
//! Both type parameters are the embedder's: the array is the shape, and what a bound slot *holds*
//! is a choice made where it is instantiated.
//!
//! **A value at rest in the region.** [`SlotArray::new`] lays the slots down through
//! [`Writer::fill`], which hands back a shared `&'cell` borrow, never `&mut`; a continuation
//! captures `'cell` borrows, so every write after construction goes through interior mutability.
//! Each slot is a [`Cell`] — the zero-cost kind, with no borrow flag — and a `Cell` never lends a
//! `&T`, so reads copy: `V` and `P` are `Copy`, reads return [`SlotState`] by value, and a
//! transition is a by-value function the array applies with [`Cell::set`]. The array itself is two
//! `'cell` borrows and `Copy`, so a continuation captures it by value; its live-claim counter lives
//! in the region beside the slots, since a counter inside a `Copy` struct would diverge between
//! copies.
//!
//! **Drop-freeness.** The region runs no destructor. `Writer::fill` asserts that for its element
//! type at compile time, and [`SlotArray::new`] restates the assert against the slot type so a
//! payload bringing drop glue fails the build with a message naming the slot array.

use std::cell::Cell;

use super::substrate::Writer;

/// One binding slot: unwritten, claimed by an in-flight binder, or bound.
///
/// `Claimed` and `Bound` are exclusive by construction rather than by a checked order — a commit
/// *replaces* the claim it satisfies — which is what lets one probe answer a name's whole state.
#[derive(Clone, Copy)]
pub enum SlotState<V, P> {
    Empty,
    Claimed(P),
    Bound(V),
}

/// What a slot already holds when a write cannot proceed. Carries the standing producer on the
/// `Claimed` arm so a caller can rule on a same-producer re-entry without a second read.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SlotConflict<P> {
    Claimed(P),
    Bound,
}

impl<V: Copy, P: Copy> SlotState<V, P> {
    /// The standing producer, if a binder for this slot is still in flight.
    pub fn claimed_by(self) -> Option<P> {
        match self {
            SlotState::Claimed(producer) => Some(producer),
            SlotState::Empty | SlotState::Bound(_) => None,
        }
    }

    /// The bound payload, if the slot is committed.
    pub fn bound(self) -> Option<V> {
        match self {
            SlotState::Bound(payload) => Some(payload),
            SlotState::Empty | SlotState::Claimed(_) => None,
        }
    }

    /// Stamp `producer`'s claim on an unwritten slot: the next state, and whether a claim was
    /// added — what a live-claim counter reads. A slot already claimed or bound conflicts, and the
    /// caller rules on whether a standing claim is a re-entry of the same binder.
    pub fn claim(self, producer: P) -> Result<(Self, bool), SlotConflict<P>> {
        match self {
            SlotState::Empty => Ok((SlotState::Claimed(producer), true)),
            SlotState::Claimed(standing) => Err(SlotConflict::Claimed(standing)),
            SlotState::Bound(_) => Err(SlotConflict::Bound),
        }
    }

    /// Commit `payload`, **retiring the slot's own claim** by replacing it: the next state, and
    /// whether a claim went away with the write. Binding is once: a committed slot conflicts.
    pub fn bind(self, payload: V) -> Result<(Self, bool), SlotConflict<P>> {
        match self {
            SlotState::Bound(_) => Err(SlotConflict::Bound),
            SlotState::Empty => Ok((SlotState::Bound(payload), false)),
            SlotState::Claimed(_) => Ok((SlotState::Bound(payload), true)),
        }
    }

    /// Drop an unsatisfied claim — what a binder that terminalizes without committing leaves
    /// behind: the next state, and whether a claim was standing. A bound slot is untouched: the
    /// commit already retired the claim it satisfied.
    pub fn retire_claim(self) -> (Self, bool) {
        match self {
            SlotState::Claimed(_) => (SlotState::Empty, true),
            SlotState::Empty | SlotState::Bound(_) => (self, false),
        }
    }
}

/// `len` binding slots in a cell's region, beside the count of those currently claimed.
///
/// The counter is what makes "no binder is still in flight here" an O(1) read rather than a walk —
/// the half of a copy-readiness gate this channel owns.
pub struct SlotArray<'cell, V, P> {
    slots: &'cell [Cell<SlotState<V, P>>],
    claimed: &'cell Cell<usize>,
}

impl<V, P> Clone for SlotArray<'_, V, P> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<V, P> Copy for SlotArray<'_, V, P> {}

impl<'cell, V: Copy, P: Copy> SlotArray<'cell, V, P> {
    /// `len` empty slots and a zero counter, laid down in the region `writer` names.
    pub fn new(writer: Writer<'cell>, len: usize) -> Self {
        const {
            assert!(
                !std::mem::needs_drop::<Cell<SlotState<V, P>>>(),
                "a region-resident slot must carry no drop glue: the region runs no destructor",
            )
        };
        let slots = writer.fill(len, |_| Cell::new(SlotState::Empty));
        let claimed = &writer.fill(1, |_| Cell::new(0usize))[0];
        SlotArray { slots, claimed }
    }

    pub fn len(self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(self) -> bool {
        self.slots.is_empty()
    }

    /// The state of the slot at `slot`.
    pub fn get(self, slot: usize) -> SlotState<V, P> {
        self.slots[slot].get()
    }

    /// [`SlotState::claim`] at `slot`, keeping the live-claim count.
    pub fn claim(self, slot: usize, producer: P) -> Result<(), SlotConflict<P>> {
        let (next, added) = self.slots[slot].get().claim(producer)?;
        self.slots[slot].set(next);
        self.claimed.set(self.claimed.get() + usize::from(added));
        Ok(())
    }

    /// [`SlotState::bind`] at `slot`, keeping the live-claim count.
    pub fn bind(self, slot: usize, payload: V) -> Result<(), SlotConflict<P>> {
        let (next, retired) = self.slots[slot].get().bind(payload)?;
        self.slots[slot].set(next);
        self.claimed.set(self.claimed.get() - usize::from(retired));
        Ok(())
    }

    /// [`SlotState::retire_claim`] at `slot`, keeping the live-claim count.
    pub fn retire_claim(self, slot: usize) {
        let (next, retired) = self.slots[slot].get().retire_claim();
        self.slots[slot].set(next);
        self.claimed.set(self.claimed.get() - usize::from(retired));
    }

    /// How many binders are still in flight into this array — one read, no walk.
    pub fn claimed_count(self) -> usize {
        self.claimed.get()
    }

    /// Every slot's state in slot order.
    pub fn iter(self) -> impl Iterator<Item = (usize, SlotState<V, P>)> + 'cell {
        self.slots.iter().map(Cell::get).enumerate()
    }
}

#[cfg(test)]
mod tests;
