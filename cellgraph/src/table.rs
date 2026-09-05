//! The cell table: a slab capped at construction, the birth relation over its slots, the executing
//! flag, and the `create` / `enter` / `release` verbs. See
//! [design/cellgraph.md](../design/cellgraph.md) § Verbs.

#[cfg(test)]
mod tests;

use crate::handle::{Handle, StaleHandle};
use crate::matrix::{BitRow, Matrix};
use crate::reattach::{Erased, Reattachable};

/// Refusals from [`CellTable::create`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CreateError {
    /// The slab is at its cap. What to do next is admission policy, and the embedder's.
    SlabFull,
    /// The named parent is not a live cell.
    StaleParent(StaleHandle),
}

/// Refusals from [`CellTable::enter`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EnterError {
    /// The named cell is not a live cell.
    Stale(StaleHandle),
    /// The cell is already executing; a cell is entered by one step at a time.
    AlreadyExecuting,
}

/// Refusals from [`CellTable::release`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReleaseError {
    /// The named cell is not a live cell — a second release names a death already declared.
    Stale(StaleHandle),
    /// The cell is executing. Death is declared from outside a step, never from within one.
    Executing,
}

/// What a slab slot currently holds. `Dead` is the resident state: the embedder declared the
/// cell's death, but something still names it, so the slot is not yet reusable.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SlotState {
    Free,
    Live,
    Dead,
}

struct Slot<C: Reattachable> {
    generation: u32,
    state: SlotState,
    continuation: Option<Erased<C>>,
}

/// A capped slab of cells over the relations that decide when a slot may be reused.
///
/// `C` is the embedder's continuation family: a one-lifetime family the table stores erased, hands
/// back re-anchored under [`enter`](CellTable::enter), and never calls.
pub struct CellTable<C: Reattachable> {
    slots: Box<[Slot<C>]>,
    free: Vec<u32>,
    birth: Matrix,
    executing: BitRow,
}

