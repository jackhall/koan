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
use crate::matrix::{BitRow, Matrix};
use crate::reattach::{DropFree, Erased, Reattachable};
use crate::region::{Region, Writer};
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

/// A node of the hold graph, as the ring detector reports it. The graph spans both tiers: a live
/// cell holds cells and sealed regions, and a sealed region's frozen aggregate holds both in turn.
#[cfg(debug_assertions)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HoldNode {
    Cell(Handle),
    Sealed(SealedId),
}

/// The same node keyed by slab slot rather than handle, so a walk can visit it before deciding
/// which generation to report.
#[cfg(debug_assertions)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Node {
    Cell(u32),
    Sealed(SealedId),
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
    executing: BitRow,
    cap: u32,
    /// Units of maintenance the seal transitions of this table have performed — the quantity the
    /// bounded-transition test asserts is independent of a region's resident value count.
    #[cfg(test)]
    seal_work: u64,
}

impl<C: Reattachable> CellTable<C> {
    /// A slab of `cap` cells. The cap is fixed here and the table never grows past it: every slab
    /// relation is a fixed-width row over these slots. The sealed tier grows in its own id space
    /// and takes no cap — retention is priced, not bounded.
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
            sealed_holds: (0..cap).map(|_| SealedSet::new()).collect(),
            naming: (0..cap).map(|_| SealedSet::new()).collect(),
            sealed: SealedTier::new(),
            executing: BitRow::new(cap),
            cap,
            #[cfg(test)]
            seal_work: 0,
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
    /// will not execute again. What happens to the slot then is
    /// [`dispose`](CellTable::dispose)'s call: reclaimed if nothing reaches it, sealed if
    /// something does, and left resident only while a descendant's birth row still names it.
    pub fn release(&mut self, handle: Handle) -> Result<(), ReleaseError> {
        let slot = self.live_slot(handle).map_err(ReleaseError::Stale)?;
        if self.executing.test(slot) {
            return Err(ReleaseError::Executing);
        }
        self.birth.clear_row(slot);
        self.slots[slot as usize].state = SlotState::Dead;
        self.settle();
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

    /// Take a disposable dead cell out of the slab, by the only two exits it has: reclamation when
    /// nothing reaches its storage, and a seal when something does.
    fn dispose(&mut self, slot: u32) {
        let holders: Vec<u32> = self
            .occupied()
            .filter(|other| self.pins.test(*other, slot))
            .collect();
        let namers = std::mem::take(&mut self.naming[slot as usize]);
        if holders.is_empty() && namers.is_empty() {
            self.reclaim(slot);
        } else {
            self.seal(slot, &holders, &namers);
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
        self.pins.clear_row(slot);
        let released = std::mem::take(&mut self.sealed_holds[slot as usize]);
        self.recycle(slot);
        self.release_sealed_holds(&released);
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
        let aggregate = Mask::with_words(
            self.pins.row_words(slot),
            std::mem::take(&mut self.sealed_holds[slot as usize]),
        );
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
        let registered: Vec<u32> = aggregate.slab_slots(self.cap).collect();
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
            },
        );
        self.pins.clear_row(slot);
        self.recycle(slot);
    }

