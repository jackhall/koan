//! The cell table: a slab capped at construction, the two hold relations over its slots, the
//! executing flag, the per-cell regions, the sealed tier a still-reached cell falls into, and the
//! `create` / `enter` / `release` verbs. See
//! [design/cellgraph.md](../design/cellgraph.md) § Verbs and
//! [design/liveness-matrix.md](../design/liveness-matrix.md) § The model.

#[cfg(test)]
mod tests;

use crate::carrier::{Opened, Sealed};
use crate::handle::{Handle, StaleHandle};
use crate::mask::Mask;
use crate::matrix::{Bits, Matrix};
use crate::reattach::{DropFree, Erased, Reattachable};
use crate::region::{Region, Writer};
#[cfg(test)]
use crate::sealed::Memo;
use crate::sealed::{SealedId, SealedRecord, SealedSet, SealedTier};

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

/// Whether a dying cell's storage may fold into a unique live holder rather than mint a record of
/// its own — the embedder's per-release say over death-time absorption
/// ([liveness-matrix.md § Locality tactics](../design/liveness-matrix.md#locality-tactics)).
///
/// The choice is recorded on the slot at the release and consulted when the slot *disposes*, which
/// may be later: a dead cell a descendant's birth row still names waits in the slab first. Only
/// this merge is refusable — the two sealed-tier merges retain exactly what a plain seal retains,
/// so there is nothing to price.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Absorption {
    /// Fold into the unique holder when there is one. What an embedder with no price to weigh
    /// passes, and what a fresh slot starts at.
    IntoHolder,
    /// Seal instead, even where a merge was available. A priced choice declines when the holder's
    /// region would outlive the storage by too much.
    Refused,
}

/// What a hold on one sealed region keeps alive: its aggregate's transitive closure over the hold
/// graph, priced in chunk bytes.
///
/// The closure spans both tiers. A live cell a reached aggregate names is retention in waiting —
/// it will seal, or fold into its namer, when it dies — so its region is priced too, and the
/// closure only settles once it names no live cell.
#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Closure {
    /// Chunk bytes of every region the closure spans, the priced region's own included. A region
    /// two branches of the closure both reach is counted once.
    pub bytes: usize,
    /// Whether the closure names no live cell, so `bytes` can never change again.
    pub frozen: bool,
}

/// How much a live cell had absorbed at one instant, taken by [`CellTable::mark`] and read back by
/// [`CellTable::absorbed_since`].
///
/// Stamped with the cell it was taken against, so it cannot be read against another one.
#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Mark {
    handle: Handle,
    absorbed: usize,
}

/// Occupancy of both tiers at one instant — the input an embedder ramps a copy-versus-hold
/// threshold over. The substrate ships the numbers and no threshold: whether the ramp is linear or
/// a watermark step is the embedder's call
/// ([liveness-matrix.md § Bounding the two tiers](../design/liveness-matrix.md#bounding-the-two-tiers)).
#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Occupancy {
    /// Slab slots occupied — live cells and dead-but-resident ones alike.
    pub occupied: u32,
    /// The slab's fixed cap.
    pub cap: u32,
    /// Records in the sealed tier, which has no cap of its own.
    pub records: usize,
    /// Chunk bytes those records retain between them.
    pub retained_bytes: usize,
}

/// A node of the hold graph, as the test-only ring walk reports it. The graph spans both tiers: a
/// live cell holds cells and sealed regions, and a sealed region's frozen aggregate holds both in
/// turn.
#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum HoldNode {
    Cell(Handle),
    Sealed(SealedId),
}

/// The same node keyed by slab slot rather than handle, so a walk can visit it before deciding
/// which generation to report.
#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Node {
    Cell(u32),
    Sealed(SealedId),
}

/// The nodes one walk of the hold graph visited, split by tier. A walk with no cells is a frozen
/// closure: nothing in it will ever seal, merge, or retire again.
#[cfg(test)]
struct Reached {
    cells: Vec<u32>,
    records: Vec<SealedId>,
}

/// What a slab slot currently holds. `Dead` is the resident state: the embedder declared the
/// cell's death, but a descendant's birth row still names it, so the slot is not yet disposable.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SlotState {
    Free,
    Live,
    Dead,
}

/// A continuation at rest in its slot, with the reach of what it captured.
///
/// The reach is the cell's own durable mask, and the only one the seal transition rewrites: a
/// carrier is branded to the step that made it, so no *other* stored mask exists to go stale.
struct Stored<C: Reattachable> {
    value: Erased<C>,
    reach: Mask,
}

struct Slot<C: Reattachable> {
    generation: u32,
    state: SlotState,
    /// What the release of this cell said about death-time absorption. Read at the slot's
    /// disposal, which is why it rests here rather than travelling with the call.
    absorption: Absorption,
    continuation: Option<Stored<C>>,
    /// Minted at the cell's first allocation, so a cell that never allocates costs no chunk. Freed
    /// whole at reclamation, and detached unmoved at a seal — which is what makes a cell's death
    /// O(1) in its resident values either way.
    region: Option<Region>,
}