impl<C: Reattachable> CellTable<C> {
    /// A slab of `cap` cells. The cap is fixed here and the table never grows past it: every
    /// relation is a fixed-width row over these slots.
    pub fn new(cap: u32) -> Self {
        let slots = (0..cap)
            .map(|_| Slot {
                generation: 0,
                state: SlotState::Free,
                continuation: None,
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        CellTable {
            slots,
            free: (0..cap).rev().collect(),
            birth: Matrix::new(cap),
            executing: BitRow::new(cap),
        }
    }

    /// Take a free slot for a new cell, optionally under a parent and with a continuation.
    ///
    /// The new cell's birth row is the parent's row plus the parent's bit, so the row is the
    /// parent chain's transitive closure by construction. A cell created without a continuation is
    /// storage-only: it is enterable, but a step finds nothing to run.
    pub fn create(
        &mut self,
        parent: Option<Handle>,
        continuation: Option<C::At<'static>>,
    ) -> Result<Handle, CreateError> {
        let parent_slot = match parent {
            Some(parent) => Some(self.live_slot(parent).map_err(CreateError::StaleParent)?),
            None => None,
        };
        let slot = self.free.pop().ok_or(CreateError::SlabFull)?;
        let cell = &mut self.slots[slot as usize];
        cell.state = SlotState::Live;
        cell.continuation = continuation.map(Erased::store);
        let generation = cell.generation;
        if let Some(parent_slot) = parent_slot {
            self.birth.row_or_assign(slot, parent_slot);
            self.birth.set(slot, parent_slot);
        }
        Ok(Handle::new(slot, generation))
    }

    /// Run `step` against the cell, with its executing flag set for the scope.
    ///
    /// The step receives a context, not the table, so it can neither enter another cell nor
    /// declare a death; the table stays exclusively borrowed for the whole call. The context's
    /// lifetime is a fresh brand the step cannot leak, since `R` is chosen outside the call and so
    /// cannot name it — that is what makes handing the continuation back re-anchored at it sound.
    ///
    /// Re-entering the executing cell is not representable: the step never holds the table.
    ///
    /// ```compile_fail
    /// use cellgraph::{CellTable, reattachable};
    /// struct Owned;
    /// reattachable!(Owned => String);
    ///
    /// let mut table: CellTable<Owned> = CellTable::new(4);
    /// let cell = table.create(None, None).unwrap();
    /// table
    ///     .enter(cell, |_context| {
    ///         // `table` is already exclusively borrowed by the `enter` this closure runs under.
    ///         let _ = table.enter(cell, |_| ());
    ///     })
    ///     .unwrap();
    /// ```
    pub fn enter<R>(
        &mut self,
        handle: Handle,
        step: impl FnOnce(&mut StepContext<'_, C>) -> R,
    ) -> Result<R, EnterError> {
        self.begin(handle)?;
        let mut context = StepContext {
            table: self,
            handle,
        };
        Ok(step(&mut context))
    }

    /// Declare the cell's death: the embedder promises never to enter it again.
    ///
    /// The cell's own birth row releases wholesale. The slot is then reused if nothing names the
    /// cell, and stays resident otherwise — reclaimed later, when the last cell that names it dies
    /// in turn.
    pub fn release(&mut self, handle: Handle) -> Result<(), ReleaseError> {
        let slot = self.live_slot(handle).map_err(ReleaseError::Stale)?;
        if self.executing.test(slot) {
            return Err(ReleaseError::Executing);
        }
        self.birth.clear_row(slot);
        self.slots[slot as usize].state = SlotState::Dead;
        self.reclaim_to_fixpoint();
        Ok(())
    }

    /// Whether the handle names a cell that is still live — false for a slot that is free, holds a
    /// later generation, or holds a cell whose death was already declared.
    pub fn is_live(&self, handle: Handle) -> bool {
        self.live_slot(handle).is_ok()
    }

    /// The slot a handle names, if that slot still holds the live cell the handle was minted for.
    fn live_slot(&self, handle: Handle) -> Result<u32, StaleHandle> {
        match self.slots.get(handle.slot() as usize) {
            Some(cell)
                if cell.state == SlotState::Live && cell.generation == handle.generation() =>
            {
                Ok(handle.slot())
            }
            _ => Err(StaleHandle(handle)),
        }
    }

    /// Set the executing flag, or refuse. Paired with the clear in [`StepContext`]'s `Drop`, so
    /// the flag falls even if the step panics.
    fn begin(&mut self, handle: Handle) -> Result<(), EnterError> {
        let slot = self.live_slot(handle).map_err(EnterError::Stale)?;
        if self.executing.test(slot) {
            return Err(EnterError::AlreadyExecuting);
        }
        self.executing.set(slot);
        Ok(())
    }

    /// A dead cell's slot is reusable once its executing flag is clear and no live cell's birth
    /// row names it.
    fn reclaimable(&self, slot: u32) -> bool {
        !self.executing.test(slot)
            && !(0..self.slots.len() as u32).any(|other| {
                self.slots[other as usize].state == SlotState::Live && self.birth.test(other, slot)
            })
    }

    /// Reclaim every dead cell whose rows have gone clear, repeating until none does — one death
    /// can free a chain of cells that were each held only by the next.
    fn reclaim_to_fixpoint(&mut self) {
        loop {
            let mut reclaimed = false;
            for slot in 0..self.slots.len() as u32 {
                if self.slots[slot as usize].state == SlotState::Dead && self.reclaimable(slot) {
                    self.reclaim(slot);
                    reclaimed = true;
                }
            }
            if !reclaimed {
                return;
            }
        }
    }

    /// Return a dead cell's slot to the free list under a fresh generation, so every handle minted
    /// for the departing occupant is stale from here on.
    fn reclaim(&mut self, slot: u32) {
        let cell = &mut self.slots[slot as usize];
        cell.state = SlotState::Free;
        cell.continuation = None;
        cell.generation = cell.generation.wrapping_add(1);
        self.free.push(slot);
    }
}

/// The view of the table a step gets: its own cell's continuation slot, and its own identity.
///
/// `'b` is the step's brand — the lifetime of the table borrow the enclosing
/// [`enter`](CellTable::enter) holds. Nothing carrying it escapes the call.
pub struct StepContext<'b, C: Reattachable> {
    table: &'b mut CellTable<C>,
    handle: Handle,
}

impl<'b, C: Reattachable> StepContext<'b, C> {
    /// The cell this step is running in.
    pub fn handle(&self) -> Handle {
        self.handle
    }

    /// Take the cell's continuation, re-anchored at the step brand. The slot is left empty: a
    /// continuation is one-shot, and a step that wants the cell entered again stores a successor.
    pub fn continuation(&mut self) -> Option<C::At<'b>> {
        let erased = self.table.slots[self.handle.slot() as usize]
            .continuation
            .take()?;
        // SAFETY: `erased` holds the family at `'static` (the only form `Erased::store` accepts),
        // so the re-anchor shortens rather than fabricates. `'b` is the enclosing `enter`'s table
        // borrow, unnameable by the step's return type, so nothing anchored at it escapes.
        Some(unsafe { erased.reattach::<'b>() })
    }

    /// Store the cell's next continuation, replacing whatever the slot holds.
    pub fn store_successor(&mut self, continuation: C::At<'static>) {
        self.table.slots[self.handle.slot() as usize].continuation =
            Some(Erased::store(continuation));
    }
}

impl<C: Reattachable> Drop for StepContext<'_, C> {
    fn drop(&mut self) {
        self.table.executing.clear(self.handle.slot());
    }
}