    /// Drop one hold on each of `released`, reclaiming every record whose count reaches zero and
    /// cascading through the holds that record's own aggregate named.
    fn release_sealed_holds(&mut self, released: &SealedSet) {
        let mut pending: Vec<SealedId> = released.iter().collect();
        while let Some(id) = pending.pop() {
            let Some(record) = self.sealed.get_mut(id) else {
                continue;
            };
            record.holders -= 1;
            if record.holders > 0 {
                continue;
            }
            let record = self.sealed.remove(id).expect("the record was just read");
            for slot in record.aggregate.slab_slots(self.cap) {
                self.naming[slot as usize].remove(id);
            }
            pending.extend(record.aggregate.sealed().iter());
            // The record's storage drops here: nothing reaches these chunks any more.
        }
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
            }
        }
    }

    /// Derive the reach a stored mask reads back at: every sealed region it names contributes its
    /// own id plus its frozen aggregate.
    ///
    /// Over-approximate and covering. The per-value masks inside a sealed region are dead bytes
    /// nothing may read, so the aggregate is what stands in for them — and a resident value's true
    /// reach is a subset of its region's holds by mint-time coverage.
    fn derive_reach(&self, stored: &Mask) -> Mask {
        let mut derived = Mask::with_words(stored.words(), SealedSet::new());
        for id in stored.sealed().iter() {
            derived.add_sealed(id);
            if let Some(record) = self.sealed.get(id) {
                derived.union_with(&record.aggregate);
            }
        }
        derived
    }

    /// Bytes the sealed region `id` still occupies, or `None` if nothing holds it any more.
    ///
    /// The slab is bounded by its cap; this tier is bounded only by what programs retain, so its
    /// occupancy is the number worth asking for
    /// ([liveness-matrix.md § Bounding the two tiers](../design/liveness-matrix.md#bounding-the-two-tiers)).
    pub fn sealed_retained_bytes(&self, id: SealedId) -> Option<usize> {
        self.sealed.get(id).map(SealedRecord::retained_bytes)
    }

    /// Walk the hold graph from `start` and report a cycle if one is reachable — the fail-safe
    /// diagnostic for a ring, which keeps everything on it alive forever rather than dangling.
    ///
    /// Debug builds only, and **not consulted on any mint or release path**: preventing rings is
    /// the embedder's crossing discipline, not a mint-time reachability check. The walk spans both
    /// tiers, since a ring among live cells becomes a ring among sealed records the moment they
    /// die.
    #[cfg(debug_assertions)]
    pub fn debug_ring_from(&self, start: Handle) -> Option<Vec<HoldNode>> {
        let mut path = Vec::new();
        let mut settled = std::collections::HashSet::new();
        self.walk_for_ring(Node::Cell(start.slot()), &mut path, &mut settled)
            .map(|cycle| cycle.into_iter().map(|node| self.name(node)).collect())
    }

    /// The same walk from a sealed region, for a ring that outlived the cells it started among:
    /// every hold on a sealed region is an id, and a stale handle can no longer reach it.
    #[cfg(debug_assertions)]
    pub fn debug_ring_from_sealed(&self, start: SealedId) -> Option<Vec<HoldNode>> {
        let mut path = Vec::new();
        let mut settled = std::collections::HashSet::new();
        self.walk_for_ring(Node::Sealed(start), &mut path, &mut settled)
            .map(|cycle| cycle.into_iter().map(|node| self.name(node)).collect())
    }

    #[cfg(debug_assertions)]
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
    #[cfg(debug_assertions)]
    fn holds_of(&self, node: Node) -> Vec<Node> {
        match node {
            Node::Cell(slot) => self
                .pins
                .held_by(slot, self.cap)
                .map(Node::Cell)
                .chain(self.sealed_holds[slot as usize].iter().map(Node::Sealed))
                .collect(),
            Node::Sealed(id) => match self.sealed.get(id) {
                Some(record) => record
                    .aggregate
                    .slab_slots(self.cap)
                    .map(Node::Cell)
                    .chain(record.aggregate.sealed().iter().map(Node::Sealed))
                    .collect(),
                None => Vec::new(),
            },
        }
    }

    #[cfg(debug_assertions)]
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

    /// Take the cell's continuation, re-anchored at the step brand and paired with the reach its
    /// captures read.
    ///
    /// **This is the sealed tier's accessor.** A continuation captured over values in cells that
    /// have since sealed comes back with those regions named by id, and the reach is derived — the
    /// id plus its frozen aggregate — rather than read off the dead per-value masks in the sealed
    /// storage. The door hangs on the step context and nowhere else, so a value reaching sealed
    /// storage is only ever live inside an `enter` scope.
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
        let reach = self.table.derive_reach(&stored.reach);
        // SAFETY: the value's referents are region storage in the cells and sealed regions its
        // stored reach names, and that reach was minted into this cell's hold set when it was
        // stored — so every one of them is either a live cell or a held sealed record, whose
        // chunks are pointer-stable and detached unmoved. The cell is live for all of `'b` (it is
        // the one executing), so its holds are too. `'b` is the enclosing `enter`'s table borrow,
        // unnameable by the step's return type, so nothing anchored at it escapes.
        let value = unsafe { stored.value.reattach::<'b>() };
        Some(Opened::new(value, reach))
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
        Opened::new(value, carrier.reach().clone())
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