/// A capped slab of cells over the relations that decide when a slot may be reused, plus the
/// sealed tier that holds the regions whose slot came back while something still reached them.
///
/// `C` is the embedder's continuation family: a one-lifetime family the table stores erased, hands
/// back re-anchored under [`enter`](CellTable::enter), and never calls.
pub struct CellTable<C: Reattachable> {
    slots: Box<[Slot<C>]>,
    free: Vec<u32>,
    birth: Matrix,
    /// The pin relation's slab half: row M is the set of live cells whose region storage M's own
    /// resident values read. Written only by [`CellTable::mint`], which is the mint OR of
    /// [liveness-matrix.md § Reach as a hybrid mask](../design/liveness-matrix.md#reach-as-a-hybrid-mask).
    pins: Matrix,
    /// The pin relation's sparse half: per slot, the sealed regions that cell's values read.
    sealed_holds: Box<[SealedSet]>,
    /// The reverse naming index: per slot, the sealed regions whose frozen aggregate names it.
    /// Written at a seal and read by the next one, so step 2 of the transition finds its namers
    /// without scanning the tier.
    naming: Box<[SealedSet]>,
    sealed: SealedTier,
    executing: Bits,
    cap: u32,
    /// Units of maintenance the seal transitions of this table have performed — the quantity the
    /// bounded-transition test asserts is independent of a region's resident value count.
    #[cfg(test)]
    seal_work: u64,
    /// How many times each locality merge has fired — what the generated-interleaving test reads
    /// to check that its runs reach all three, rather than hoping they do.
    #[cfg(test)]
    merges: Merges,
}

/// A tally of the three locality merges.
#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct Merges {
    /// Death-time absorption of a uniquely held cell into its holder.
    pub(crate) into_cell: u64,
    /// Seal-time absorption of a count-1 sealed region into its sealing holder.
    pub(crate) at_seal: u64,
    /// A column-zero cell sealing into its single sealed namer.
    pub(crate) into_namer: u64,
}

