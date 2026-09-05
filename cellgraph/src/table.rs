//! The cell table: a slab capped at construction, the two hold relations over its slots, the
//! executing flag, the per-cell regions, and the `create` / `enter` / `release` verbs. See
//! [design/cellgraph.md](../design/cellgraph.md) § Verbs and
//! [design/liveness-matrix.md](../design/liveness-matrix.md) § The model.

#[cfg(test)]
mod tests;

use crate::carrier::{Opened, Sealed};
use crate::handle::{Handle, StaleHandle};
use crate::mask::Mask;
use crate::matrix::{BitRow, Matrix};
use crate::reattach::{DropFree, Erased, Reattachable};
use crate::region::{Region, Writer};

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
    /// Minted at the cell's first allocation, so a cell that never allocates costs no chunk. Freed
    /// whole at reclamation, which is what makes a cell's death O(1) in its resident values.
    region: Option<Region>,
}

/// A capped slab of cells over the relations that decide when a slot may be reused.
///
/// `C` is the embedder's continuation family: a one-lifetime family the table stores erased, hands
/// back re-anchored under [`enter`](CellTable::enter), and never calls.
pub struct CellTable<C: Reattachable> {
    slots: Box<[Slot<C>]>,
    free: Vec<u32>,
    birth: Matrix,
    /// The pin relation: row M is the set of cells whose region storage M's own resident values
    /// read. Written only by [`CellTable::mint`], which is the mint OR of
    /// [liveness-matrix.md § Reach as a hybrid mask](../design/liveness-matrix.md#reach-as-a-hybrid-mask).
    pins: Matrix,
    executing: BitRow,
    cap: u32,
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
                region: None,
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        CellTable {
            slots,
            free: (0..cap).rev().collect(),
            birth: Matrix::new(cap),
            pins: Matrix::new(cap),
            executing: BitRow::new(cap),
            cap,
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
            self.birth.inherit_row(slot, parent_slot);
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

    /// A dead cell's slot is reusable once its executing flag is clear and no *occupied* slot names
    /// it in either relation.
    ///
    /// A dead-but-resident cell counts as a holder: its hold set releases at reclamation, not at
    /// its declared death, so its holds outlive it exactly as long as it does. That is what makes
    /// a ring a leak rather than a dangle — two cells naming each other stay resident through every
    /// pass of the fixpoint below — and it is the only reason the cascade is a fixpoint at all: a
    /// reclaim clears a row, which is what can bring another cell's holder set to zero.
    fn reclaimable(&self, slot: u32) -> bool {
        if self.executing.test(slot) {
            return false;
        }
        let occupied =
            || (0..self.cap).filter(|other| self.slots[*other as usize].state != SlotState::Free);
        !self.birth.held_by_any(occupied(), slot) && !self.pins.held_by_any(occupied(), slot)
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
        // The hold set releases wholesale here, not at the declared death: holds are monotone for
        // the cell's whole life, and the cleared entries name exactly the cells worth re-checking.
        self.pins.clear_row(slot);
        let cell = &mut self.slots[slot as usize];
        cell.state = SlotState::Free;
        cell.continuation = None;
        cell.region = None;
        cell.generation = cell.generation.wrapping_add(1);
        self.free.push(slot);
    }

    /// The mint: fold a value's reach into the hold set of the cell whose region now stores it,
    /// minus that cell's own bit. **The only write into the pin relation.** Private to the table,
    /// so every path that puts a value in a region passes through here.
    fn mint(&mut self, into: u32, reach: &Mask) {
        self.pins.mint(into, reach);
    }

    /// Walk the hold graph from `start` and report a cycle if one is reachable — the fail-safe
    /// diagnostic for a ring, which keeps both cells alive forever rather than dangling.
    ///
    /// Debug builds only, and **not consulted on any mint or release path**: preventing rings is
    /// the embedder's crossing discipline, not a mint-time reachability check.
    #[cfg(debug_assertions)]
    pub fn debug_ring_from(&self, start: Handle) -> Option<Vec<Handle>> {
        let mut path = Vec::new();
        let mut on_path = vec![false; self.cap as usize];
        let mut settled = vec![false; self.cap as usize];
        self.walk_for_ring(start.slot(), &mut path, &mut on_path, &mut settled)
            .map(|cycle| {
                cycle
                    .into_iter()
                    .map(|slot| Handle::new(slot, self.slots[slot as usize].generation))
                    .collect()
            })
    }

    #[cfg(debug_assertions)]
    fn walk_for_ring(
        &self,
        slot: u32,
        path: &mut Vec<u32>,
        on_path: &mut [bool],
        settled: &mut [bool],
    ) -> Option<Vec<u32>> {
        if on_path[slot as usize] {
            let entry = path.iter().position(|step| *step == slot).unwrap_or(0);
            return Some(path[entry..].to_vec());
        }
        if settled[slot as usize] {
            return None;
        }
        on_path[slot as usize] = true;
        path.push(slot);
        for held in self.pins.held_by(slot, self.cap).collect::<Vec<_>>() {
            if let Some(cycle) = self.walk_for_ring(held, path, on_path, settled) {
                return Some(cycle);
            }
        }
        path.pop();
        on_path[slot as usize] = false;
        settled[slot as usize] = true;
        None
    }

    /// Whether `holder` holds `held` in the pin relation.
    #[cfg(test)]
    fn holds(&self, holder: Handle, held: Handle) -> bool {
        self.pins.test(holder.slot(), held.slot())
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

    /// Build a value in the executing cell's own region.
    ///
    /// `build` receives the region's write surface at a brand it cannot widen, so the value it
    /// returns borrows region-derived or owned data and nothing else — an ambient `&'x` has no
    /// outlives relation to a universally quantified `'r`. The value's reach is therefore exactly
    /// the executing cell, and the mint's self rule makes storing it a hold on nothing.
    ///
    /// ```
    /// use cellgraph::{CellTable, DropFree, reattachable};
    /// struct Work;
    /// reattachable!(Work => String);
    /// struct Number;
    /// reattachable!(Number => &'r u32);
    /// impl DropFree for Number {}
    ///
    /// let mut table: CellTable<Work> = CellTable::new(2);
    /// let cell = table.create(None, None).unwrap();
    /// let read = table
    ///     .enter(cell, |context| {
    ///         let value = context.alloc::<Number>(|writer| writer.value(41));
    ///         assert!(value.reach().names(cell.slot()));
    ///         *context.read(&value).value()
    ///     })
    ///     .unwrap();
    /// assert_eq!(read, 41);
    /// ```
    ///
    /// A carrier cannot leave the step that built it: its home brand is the step's own, and
    /// `enter`'s result type is chosen outside the call, so it cannot name that brand.
    ///
    /// ```compile_fail
    /// use cellgraph::{CellTable, DropFree, Sealed, reattachable};
    /// struct Work;
    /// reattachable!(Work => String);
    /// struct Number;
    /// reattachable!(Number => &'r u32);
    /// impl DropFree for Number {}
    ///
    /// let mut table: CellTable<Work> = CellTable::new(2);
    /// let cell = table.create(None, None).unwrap();
    /// let escaped: Sealed<'_, Number> = table
    ///     .enter(cell, |context| context.alloc::<Number>(|writer| writer.value(41)))
    ///     .unwrap();
    /// ```
    pub fn alloc<T>(&mut self, build: impl for<'r> FnOnce(Writer<'r>) -> T::At<'r>) -> Sealed<'b, T>
    where
        T: Reattachable + DropFree,
    {
        let slot = self.handle.slot();
        let reach = Mask::empty(self.table.cap);
        self.mint_and_build(slot, reach, build)
    }

    /// Destination-homed placement: build a value **in `dest`'s region**, embedding the views of
    /// `operands`, and fold every operand's reach into `dest`'s hold set.
    ///
    /// This is the push shape of [design/cellgraph.md § Passing values between
    /// cells](../design/cellgraph.md#passing-values-between-cells): the producer builds straight
    /// into the consumer, the consumer's row takes the reach, and the producer can then die.
    /// Operands share one family `V` and arrive as carriers, never as values beside a mask.
    pub fn alloc_into<T, V>(
        &mut self,
        dest: Handle,
        operands: &[&Sealed<'b, V>],
        build: impl for<'r> FnOnce(Writer<'r>, &[V::At<'r>]) -> T::At<'r>,
    ) -> Result<Sealed<'b, T>, StaleHandle>
    where
        T: Reattachable + DropFree,
        V: Reattachable + DropFree,
        Erased<V>: Copy,
    {
        let dest_slot = self.table.live_slot(dest)?;
        let mut reach = Mask::empty(self.table.cap);
        for operand in operands {
            reach.union_with(operand.reach());
        }
        let erased: Vec<_> = operands.iter().map(|operand| operand.erased()).collect();
        Ok(self.mint_and_build(dest_slot, reach, move |writer| {
            // SAFETY: each operand is a carrier branded to this step, so its referents are region
            // storage in cells its reach names. No cell can die inside a step — `release` needs the
            // table, which `enter` holds exclusively for the whole call — and the mint below has
            // already folded that reach into the destination's hold set, so the storage outlives
            // both `'r` and the destination. `'r` is the region borrow, strictly inside the step
            // brand, and the `for<'r>` quantifier keeps a view from escaping the build.
            let views: Vec<_> = erased
                .into_iter()
                .map(|operand| unsafe { operand.reattach() })
                .collect();
            build(writer, &views)
        }))
    }

    /// Mint a bare hold on another live cell — the pull shape's first half: the executing cell
    /// takes a hold with no value crossing, so the held cell seals rather than reclaims when it
    /// dies, and this cell can read out of it later.
    pub fn hold(&mut self, other: Handle) -> Result<(), StaleHandle> {
        let other_slot = self.table.live_slot(other)?;
        let reach = Mask::single(self.table.cap, other_slot);
        self.table.mint(self.handle.slot(), &reach);
        Ok(())
    }

    /// Read a carrier out at the reading borrow. The door hangs on the context, so a value with
    /// reach is only ever live inside an `enter` scope.
    pub fn read<'s, T>(&'s self, carrier: &'s Sealed<'b, T>) -> Opened<'s, T>
    where
        T: Reattachable + DropFree,
        Erased<T>: Copy,
    {
        // SAFETY: `carrier` is branded to this step and its referents are region storage in the
        // cells its reach names; nothing dies inside a step, so they are live for all of `'s`,
        // which the `&'s self` borrow bounds inside the step brand. The re-anchor shortens.
        let value: T::At<'s> = unsafe { carrier.erased().reattach::<'s>() };
        Opened::new(value, carrier.reach())
    }

    /// The one path from a built value into a region: fold `reach` into the destination's hold
    /// set, write the value, and hand back the carrier that pairs it with its own reach — the
    /// destination's bit plus everything the operands reached.
    fn mint_and_build<T>(
        &mut self,
        dest_slot: u32,
        reach: Mask,
        build: impl for<'r> FnOnce(Writer<'r>) -> T::At<'r>,
    ) -> Sealed<'b, T>
    where
        T: Reattachable + DropFree,
    {
        // A bump releases its chunks whole and never walks a value, so a family with drop glue
        // would leak whatever it owns. `DropFree` declares the absence; this is the check.
        const { assert!(!std::mem::needs_drop::<T::At<'static>>()) };
        self.table.mint(dest_slot, &reach);
        let value = {
            let region = self.table.slots[dest_slot as usize]
                .region
                .get_or_insert_with(Region::new);
            Erased::<T>::erase(build(region.writer()))
        };
        let mut reach = reach;
        reach.add(dest_slot);
        Sealed::new(value, reach)
    }
}

impl<C: Reattachable> Drop for StepContext<'_, C> {
    fn drop(&mut self) {
        self.table.executing.clear(self.handle.slot());
    }
}