impl<C: Reattachable> CellTable<C> {
    /// A slab of `cap` cells. The cap is fixed here and the table never grows past it: every slab
    /// relation is a fixed-width row over these slots. Each slab relation costs
    /// `cap × ceil(cap / 64)` words — a dense matrix is quadratic in the cap by construction — and
    /// the sealed tier grows in its own id space and takes no cap: retention is priced, not
    /// bounded.
    pub fn new(cap: u32) -> Self {
        let slots = (0..cap)
            .map(|_| Slot {
                generation: 0,
                state: SlotState::Free,
                absorption: Absorption::IntoHolder,
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
            sealed_holds: (0..cap).map(|_| SealedSet::new()).collect(),
            naming: (0..cap).map(|_| SealedSet::new()).collect(),
            sealed: SealedTier::new(),
            executing: Bits::new(cap),
            cap,
            #[cfg(test)]
            seal_work: 0,
            #[cfg(test)]
            merges: Merges::default(),
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
        let empty = Mask::empty(self.cap);
        let cell = &mut self.slots[slot as usize];
        cell.state = SlotState::Live;
        cell.continuation = continuation.map(|value| Stored {
            value: Erased::store(value),
            // A continuation handed in from outside is at `'static`: it captures nothing any
            // region owns, so it reaches nothing.
            reach: empty,
        });
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
    /// cannot name it — that is what makes handing region-borrowing values back re-anchored at it
    /// sound.
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
    /// The cell's own birth row releases wholesale — birth holds exist for execution, and the cell
    /// will not execute again. What happens to the slot then is the disposal's call: reclaimed if
    /// nothing reaches it, absorbed into a unique holder if `absorption` allows and one is there,
    /// sealed if something else reaches it, and left resident only while a descendant's birth row
    /// still names it.
    ///
    /// `absorption` is the embedder's say over that merge, recorded on the slot and applied
    /// whenever the slot actually disposes.
    pub fn release(&mut self, handle: Handle, absorption: Absorption) -> Result<(), ReleaseError> {
        let slot = self.live_slot(handle).map_err(ReleaseError::Stale)?;
        if self.executing.test(slot) {
            return Err(ReleaseError::Executing);
        }
        self.birth.clear_row(slot);
        let cell = &mut self.slots[slot as usize];
        cell.state = SlotState::Dead;
        cell.absorption = absorption;
        self.settle();
        Ok(())
    }

    /// Whether the table holds nothing at all: every slab slot free, and no sealed region left.
    ///
    /// The embedder's end-of-program alarm, and the only one the substrate ships. After the last
    /// release, a non-empty table means either a release was forgotten — a slot is still occupied
    /// — or a ring no merge dissolved survives in the tier; the crate's test-only ring walk names
    /// one.
    pub fn is_empty(&self) -> bool {
        self.occupied().next().is_none() && self.sealed.is_empty()
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

    /// The slots currently occupied — live cells and dead-but-resident ones alike.
    ///
    /// A dead-but-resident cell counts as a holder: its hold set releases when its slot goes, not
    /// when its death is declared, so its holds outlive it exactly as long as it does.
    fn occupied(&self) -> impl Iterator<Item = u32> + '_ {
        (0..self.cap).filter(|slot| self.slots[*slot as usize].state != SlotState::Free)
    }

    /// Whether a dead cell's slot may leave the slab now: nothing is executing in it, and no
    /// occupant's birth row still names it. Birth holds are the one relation that keeps a dead
    /// cell in place — a descendant that can still walk to it has not finished with it, and the
    /// relation has no sealed half for the walk to follow.
    fn disposable(&self, slot: u32) -> bool {
        !self.executing.test(slot) && !self.birth.held_by_any(self.occupied(), slot)
    }

    /// Take a disposable dead cell out of the slab, by the four exits it has: reclamation when
    /// nothing reaches its storage, absorption into a unique slab holder, a seal into a single
    /// sealed namer, and the plain seal everything else takes
    /// ([liveness-matrix.md § Locality tactics](../design/liveness-matrix.md#locality-tactics)).
    ///
    /// The two merges are the degenerate shapes the model is designed around — a chain of
    /// single-consumer producers — and each one is a record the tier never mints. A refused
    /// release falls through to the plain seal.
    fn dispose(&mut self, slot: u32) {
        let holders: Vec<u32> = self
            .occupied()
            .filter(|other| self.pins.test(*other, slot))
            .collect();
        let namers = std::mem::take(&mut self.naming[slot as usize]);
        let refused = self.slots[slot as usize].absorption == Absorption::Refused;
        match (holders.as_slice(), namers.len()) {
            ([], 0) => self.reclaim(slot),
            ([into], 0) if !refused => self.absorb_into_cell(slot, *into),
            ([], 1) => {
                let namer = namers.iter().next().expect("the set holds one id");
                self.fold_into_namer(slot, namer);
            }
            _ => self.seal(slot, &holders, &namers),
        }
    }

    /// Merge 1: fold a dying cell's storage and holds into the one slab occupant that holds it,
    /// minting no record at all.
    ///
    /// The target is any occupant, live or dead-resident: "live holder" in the design means the
    /// slab tier as opposed to the sealed one, and a dead-resident cell's row is still a maintained
    /// row. Its holds become the target's — the slab half through the standard mint, whose and-not
    /// is what makes a hold the dead cell had *on its own holder* land nowhere, dissolving a
    /// two-cell ring rather than sealing it.
    ///
    /// Reads of the absorbed values stay on the per-value-mask path: the chunks are now the
    /// target's own storage, which its stored mask already names, so no id enters the picture.
    fn absorb_into_cell(&mut self, dead: u32, into: u32) {
        let holds = self.take_holds(dead);
        // The target's hold on the dead cell is structural from here on: the storage is its own.
        self.pins.clear(into, dead);
        if let Some(stored) = &mut self.slots[into as usize].continuation {
            stored.reach.remove_slot(dead);
        }
        self.pins.mint(into, &holds);

        // The sparse half moves holder without changing count, except where the target already
        // held the same region: there the dead cell's hold simply vanishes.
        let mut dups = SealedSet::new();
        for id in holds.sealed().iter() {
            if !self.sealed_holds[into as usize].insert(id) {
                dups.insert(id);
            }
        }
        let storage = self.slots[dead as usize].region.take();
        Region::splice(&mut self.slots[into as usize].region, storage);

        self.vacate(dead, &dups);
        debug_assert!(
            !self.pins.test(into, into),
            "a cell absorbed a hold on itself"
        );
        #[cfg(test)]
        {
            self.seal_work += 1 + dups.len() as u64;
            self.merges.into_cell += 1;
        }
    }

    /// Merge 3: a cell nothing in the slab holds, named by exactly one sealed aggregate, folds
    /// into that record instead of minting one beside it.
    ///
    /// No stored mask needs rewriting: a stored mask naming a slot implies a pin hold on it, and
    /// this cell's slab column is empty by the precondition.
    fn fold_into_namer(&mut self, dead: u32, namer: SealedId) {
        let holds = self.take_holds(dead);
        let storage = self.slots[dead as usize].region.take();
        // The namer's hold on the dead cell is structural; the slots its row named trade the dead
        // cell's bit for the record's own name inside the fold.
        self.sealed
            .get_mut(namer)
            .expect("the namer came out of the reverse index")
            .aggregate
            .remove_slot(dead);
        let (_, dups) = self.fold_into_record(namer, holds, storage);

        self.vacate(dead, &dups);
        #[cfg(test)]
        {
            self.seal_work += 1 + dups.len() as u64;
            self.merges.into_namer += 1;
        }
        // The dead cell may have been the namer's last holder — a ring whose final cell just died.
        if self
            .sealed
            .get(namer)
            .is_some_and(|record| record.holders == 0)
        {
            self.reclaim_record(namer);
            return;
        }
        self.absorb_singletons(namer);
    }

    /// Fold a hold set and a region into an existing record: the shared body of merges 2 and 3.
    ///
    /// Returns the sealed ids that *transferred* (absent from the target's aggregate, so the hold
    /// changed owner without changing count) and the ones that *duplicated* (already there, so one
    /// hold on each vanishes). The caller releases the duplicates, since the borrow of the record
    /// has to end first, and then checks whether the target still has a holder: a source that held
    /// its own target contributes a self-hold, which has no representation and drops the count.
    fn fold_into_record(
        &mut self,
        target: SealedId,
        holds: Mask,
        storage: Option<Region>,
    ) -> (Vec<SealedId>, SealedSet) {
        let record = self
            .sealed
            .get_mut(target)
            .expect("the merge target is in the tier");
        let newly_named: Vec<u32> = holds
            .slab_slots()
            .filter(|slot| !record.aggregate.names(*slot))
            .collect();
        record.aggregate.union_slab_with(&holds);

        let mut transferred = Vec::new();
        let mut duplicated = SealedSet::new();
        for id in holds.sealed().iter() {
            if id == target {
                // The source held its own target. The hold becomes a self-hold, which no aggregate
                // can express, so it simply goes.
                debug_assert!(
                    record.holders >= 1,
                    "a record with no holder is still in the tier"
                );
                record.holders -= 1;
                continue;
            }
            if record.aggregate.add_sealed(id) {
                transferred.push(id);
            } else {
                duplicated.insert(id);
            }
        }
        debug_assert!(
            !record.aggregate.names_sealed(target),
            "a record's aggregate names itself"
        );
        self.sealed.splice_storage(target, storage);

        // The slots the fold newly reached register the target, so the next seal of one of them
        // finds it.
        for slot in newly_named {
            self.naming[slot as usize].insert(target);
        }
        (transferred, duplicated)
    }

    /// Merge 2: absorb every count-1 sealed region the record `target` holds, to a fixpoint.
    ///
    /// A count of 1 on a record the target names means the target *is* that holder, so the region
    /// is reachable through this record and nothing else — exactly the chain of single-consumer
    /// producers the tier would otherwise keep as a chain of records. The candidate set is a
    /// worklist rather than one pass: a fold transfers ids the target did not hold before, and
    /// drops a duplicate's count, either of which can newly qualify.
    fn absorb_singletons(&mut self, target: SealedId) {
        let Some(record) = self.sealed.get(target) else {
            return;
        };
        let mut pending: Vec<SealedId> = record.aggregate.sealed().iter().collect();
        while let Some(source) = pending.pop() {
            if source == target {
                continue;
            }
            match self.sealed.get(source) {
                Some(record) if record.holders == 1 => {}
                _ => continue,
            }
            let absorbed = self
                .sealed
                .remove(source)
                .expect("the record was just read");
            let named: Vec<u32> = absorbed.aggregate.slab_slots().collect();
            for slot in &named {
                self.naming[*slot as usize].remove(source);
            }
            self.sealed
                .get_mut(target)
                .expect("the target is in the tier")
                .aggregate
                .remove_sealed(source);
            #[cfg(test)]
            let sealed_width = absorbed.aggregate.sealed().len() as u64;
            let (transferred, duplicated) =
                self.fold_into_record(target, absorbed.aggregate, absorbed.storage);
            // Each duplicate had at least two holders — the target and the absorbed record — so
            // none of these counts reaches zero, and the target survives the call.
            self.release_sealed_holds(&duplicated);
            #[cfg(test)]
            {
                self.seal_work += 1 + named.len() as u64 + sealed_width;
                self.merges.at_seal += 1;
            }

            if self
                .sealed
                .get(target)
                .is_some_and(|record| record.holders == 0)
            {
                // The absorbed record held its own holder, and was its last: the ring dissolves.
                self.reclaim_record(target);
                return;
            }
            pending.extend(transferred);
            pending.extend(duplicated.iter());
        }
    }

    /// Dispose of every dead cell that has become disposable, repeating until none has — one
    /// death can free a chain of cells that were each held only by the next, and a record's
    /// reclamation can release the last hold on a slab slot in turn.
    fn settle(&mut self) {
        loop {
            let mut progressed = false;
            for slot in 0..self.cap {
                if self.slots[slot as usize].state == SlotState::Dead && self.disposable(slot) {
                    self.dispose(slot);
                    progressed = true;
                }
            }
            if !progressed {
                return;
            }
        }
    }

    /// Return a slot to the free list under a fresh generation, so every handle minted for the
    /// departing occupant is stale from here on.
    fn recycle(&mut self, slot: u32) {
        let cell = &mut self.slots[slot as usize];
        cell.state = SlotState::Free;
        cell.absorption = Absorption::IntoHolder;
        cell.continuation = None;
        cell.region = None;
        cell.generation = cell.generation.wrapping_add(1);
        self.free.push(slot);
    }

    /// Reclaim a cell nothing reaches: its storage goes, and its hold set releases wholesale.
    ///
    /// Releasing is only ever wholesale — there is no mid-life, per-reason release — which is what
    /// makes the mint's bit-setting idempotence safe.
    fn reclaim(&mut self, slot: u32) {
        let released = std::mem::take(&mut self.sealed_holds[slot as usize]);
        self.vacate(slot, &released);
    }

    /// Freeze a dying cell's hold set, both halves: the slab row copied, the sparse half taken off
    /// the slot so the ids it names change holder without changing count.
    fn take_holds(&mut self, slot: u32) -> Mask {
        Mask::from_parts(
            self.pins.row(slot).to_owned(),
            std::mem::take(&mut self.sealed_holds[slot as usize]),
        )
    }

    /// The tail every exit from the slab shares: the row clears, the slot recycles under a fresh
    /// generation, and one hold on each of `released` drops.
    fn vacate(&mut self, slot: u32, released: &SealedSet) {
        self.pins.clear_row(slot);
        self.recycle(slot);
        self.release_sealed_holds(released);
    }

    /// The seal transition: convert every representation of the dying cell from slab bit to sealed
    /// id, detach its storage, and recycle its slot
    /// ([liveness-matrix.md § The seal transition](../design/liveness-matrix.md#the-seal-transition)).
    ///
    /// The work is bounded by `holders`, `namers`, and the aggregate's width — never by what the
    /// region stores. Nothing here reads a region byte: monotone holds make the frozen row
    /// exactly the union of every reach ever minted in, so the aggregate is a word copy.
    fn seal(&mut self, slot: u32, holders: &[u32], namers: &SealedSet) {
        let id = self.sealed.mint_id();
        // The cell's hold set, both halves, frozen rather than cleared. Its sealed half moves from
        // the cell to the record, so the ids it names change holder without changing count.
        let aggregate = self.take_holds(slot);
        let storage = self.slots[slot as usize].region.take();
        let count = (holders.len() + namers.len()) as u32;

        // 1. Holders convert: the slab bit becomes the id, in the hold set and in the one stored
        //    mask a cell owns.
        for holder in holders {
            self.pins.clear(*holder, slot);
            self.sealed_holds[*holder as usize].insert(id);
            if let Some(stored) = &mut self.slots[*holder as usize].continuation {
                stored.reach.replace_slot(slot, id);
            }
        }
        // 2. Frozen aggregates convert, located through the reverse naming index.
        for namer in namers.iter() {
            if let Some(record) = self.sealed.get_mut(namer) {
                record.aggregate.replace_slot(slot, id);
            }
        }
        // 3. The new record registers under every slab bit it names, so the next seal of one of
        //    those slots finds it.
        let registered: Vec<u32> = aggregate.slab_slots().collect();
        for named in &registered {
            self.naming[*named as usize].insert(id);
        }
        #[cfg(test)]
        {
            self.seal_work += (holders.len() + namers.len() + registered.len()) as u64;
        }

        self.sealed.insert(
            id,
            SealedRecord {
                aggregate,
                storage,
                holders: count,
                #[cfg(test)]
                peak_holders: count,
                #[cfg(test)]
                closure: std::cell::OnceCell::new(),
            },
        );
        // The dying cell's sealed half moved into the record above, so the exit releases nothing.
        self.vacate(slot, &SealedSet::new());
        // 4. Every count-1 region the new record holds folds into it: a chain of single-consumer
        //    producers collapses to the one record at its head rather than one record per link.
        self.absorb_singletons(id);
    }

    /// Drop one hold on each of `released`, reclaiming every record whose count reaches zero and
    /// cascading through the holds that record's own aggregate named.
    fn release_sealed_holds(&mut self, released: &SealedSet) {
        let mut pending: Vec<SealedId> = released.iter().collect();
        while let Some(id) = pending.pop() {
            let Some(record) = self.sealed.get_mut(id) else {
                continue;
            };
            debug_assert!(
                record.holders >= 1,
                "a record with no holder is still in the tier"
            );
            record.holders -= 1;
            if record.holders > 0 {
                continue;
            }
            pending.extend(self.retire_record(id).iter());
        }
    }

    /// Retire a record whose count has reached zero: out of the tier, out of the reverse naming
    /// index, and its storage dropped. Hands back the holds its aggregate named, which the caller
    /// releases in turn.
    fn retire_record(&mut self, id: SealedId) -> SealedSet {
        let Some(record) = self.sealed.remove(id) else {
            return SealedSet::new();
        };
        let named: Vec<u32> = record.aggregate.slab_slots().collect();
        for slot in named {
            self.naming[slot as usize].remove(id);
        }
        // The record's storage drops here: nothing reaches these chunks any more.
        record.aggregate.sealed().clone()
    }

    /// Reclaim a record nothing holds any more — the zero-count exit, reached directly when a
    /// merge dissolves the last hold on its own target rather than through a holder's release.
    fn reclaim_record(&mut self, id: SealedId) {
        let released = self.retire_record(id);
        self.release_sealed_holds(&released);
    }

    /// The mint: fold a value's reach into the hold set of the region that now stores it, minus
    /// that region's own bit. **The only write into the pin relation.** Private to the table, so
    /// every path that puts a value in a region passes through here.
    ///
    /// A sealed id already in the destination's set is not a second hold — a hold set names a
    /// region at most once — which is what keeps the count in step with the wholesale release.
    fn mint(&mut self, into: u32, reach: &Mask) {
        self.pins.mint(into, reach);
        for id in reach.sealed().iter() {
            if self.sealed_holds[into as usize].insert(id)
                && let Some(record) = self.sealed.get_mut(id)
            {
                record.holders += 1;
                #[cfg(test)]
                {
                    record.peak_holders = record.peak_holders.max(record.holders);
                }
            }
        }
    }

    /// The price queries. Read-only, and crate-private until the resident-carrier crossing verdict
    /// consumes them — the tests are their only caller today.
    ///
    /// Bytes the sealed region `id` still occupies, or `None` if nothing holds it any more.
    ///
    /// The slab is bounded by its cap; this tier is bounded only by what programs retain, so its
    /// occupancy is the number worth asking for
    /// ([liveness-matrix.md § Bounding the two tiers](../design/liveness-matrix.md#bounding-the-two-tiers)).
    #[cfg(test)]
    pub(crate) fn sealed_retained_bytes(&self, id: SealedId) -> Option<usize> {
        self.sealed.get(id).map(SealedRecord::retained_bytes)
    }

    /// The slice of each candidate's closure that no *other* candidate reaches — the marginal price
    /// of releasing one hold, with the part it shares with another candidate billed to neither.
    ///
    /// The walk spans both tiers, so a live cell a closure names is priced at its region and
    /// walked through in turn. An answer is [`frozen`](Closure::frozen) once no live cell is left
    /// in the whole closure, and a frozen answer is memoized on the record and reused forever —
    /// nothing inside a frozen closure can change, which is the argument the memo field carries.
    ///
    /// One answer per input position, `None` where the id is no longer in the tier; a repeated id
    /// gets the same answer at every position naming it. A single candidate's slice is its whole
    /// closure.
    ///
    /// Uniqueness is **relative to the candidate set**. A holder outside the set that also reaches
    /// a node is not discounted, so a candidate lying inside another candidate's closure is shared
    /// throughout and prices at zero — the honest marginal price of releasing both.
    #[cfg(test)]
    pub(crate) fn unique_closures(&self, candidates: &[SealedId]) -> Vec<Option<Closure>> {
        let mut seen = SealedSet::new();
        let mut walks: Vec<(SealedId, Reached)> = Vec::new();
        for id in candidates {
            if seen.insert(*id)
                && let Some(reached) = self.reached_from_record(*id)
            {
                walks.push((*id, reached));
            }
        }

        // How many candidates' closures each node lies in. A count of one is what makes it unique.
        let mut cell_count = vec![0u32; self.cap as usize];
        let mut record_count: std::collections::HashMap<SealedId, u32> =
            std::collections::HashMap::new();
        for (_, reached) in &walks {
            for slot in &reached.cells {
                cell_count[*slot as usize] += 1;
            }
            for id in &reached.records {
                *record_count.entry(*id).or_insert(0) += 1;
            }
        }

        let priced: Vec<(SealedId, Closure)> = walks
            .iter()
            .map(|(id, reached)| {
                let cells: usize = reached
                    .cells
                    .iter()
                    .filter(|slot| cell_count[**slot as usize] == 1)
                    .map(|slot| self.cell_bytes(*slot))
                    .sum();
                let records: usize = reached
                    .records
                    .iter()
                    .filter(|inner| record_count[*inner] == 1)
                    .map(|inner| self.record_bytes(*inner))
                    .sum();
                (
                    *id,
                    Closure {
                        bytes: cells + records,
                        // The whole closure's, not the slice's: a live cell anywhere in it can
                        // still redraw the partition.
                        frozen: reached.cells.is_empty(),
                    },
                )
            })
            .collect();

        candidates
            .iter()
            .map(|id| {
                priced
                    .iter()
                    .find(|(priced, _)| priced == id)
                    .map(|(_, closure)| *closure)
            })
            .collect()
    }

    /// Chunk bytes a live cell's region bundle occupies, absorbed bumps included. `0` for a cell
    /// that never allocated.
    #[cfg(test)]
    pub(crate) fn region_bytes(&self, handle: Handle) -> Result<usize, StaleHandle> {
        let slot = self.live_slot(handle)?;
        Ok(self.cell_bytes(slot))
    }

    /// Snapshot how much the cell has absorbed so far, to read a later total against.
    #[cfg(test)]
    pub(crate) fn mark(&self, handle: Handle) -> Result<Mark, StaleHandle> {
        let slot = self.live_slot(handle)?;
        Ok(Mark {
            handle,
            absorbed: self.absorbed_bytes(slot),
        })
    }

    /// Chunk bytes the marked cell has taken in from other regions since the mark — a loop cart's
    /// accretion, without a scan.
    ///
    /// This counts what death-time absorption merged in, which is the proxy the design names for a
    /// cart's dead bytes. It is not a count of bytes that are actually dead: an absorbed region may
    /// still hold the value the cart carries, and a value the cart allocated and then replaced is
    /// invisible here. `Err` once the marked cell has died.
    #[cfg(test)]
    pub(crate) fn absorbed_since(&self, mark: Mark) -> Result<usize, StaleHandle> {
        let slot = self.live_slot(mark.handle)?;
        // A live cell's bundle only ever takes storage in, so the counter never runs backwards.
        Ok(self.absorbed_bytes(slot) - mark.absorbed)
    }

    /// How full both tiers are right now. The slab is bounded by its cap and the sealed tier by
    /// nothing, so an embedder ramps its copy-versus-hold threshold on these two numbers together.
    #[cfg(test)]
    pub(crate) fn occupancy(&self) -> Occupancy {
        Occupancy {
            occupied: self.cap - self.free.len() as u32,
            cap: self.cap,
            records: self.sealed.len(),
            retained_bytes: self.sealed.retained_bytes(),
        }
    }

    /// Chunk bytes of the region in one slab slot, `0` where the slot never allocated.
    #[cfg(test)]
    fn cell_bytes(&self, slot: u32) -> usize {
        self.slots[slot as usize]
            .region
            .as_ref()
            .map_or(0, Region::allocated_bytes)
    }

    #[cfg(test)]
    fn absorbed_bytes(&self, slot: u32) -> usize {
        self.slots[slot as usize]
            .region
            .as_ref()
            .map_or(0, Region::absorbed_bytes)
    }

    /// Chunk bytes a record retains, `0` for an id no longer in the tier.
    #[cfg(test)]
    fn record_bytes(&self, id: SealedId) -> usize {
        self.sealed.get(id).map_or(0, SealedRecord::retained_bytes)
    }

    #[cfg(test)]
    fn bytes_of(&self, reached: &Reached) -> usize {
        reached
            .cells
            .iter()
            .map(|slot| self.cell_bytes(*slot))
            .sum::<usize>()
            + reached
                .records
                .iter()
                .map(|id| self.record_bytes(*id))
                .sum::<usize>()
    }

    /// Walk from a record and memoize the result when it comes back frozen. `None` for an id no
    /// longer in the tier.
    #[cfg(test)]
    fn reached_from_record(&self, id: SealedId) -> Option<Reached> {
        let record = self.sealed.get(id)?;
        if let Some(memo) = record.closure.get() {
            return Some(Reached {
                cells: Vec::new(),
                records: memo.records.clone(),
            });
        }
        let reached = self.reached_from(Node::Sealed(id), true);
        if reached.cells.is_empty() {
            let _ = record.closure.set(Memo {
                records: reached.records.clone(),
                bytes: self.bytes_of(&reached),
            });
        }
        Some(reached)
    }

    /// Every node of the hold graph reachable from `start`, `start` itself included, over both
    /// tiers. A ring terminates on the seen sets rather than looping.
    ///
    /// A record that already carries a memo *is* its own frozen closure, so with `use_memos` the
    /// walk folds the memo's record set in instead of descending. The set is merged, never summed:
    /// two branches of one closure may share a sub-tier, and adding two memoized totals would bill
    /// the shared part twice. `use_memos` is false only where a test recomputes a memo from
    /// scratch to check it against what was recorded.
    #[cfg(test)]
    fn reached_from(&self, start: Node, use_memos: bool) -> Reached {
        let mut seen_cells = Bits::new(self.cap);
        let mut seen_records = SealedSet::new();
        let mut reached = Reached {
            cells: Vec::new(),
            records: Vec::new(),
        };
        let mut stack = vec![start];
        while let Some(node) = stack.pop() {
            match node {
                Node::Cell(slot) => {
                    if seen_cells.test(slot) {
                        continue;
                    }
                    seen_cells.set(slot);
                    reached.cells.push(slot);
                    stack.extend(self.holds_of(node));
                }
                Node::Sealed(id) => {
                    if !seen_records.insert(id) {
                        continue;
                    }
                    reached.records.push(id);
                    let memo = use_memos
                        .then(|| self.sealed.get(id).and_then(|record| record.closure.get()))
                        .flatten();
                    match memo {
                        Some(memo) => {
                            for inner in &memo.records {
                                if seen_records.insert(*inner) {
                                    reached.records.push(*inner);
                                }
                            }
                        }
                        None => stack.extend(self.holds_of(node)),
                    }
                }
            }
        }
        reached
    }

    /// Walk the hold graph from `start` and report a cycle if one is reachable — the fail-safe
    /// diagnostic for a ring, which keeps everything on it alive forever rather than dangling.
    ///
    /// Test-only, and **not consulted on any mint or release path**: preventing rings is the
    /// embedder's crossing discipline, not a mint-time reachability check. The walk spans both
    /// tiers, since a ring among live cells becomes a ring among sealed records the moment they
    /// die.
    #[cfg(test)]
    pub(crate) fn debug_ring_from(&self, start: Handle) -> Option<Vec<HoldNode>> {
        let mut path = Vec::new();
        let mut settled = std::collections::HashSet::new();
        self.walk_for_ring(Node::Cell(start.slot()), &mut path, &mut settled)
            .map(|cycle| cycle.into_iter().map(|node| self.name(node)).collect())
    }

    /// The same walk from a sealed region, for a ring that outlived the cells it started among:
    /// every hold on a sealed region is an id, and a stale handle can no longer reach it.
    #[cfg(test)]
    pub(crate) fn debug_ring_from_sealed(&self, start: SealedId) -> Option<Vec<HoldNode>> {
        let mut path = Vec::new();
        let mut settled = std::collections::HashSet::new();
        self.walk_for_ring(Node::Sealed(start), &mut path, &mut settled)
            .map(|cycle| cycle.into_iter().map(|node| self.name(node)).collect())
    }

    #[cfg(test)]
    fn name(&self, node: Node) -> HoldNode {
        match node {
            Node::Cell(slot) => {
                HoldNode::Cell(Handle::new(slot, self.slots[slot as usize].generation))
            }
            Node::Sealed(id) => HoldNode::Sealed(id),
        }
    }

    /// What one node of the hold graph holds: for a cell, its two hold-set halves; for a sealed
    /// region, the two halves of its frozen aggregate.
    #[cfg(test)]
    fn holds_of(&self, node: Node) -> Vec<Node> {
        match node {
            Node::Cell(slot) => self
                .pins
                .held_by(slot)
                .map(Node::Cell)
                .chain(self.sealed_holds[slot as usize].iter().map(Node::Sealed))
                .collect(),
            Node::Sealed(id) => match self.sealed.get(id) {
                Some(record) => record
                    .aggregate
                    .slab_slots()
                    .map(Node::Cell)
                    .chain(record.aggregate.sealed().iter().map(Node::Sealed))
                    .collect(),
                None => Vec::new(),
            },
        }
    }

    #[cfg(test)]
    fn walk_for_ring(
        &self,
        node: Node,
        path: &mut Vec<Node>,
        settled: &mut std::collections::HashSet<Node>,
    ) -> Option<Vec<Node>> {
        if let Some(entry) = path.iter().position(|step| *step == node) {
            return Some(path[entry..].to_vec());
        }
        if settled.contains(&node) {
            return None;
        }
        path.push(node);
        for next in self.holds_of(node) {
            if let Some(cycle) = self.walk_for_ring(next, path, settled) {
                return Some(cycle);
            }
        }
        path.pop();
        settled.insert(node);
        None
    }

    /// Whether `holder` holds `held` in the pin relation.
    #[cfg(test)]
    fn holds(&self, holder: Handle, held: Handle) -> bool {
        self.pins.test(holder.slot(), held.slot())
    }
}

/// The view of the table a step gets: its own cell's continuation slot, the region doors, and its
/// own identity.
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

    /// Take the cell's continuation, re-anchored at the step brand.
    ///
    /// **This is the sealed tier's accessor.** A continuation captured over values in cells that
    /// have since sealed comes back reading storage those records still retain. The door hangs on
    /// the step context and nowhere else, so a value reaching sealed storage is only ever live
    /// inside an `enter` scope.
    ///
    /// The slot is left empty: a continuation is one-shot, and a step that wants the cell entered
    /// again stores a successor.
    ///
    /// There is no table-level twin, so a value reaching sealed storage cannot be read from
    /// outside a step:
    ///
    /// ```compile_fail
    /// use cellgraph::{CellTable, reattachable};
    /// struct Work;
    /// reattachable!(Work => String);
    ///
    /// let mut table: CellTable<Work> = CellTable::new(2);
    /// let cell = table.create(None, None).unwrap();
    /// let _ = table.continuation(cell);
    /// ```
    pub fn continuation(&mut self) -> Option<Opened<'b, C>> {
        let stored = self.table.slots[self.handle.slot() as usize]
            .continuation
            .take()?;
        // SAFETY: the value's referents are region storage in the cells and sealed regions its
        // stored reach names, and that reach was minted into this cell's hold set when it was
        // stored — so every one of them is either a live cell or a held sealed record, whose
        // chunks are pointer-stable and detached unmoved. The cell is live for all of `'b` (it is
        // the one executing), so its holds are too. `'b` is the enclosing `enter`'s table borrow,
        // unnameable by the step's return type, so nothing anchored at it escapes.
        let value = unsafe { stored.value.reattach::<'b>() };
        Some(Opened::new(value))
    }

    /// Store a continuation that captures nothing any region owns, so it reaches nothing.
    pub fn store_successor(&mut self, continuation: C::At<'static>) {
        let reach = Mask::empty(self.table.cap);
        self.table.slots[self.handle.slot() as usize].continuation = Some(Stored {
            value: Erased::store(continuation),
            reach,
        });
    }

    /// Store a continuation built over carriers, so it may capture values living in regions.
    ///
    /// The captures' reach is minted into this cell's hold set before the continuation rests in
    /// its slot: a cell holds what its own continuation reads, which is what keeps those regions
    /// alive across the gap between this step and the next, and what makes the seal transition's
    /// rewrite of this mask the only rewrite the transition owes.
    ///
    /// `build` also receives this cell's own write surface, since a continuation that captures
    /// anything usually needs somewhere to put the captures' spine; the self rule makes the
    /// resulting self-reach a hold on nothing.
    pub fn store_successor_capturing<V>(
        &mut self,
        captures: &[&Sealed<'b, V>],
        build: impl for<'r> FnOnce(Writer<'r>, &[V::At<'r>]) -> C::At<'r>,
    ) where
        V: Reattachable + DropFree,
        Erased<V>: Copy,
    {
        let slot = self.handle.slot();
        let mut reach = Mask::empty(self.table.cap);
        for capture in captures {
            reach.union_with(capture.reach());
        }
        let erased: Vec<_> = captures.iter().map(|capture| capture.erased()).collect();
        self.table.mint(slot, &reach);
        let value = {
            let region = self.table.slots[slot as usize]
                .region
                .get_or_insert_with(Region::new);
            // SAFETY: each capture is a carrier branded to this step, so its referents are region
            // storage in the regions its reach names. The mint above has folded that reach into
            // this cell's hold set, so none of it can go away while the cell holds it; the views
            // live only for the `build` call, and `for<'r>` keeps one from escaping it.
            let views: Vec<_> = erased
                .into_iter()
                .map(|capture| unsafe { capture.reattach() })
                .collect();
            Erased::<C>::erase(build(region.writer(), &views))
        };
        reach.add(slot);
        self.table.slots[slot as usize].continuation = Some(Stored { value, reach });
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
            // storage in the regions its reach names. No cell can die inside a step — `release`
            // needs the table, which `enter` holds exclusively for the whole call — and the mint
            // below has already folded that reach into the destination's hold set, so the storage
            // outlives both `'r` and the destination. `'r` is the region borrow, strictly inside
            // the step brand, and the `for<'r>` quantifier keeps a view from escaping the build.
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
        // regions its reach names; nothing dies inside a step, so they are live for all of `'s`,
        // which the `&'s self` borrow bounds inside the step brand. The re-anchor shortens.
        let value: T::At<'s> = unsafe { carrier.erased().reattach::<'s>() };
        Opened::new(value)
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
