//! The cell table: a slab capped at construction, the two hold relations over its slots, the
//! executing flag, the per-cell regions and resident tables, the sealed tier a still-reached cell
//! falls into, the relocation map that forwards a resident through a merge, and the
//! `create` / `enter` / `release` verbs. The embedder's crossing verdict is taken here too, at
//! construction, and consulted once per operand of every placement. See
//! [design/cellgraph.md](../design/cellgraph.md) § Verbs and § The crossing verdict, and
//! [design/liveness-matrix.md](../design/liveness-matrix.md) § The model.

#[cfg(test)]
mod tests;

use crate::carrier::{Opened, Sealed};
use crate::handle::{Handle, StaleHandle};
use crate::mask::Mask;
use crate::matrix::{Bits, Matrix};
use crate::reattach::{DropFree, Erased, Reattachable};
use crate::region::{Region, Writer};
use crate::resident::{Resident, ResidentKey, Residents};
use crate::sealed::{Memo, SealedId, SealedRecord, SealedSet, SealedTier};

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

/// Refusals from [`StepContext::redeem`]. Never a panic: an at-rest carrier outlives the steps
/// around it, so meeting one whose home has moved on is ordinary, not a bug.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RedeemError {
    /// The storage the value names is gone — its home cell reclaimed, or the record it sealed into
    /// retired. Nothing could have read it, so nothing was lost by refusing.
    Gone,
    /// The storage is alive, but this cell has no claim on it: it is not the home, its pin row and
    /// birth row do not name the home, and it does not hold the record the home sealed into. A
    /// hold reached only transitively does not entitle — the entitling relations are the two that
    /// keep the home in the slab with its storage intact.
    Unheld,
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

/// What the embedder decides about one operand of a placement: pin the storage it reaches into the
/// destination, or copy it in.
///
/// A pin is cheap now and retentive later — the destination's row takes the operand's whole reach,
/// so nothing it names can be reclaimed while the destination lives. A copy is the reverse, and it
/// **severs**: a copied operand's view comes back at a brand unrelated to the destination's region,
/// so nothing borrowed through it can be embedded and the only copy that typechecks is a deep one
/// into the destination's own writer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict {
    Pin,
    Copy,
}

/// Both prices of one operand crossing into one destination, and the occupancy the choice plays
/// out against — the whole input of the crossing verdict, and **the only place the substrate's
/// prices appear in a signature**
/// ([liveness-matrix.md § Bounding the two tiers](../design/liveness-matrix.md#bounding-the-two-tiers)).
///
/// The substrate ships the numbers and no threshold. Whether the ramp is linear, a watermark step,
/// or a flat "always pin" is the embedder's call, and it is the one closure a table is built with.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Crossing {
    /// Bytes a pin would **newly** keep alive: what the walk from the operand's reach visits that
    /// the destination, its pin row, and its sealed-hold set do not already name, across both
    /// tiers. Zero for an operand the destination is already answerable for.
    pub pin_bytes: usize,
    /// What a copy costs, as the embedder passed it beside the operand. The substrate cannot know
    /// it: only the embedder knows how deep the value is.
    pub copy_bytes: usize,
    /// Slab slots occupied — live cells and dead-but-resident ones alike.
    pub occupied: u32,
    /// The slab's fixed cap.
    pub cap: u32,
    /// Records in the sealed tier, which has no cap of its own.
    pub records: usize,
    /// Chunk bytes those records retain between them.
    pub retained_bytes: usize,
    /// Chunk bytes the destination's own region bundle occupies. A loop's storage cell accretes
    /// only the values built into it, so this figure is the accretion signal a consolidation copy
    /// is decided on.
    pub dest_bytes: usize,
}

/// One operand of a placement: the carrier, and what the embedder says copying it would cost.
///
/// The two halves of the price meet here and nowhere else — the substrate walks the reach, the
/// embedder knows the depth — and neither is representable apart from the other.
pub struct Operand<'a, 'b, V: Reattachable + DropFree> {
    pub carrier: &'a Sealed<'b, V>,
    pub copy_bytes: usize,
}

/// An operand as the build closure receives it: at the region brand when the verdict pinned it, at
/// an unrelated brand when the verdict copied it.
///
/// The build closure is quantified over both brands, so a `Copied` view has no outlives relation to
/// the destination's region and embedding one is a compile error. A shallow copy is therefore
/// unrepresentable, and the only copy that typechecks is a deep one through the writer.
///
/// Embedding the pinned view is what a pin buys, and it compiles:
///
/// ```
/// use cellgraph::{CellTable, Crossed, DropFree, Operand, Verdict, reattachable};
/// struct Work;
/// reattachable!(Work => String);
/// struct Number;
/// reattachable!(Number => &'r u32);
/// impl DropFree for Number {}
///
/// let mut table: CellTable<Work> = CellTable::new(2, |_| Verdict::Pin);
/// let cell = table.create(None, None).unwrap();
/// let other = table.create(None, None).unwrap();
/// let read = table
///     .enter(cell, |context| {
///         let value = context.alloc::<Number>(|writer| writer.value(41));
///         let placed = context
///             .alloc_into::<Number, Number>(
///                 other,
///                 &[Operand { carrier: &value, copy_bytes: usize::MAX }],
///                 |writer, views| match views[0] {
///                     // The borrow itself, stored in the destination's region.
///                     Crossed::Pinned(value) => value,
///                     // A severed view can only be read and written again.
///                     Crossed::Copied(value) => writer.value(*value),
///                 },
///             )
///             .unwrap();
///         *context.read(&placed).value()
///     })
///     .unwrap();
/// assert_eq!(read, 41);
/// ```
///
/// Handing a `Copied` view back as the built value does not:
///
/// ```compile_fail
/// use cellgraph::{CellTable, Crossed, DropFree, Operand, Verdict, reattachable};
/// struct Work;
/// reattachable!(Work => String);
/// struct Number;
/// reattachable!(Number => &'r u32);
/// impl DropFree for Number {}
///
/// let mut table: CellTable<Work> = CellTable::new(2, |_| Verdict::Copy);
/// let cell = table.create(None, None).unwrap();
/// let other = table.create(None, None).unwrap();
/// table
///     .enter(cell, |context| {
///         let value = context.alloc::<Number>(|writer| writer.value(41));
///         context
///             .alloc_into::<Number, Number>(
///                 other,
///                 &[Operand { carrier: &value, copy_bytes: 0 }],
///                 |_writer, views| match views[0] {
///                     Crossed::Pinned(value) => value,
///                     Crossed::Copied(value) => value,
///                 },
///             )
///             .unwrap();
///     })
///     .unwrap();
/// ```
pub enum Crossed<'r, 'v, V: Reattachable> {
    Pinned(V::At<'r>),
    Copied(V::At<'v>),
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
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Node {
    Cell(u32),
    Sealed(SealedId),
}

/// The nodes one walk of the hold graph visited, split by tier. A walk with no cells is a frozen
/// closure: nothing in it will ever seal, merge, or retire again.
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

/// Where a departed cell's residents ended up, so a key minted under its handle still finds the
/// mask that names its reach.
///
/// `Slab` is a merge into a live cell: the masks moved into that cell's table at `base`, re-homed.
/// `Record` is a seal or a fold: the masks are gone, and a redeemed value's reach is derived from
/// the record instead.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Location {
    Slab { slot: u32, base: u32 },
    Record(SealedId),
}

struct Slot<C: Reattachable> {
    generation: u32,
    state: SlotState,
    /// The slot this cell was created under, and `None` for a root. The birth matrix answers
    /// "is this cell an ancestor" in O(1) and the parent link answers "which cell is next up",
    /// which is the axis a disposal cascade walks: the slots one release can free are a prefix of
    /// this chain upward from the released cell.
    parent: Option<u32>,
    /// What the release of this cell said about death-time absorption. Read at the slot's
    /// disposal, which is why it rests here rather than travelling with the call.
    absorption: Absorption,
    /// The continuation at rest, erased. Its reach is one entry of `residents` — the continuation
    /// is a resident like any other, so the seal transition maintains one collection per cell.
    continuation: Option<Erased<C>>,
    /// Which entry of `residents` holds the continuation's reach, and `None` for a continuation
    /// that reaches nothing. A store repoints this at the entry its reach interns to rather than
    /// writing the entry it named before, so a cell that alternates between a few continuation
    /// shapes settles at one entry per shape.
    continuation_reach: Option<u32>,
    /// The reach of every value kept in this cell's region, interned on content — the one durable
    /// habitat of a mask on the slab side, and what the seal transition's step 1 rewrites. One
    /// entry per distinct reach is what bounds that rewrite.
    residents: Residents,
    /// The departed cells whose residents this one absorbed, each of them a key in the table's
    /// relocation map pointing here. Bounded by merges, never by values.
    lineage: Vec<Handle>,
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
    /// Where the residents of a cell that has left the slab went. A departed handle maps to the
    /// live cell whose table absorbed its masks, or to the record its storage sealed into; a cell
    /// with an empty resident table leaves no entry. Rewritten at every merge and dropped at the
    /// target's reclamation, so the map is bounded by merges rather than by values.
    relocated: std::collections::HashMap<Handle, Location>,
    /// The embedder's crossing verdict, taken at construction. There is no verdict-free
    /// constructor: a table that can place a value can price the placement. One indirect call per
    /// priced operand is nothing beside the walk that prices it, and keeping the closure here is
    /// what leaves every other signature free of the parameter.
    verdict: Box<dyn FnMut(Crossing) -> Verdict>,
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
    ///
    /// `verdict` is the embedder's crossing verdict, consulted once per operand of every placement
    /// over operands. It is taken here rather than at a door so that no other signature names a
    /// price.
    pub fn new(cap: u32, verdict: impl FnMut(Crossing) -> Verdict + 'static) -> Self {
        let slots = (0..cap)
            .map(|_| Slot {
                generation: 0,
                state: SlotState::Free,
                parent: None,
                absorption: Absorption::IntoHolder,
                continuation: None,
                continuation_reach: None,
                residents: Residents::default(),
                lineage: Vec::new(),
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
            relocated: std::collections::HashMap::new(),
            verdict: Box::new(verdict),
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
        let cell = &mut self.slots[slot as usize];
        cell.state = SlotState::Live;
        cell.parent = parent_slot;
        // A continuation handed in from outside is at `'static`: it captures nothing any region
        // owns, so it reaches nothing and takes no resident entry.
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
    /// cannot name it — that is what makes handing region-borrowing values back re-anchored at it
    /// sound.
    ///
    /// Re-entering the executing cell is not representable: the step never holds the table.
    ///
    /// ```compile_fail
    /// use cellgraph::{CellTable, Verdict, reattachable};
    /// struct Owned;
    /// reattachable!(Owned => String);
    ///
    /// let mut table: CellTable<Owned> = CellTable::new(4, |_| Verdict::Pin);
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
        self.dispose_chain(slot);
        Ok(())
    }

    /// Whether the table holds nothing at all: every slab slot free, and no sealed region left.
    ///
    /// The embedder's end-of-program alarm, and the only one the substrate ships. After the last
    /// release, a non-empty table means either a release was forgotten — a slot is still occupied
    /// — or a ring no merge dissolved survives in the tier; the crate's test-only ring walk names
    /// one.
    pub fn is_empty(&self) -> bool {
        // A slot is on the free list exactly when it is free, so a full list is an empty slab.
        let vacant = self.free.len() == self.cap as usize;
        #[cfg(test)]
        debug_assert_eq!(
            vacant,
            self.occupied().next().is_none(),
            "the free list disagrees with a scan across the slab"
        );
        vacant && self.sealed.is_empty()
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

    /// Whether a dead cell's slot may leave the slab now: no occupant's birth row still names it.
    /// Birth holds are the one relation that keeps a dead cell in place — a descendant that can
    /// still walk to it has not finished with it, and the relation has no sealed half for the walk
    /// to follow.
    ///
    /// Execution does not enter the question: `release` refuses an executing cell and `begin`
    /// refuses a dead one, so a dead cell is never executing.
    fn disposable(&self, slot: u32) -> bool {
        // A free slot's birth row is cleared before the slot is recycled, so no free row names
        // anything and the count across every row is the count across the occupied ones.
        let held = self.birth.holders(slot);
        #[cfg(test)]
        debug_assert_eq!(
            held > 0,
            self.birth.held_by_any(self.occupied(), slot),
            "the birth tally disagrees with a scan across the occupied rows"
        );
        debug_assert!(!self.executing.test(slot), "a dead cell is executing");
        held == 0
    }

    /// The handle of whatever occupies `slot` right now, at its current generation.
    fn occupant(&self, slot: u32) -> Handle {
        Handle::new(slot, self.slots[slot as usize].generation)
    }

    /// Where the residents a key names live now, or `None` when their storage is gone.
    ///
    /// A handle whose slot still holds it names that slot directly, at base zero. Otherwise the
    /// cell has left the slab, and the relocation map answers — or does not, which means the cell
    /// reclaimed or left an empty table behind.
    fn locate(&self, home: Handle) -> Option<Location> {
        let cell = &self.slots[home.slot() as usize];
        if cell.state != SlotState::Free && cell.generation == home.generation() {
            return Some(Location::Slab {
                slot: home.slot(),
                base: 0,
            });
        }
        self.relocated.get(&home).copied()
    }

    /// Every departed handle whose residents `slot` answers for, plus `slot`'s own occupant if it
    /// kept anything. Taken off the slot: the caller is moving them somewhere else.
    fn take_lineage(&mut self, slot: u32) -> Vec<Handle> {
        let mut lineage = std::mem::take(&mut self.slots[slot as usize].lineage);
        if !self.slots[slot as usize].residents.is_empty() {
            lineage.push(self.occupant(slot));
        }
        lineage
    }

    /// Point every handle of `lineage` at `target`, and record them on the target so its own
    /// retirement can drop them again.
    fn relocate_to_record(&mut self, lineage: Vec<Handle>, target: SealedId) {
        for handle in &lineage {
            self.relocated.insert(*handle, Location::Record(target));
        }
        self.sealed
            .get_mut(target)
            .expect("the relocation target is in the tier")
            .lineage
            .extend(lineage);
    }

    /// Drop every entry of `lineage` from the map — the storage those keys named is gone, so a
    /// redeem under one of them refuses rather than finding a stale answer.
    fn forget_lineage(&mut self, lineage: &[Handle]) {
        for handle in lineage {
            self.relocated.remove(handle);
        }
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
        let namers = std::mem::take(&mut self.naming[slot as usize]);
        // Nothing holds the storage, so no merge has anything to fold it into and the identity of
        // a holder is not wanted: the pin tally settles that without reading down the column.
        if self.pins.holders(slot) == 0 && namers.is_empty() {
            self.reclaim(slot);
            return;
        }
        let holders: Vec<u32> = self
            .occupied()
            .filter(|other| self.pins.test(*other, slot))
            .collect();
        #[cfg(test)]
        debug_assert_eq!(
            holders.len() as u32,
            self.pins.holders(slot),
            "the pin tally disagrees with a scan across the occupied rows"
        );
        let refused = self.slots[slot as usize].absorption == Absorption::Refused;
        match (holders.as_slice(), namers.len()) {
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
        self.migrate_residents(dead, into);
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

    /// Move a dying cell's resident masks into the cell absorbing it, re-homed: bit `dead` becomes
    /// bit `into` throughout, since that storage is the target's own bundle from here on.
    ///
    /// Every key minted under a handle the dead cell answered for is forwarded to the target's
    /// table at its new base, so a resident survives any number of merges. A dead cell with an
    /// empty table forwards nothing and leaves no entry behind. The moved block is appended
    /// without interning: its position at `base` is what forwards the keys minted under it.
    fn migrate_residents(&mut self, dead: u32, into: u32) {
        // The target's own residents lose the dead cell's bit: those chunks are its storage now.
        for mask in self.slots[into as usize].residents.iter_mut() {
            mask.remove_slot(dead);
        }
        let base = self.slots[into as usize].residents.len();
        let moved = self.slots[dead as usize].residents.take();
        let kept_any = !moved.is_empty();
        for mut mask in moved {
            mask.remove_slot(dead);
            mask.add(into);
            self.slots[into as usize].residents.append(mask);
        }

        let mut lineage = std::mem::take(&mut self.slots[dead as usize].lineage);
        for handle in &lineage {
            let Location::Slab { base: old, .. } = self
                .relocated
                .get(handle)
                .copied()
                .expect("a slot's lineage entry points at that slot")
            else {
                unreachable!("a slot's lineage entry points at a slab slot, not a record");
            };
            self.relocated.insert(
                *handle,
                Location::Slab {
                    slot: into,
                    base: base + old,
                },
            );
        }
        if kept_any {
            let departing = self.occupant(dead);
            self.relocated
                .insert(departing, Location::Slab { slot: into, base });
            lineage.push(departing);
        }
        self.slots[into as usize].lineage.extend(lineage);
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
        let lineage = self.take_lineage(dead);
        self.relocate_to_record(lineage, namer);

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
        let naming = &mut self.naming;
        let record = self
            .sealed
            .get_mut(target)
            .expect("the merge target is in the tier");
        // The slots the fold newly reaches register the target, so the next seal of one of them
        // finds it. Registering before the union is what makes "newly" a reading off the
        // aggregate: after it, every slot the fold names is one the aggregate names.
        for slot in holds
            .slab_slots()
            .filter(|slot| !record.aggregate.names(*slot))
        {
            naming[slot as usize].insert(target);
        }
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
            let mut absorbed = self
                .sealed
                .remove(source)
                .expect("the record was just read");
            let lineage = std::mem::take(&mut absorbed.lineage);
            self.relocate_to_record(lineage, target);
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

    /// Dispose of the just-released cell and then of every dead ancestor the release left with no
    /// birth holder, innermost first — the whole cascade one death can set off, walked rather than
    /// scanned for.
    ///
    /// The walk is complete because only `create` and `release` write the birth relation: no
    /// disposal changes any slot's birth-holder count, so the only slots this release can bring to
    /// zero are the ones its own row named, its ancestors. Each ancestor's row names everything
    /// the row below it names ([liveness-matrix.md § Two relations](../design/liveness-matrix.md#two-relations-two-structures)),
    /// so the dead ancestors this release zeroes are a prefix of the chain upward: a live
    /// ancestor, or one another branch's row still names, stops the walk, and everything above it
    /// is still held.
    fn dispose_chain(&mut self, released: u32) {
        let mut next = Some(released);
        while let Some(slot) = next {
            if self.slots[slot as usize].state != SlotState::Dead || !self.disposable(slot) {
                return;
            }
            // Read the link first: disposal recycles the slot, which clears it.
            next = self.slots[slot as usize].parent;
            self.dispose(slot);
        }
    }

    /// Return a slot to the free list under a fresh generation, so every handle minted for the
    /// departing occupant is stale from here on.
    fn recycle(&mut self, slot: u32) {
        let cell = &mut self.slots[slot as usize];
        cell.state = SlotState::Free;
        cell.parent = None;
        cell.absorption = Absorption::IntoHolder;
        cell.continuation = None;
        cell.continuation_reach = None;
        cell.residents = Residents::default();
        cell.lineage.clear();
        cell.region = None;
        cell.generation = cell.generation.wrapping_add(1);
        self.free.push(slot);
    }

    /// Reclaim a cell nothing reaches: its storage goes, and its hold set releases wholesale.
    ///
    /// Releasing is only ever wholesale — there is no mid-life, per-reason release — which is what
    /// makes the mint's bit-setting idempotence safe.
    fn reclaim(&mut self, slot: u32) {
        // Nothing reaches this cell's storage, so every mask its residents named dies with it and
        // the handles it answered for stop resolving. Its own handle was never in the map.
        let lineage = std::mem::take(&mut self.slots[slot as usize].lineage);
        self.forget_lineage(&lineage);
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

        // 1. Holders convert: the slab bit becomes the id, in the hold set and in every mask of
        //    the holder's resident table — the only durable habitat a mask has on the slab side.
        //    The scan is bounded by the holders' resident counts, never by what the region stores.
        for holder in holders {
            self.pins.clear(*holder, slot);
            self.sealed_holds[*holder as usize].insert(id);
            let residents = &mut self.slots[*holder as usize].residents;
            for mask in residents.iter_mut() {
                mask.replace_slot(slot, id);
            }
            #[cfg(test)]
            {
                self.seal_work += self.slots[*holder as usize].residents.len() as u64;
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
                closure: std::cell::OnceCell::new(),
                lineage: Vec::new(),
            },
        );
        // The cell's own resident masks are dead bytes from here on: the storage they named is in
        // the record, and a redeem under one of these keys derives its reach from the record's id.
        let lineage = self.take_lineage(slot);
        self.relocate_to_record(lineage, id);
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
        for slot in record.aggregate.slab_slots() {
            self.naming[slot as usize].remove(id);
        }
        self.forget_lineage(&record.lineage);
        // The record's storage drops here: nothing reaches these chunks any more.
        record.aggregate.into_sealed()
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
    fn cell_bytes(&self, slot: u32) -> usize {
        self.slots[slot as usize]
            .region
            .as_ref()
            .map_or(0, Region::allocated_bytes)
    }

    /// Chunk bytes a record retains, `0` for an id no longer in the tier.
    fn record_bytes(&self, id: SealedId) -> usize {
        self.sealed.get(id).map_or(0, SealedRecord::retained_bytes)
    }

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
            });
        }
        Some(reached)
    }

    /// Record `id`'s frozen closure if it has one and has not been asked before, so a later walk
    /// folds the memo in rather than descending. Written once and never cleared: a closure that has
    /// frozen is walked exactly once for the table's whole life.
    fn prime_memo(&self, id: SealedId) {
        let Some(record) = self.sealed.get(id) else {
            return;
        };
        if record.closure.get().is_some() {
            return;
        }
        // Priming wants the walk and not its result, so the record set moves into the memo rather
        // than being copied into it and the walk's own copy dropped.
        let reached = self.reached_from(Node::Sealed(id), true);
        if reached.cells.is_empty() {
            let _ = record.closure.set(Memo {
                records: reached.records,
            });
        }
    }

    /// Every node of the hold graph reachable from `start`, `start` itself included, over both
    /// tiers. A ring terminates on the seen sets rather than looping.
    ///
    /// A record that already carries a memo *is* its own frozen closure, so with `use_memos` the
    /// walk folds the memo's record set in instead of descending. The set is merged, never summed:
    /// two branches of one closure may share a sub-tier, and adding two memoized totals would bill
    /// the shared part twice. `use_memos` is false only where a test recomputes a memo from
    /// scratch to check it against what was recorded.
    fn reached_from(&self, start: Node, use_memos: bool) -> Reached {
        self.walk([start], Bits::new(self.cap), SealedSet::new(), use_memos)
    }

    /// The walk itself, from a set of seeds and over a pair of already-seen sets: a node either
    /// set names is neither reported nor descended through, which is how a marginal price prunes
    /// at what the destination already holds.
    fn walk(
        &self,
        seeds: impl IntoIterator<Item = Node>,
        mut seen_cells: Bits,
        mut seen_records: SealedSet,
        use_memos: bool,
    ) -> Reached {
        let mut reached = Reached {
            cells: Vec::new(),
            records: Vec::new(),
        };
        let mut stack: Vec<Node> = seeds.into_iter().collect();
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

    /// Bytes that pinning a value with reach `reach` into `dest` would **newly** keep alive.
    ///
    /// The walk starts from the reach with `dest` itself, everything `dest`'s pin row names, and
    /// every record `dest` holds already marked as seen, so what the destination is answerable for
    /// anyway is billed to nobody. An operand homed in the destination, or in a cell the
    /// destination holds directly, prices at zero.
    ///
    /// The pruning is at *direct* holds, not at the destination's whole closure: a node the
    /// destination reaches only through a directly held node is still billed. The figure therefore
    /// only ever over-bills, which biases the verdict toward copying and never toward a pin whose
    /// cost the embedder was not shown.
    fn pin_price(&self, dest: u32, reach: &Mask) -> usize {
        // Every seed already seen is a walk that reports nothing and a sum over nothing, so the
        // price is zero without building the seed list or the seen sets. Priming a record's memo
        // only fills a cache no reading depends on, so skipping it changes no answer either.
        let row = self.pins.row(dest);
        let covered = reach
            .slab_slots()
            .all(|slot| slot == dest || row.test(slot))
            && reach
                .sealed()
                .iter()
                .all(|id| self.sealed_holds[dest as usize].contains(id));
        if covered {
            return 0;
        }

        let mut seen_cells = self.pins.row(dest).to_owned();
        seen_cells.set(dest);
        let seen_records = self.sealed_holds[dest as usize].clone();
        for id in reach.sealed().iter() {
            self.prime_memo(id);
        }
        let seeds: Vec<Node> = reach
            .slab_slots()
            .map(Node::Cell)
            .chain(reach.sealed().iter().map(Node::Sealed))
            .collect();
        let reached = self.walk(seeds, seen_cells, seen_records, true);
        self.bytes_of(&reached)
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
    /// use cellgraph::{CellTable, Verdict, reattachable};
    /// struct Work;
    /// reattachable!(Work => String);
    ///
    /// let mut table: CellTable<Work> = CellTable::new(2, |_| Verdict::Pin);
    /// let cell = table.create(None, None).unwrap();
    /// let _ = table.continuation(cell);
    /// ```
    pub fn continuation(&mut self) -> Option<Opened<'b, C>> {
        let stored = self.table.slots[self.handle.slot() as usize]
            .continuation
            .take()?;
        // SAFETY: the value's referents are region storage in the cells and sealed regions its
        // stored reach names — the resident entry `continuation_reach` names, which the seal
        // transition maintains, and none at all when it names no entry — and that reach was minted
        // into this cell's hold set when it was stored, so every one of them is either a live cell
        // or a held sealed record, whose
        // chunks are pointer-stable and detached unmoved. The cell is live for all of `'b` (it is
        // the one executing), so its holds are too. `'b` is the enclosing `enter`'s table borrow,
        // unnameable by the step's return type, so nothing anchored at it escapes.
        let value = unsafe { stored.reattach::<'b>() };
        Some(Opened::new(value))
    }

    /// Store a continuation that captures nothing any region owns, so it reaches nothing.
    ///
    /// Reaching nothing takes no entry: the cell stops naming one rather than naming an entry that
    /// names nothing, so a cell whose continuations never capture keeps an empty table. Whatever
    /// entry an earlier store pointed at stays as it is — entries are content, and nothing rewrites
    /// one because the value that minted it moved on.
    pub fn store_successor(&mut self, continuation: C::At<'static>) {
        let cell = &mut self.table.slots[self.handle.slot() as usize];
        cell.continuation_reach = None;
        cell.continuation = Some(Erased::store(continuation));
    }

    /// Store a continuation built over carriers, so it may capture values living in regions.
    ///
    /// The captures' reach is minted into this cell's hold set before the continuation rests in
    /// its slot: a cell holds what its own continuation reads, which is what keeps those regions
    /// alive across the gap between this step and the next.
    ///
    /// That reach is kept exactly as [`keep`](Self::keep) keeps one — interned into this cell's
    /// resident table, with the continuation pointing at the entry it landed in. Storing over an
    /// earlier continuation writes no entry, so a table entry is content that only the seal
    /// transition's uniform rewrite ever changes.
    ///
    /// `build` also receives this cell's own write surface, since a continuation that captures
    /// anything usually needs somewhere to put the captures' spine; the self rule makes the
    /// resulting self-reach a hold on nothing.
    pub fn store_successor_capturing<V>(
        &mut self,
        captures: &[Operand<'_, 'b, V>],
        build: impl for<'r, 'v> FnOnce(Writer<'r>, &[Crossed<'r, 'v, V>]) -> C::At<'r>,
    ) where
        V: Reattachable + DropFree,
        Erased<V>: Copy,
    {
        let slot = self.handle.slot();
        let (mut reach, crossed) = self.cross(slot, captures);
        self.table.mint(slot, &reach);
        let value = {
            let region = self.table.slots[slot as usize]
                .region
                .get_or_insert_with(Region::new);
            // SAFETY: see `crossed_views`. The mint above has folded every pinned capture's reach
            // into this cell's hold set, so none of that storage can go away while the cell holds
            // it, and the views live only for the `build` call.
            let views = unsafe { crossed_views(crossed) };
            Erased::<C>::erase(build(region.writer(), &views))
        };
        reach.add(slot);
        let cell = &mut self.table.slots[slot as usize];
        cell.continuation_reach = Some(cell.residents.intern(reach));
        cell.continuation = Some(value);
    }

    /// Build a value in the executing cell's own region.
    ///
    /// `build` receives the region's write surface at a brand it cannot widen, so the value it
    /// returns borrows region-derived or owned data and nothing else — an ambient `&'x` has no
    /// outlives relation to a universally quantified `'r`. The value's reach is therefore exactly
    /// the executing cell, and the mint's self rule makes storing it a hold on nothing.
    ///
    /// ```
    /// use cellgraph::{CellTable, DropFree, Verdict, reattachable};
    /// struct Work;
    /// reattachable!(Work => String);
    /// struct Number;
    /// reattachable!(Number => &'r u32);
    /// impl DropFree for Number {}
    ///
    /// let mut table: CellTable<Work> = CellTable::new(2, |_| Verdict::Pin);
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
    /// use cellgraph::{CellTable, DropFree, Sealed, Verdict, reattachable};
    /// struct Work;
    /// reattachable!(Work => String);
    /// struct Number;
    /// reattachable!(Number => &'r u32);
    /// impl DropFree for Number {}
    ///
    /// let mut table: CellTable<Work> = CellTable::new(2, |_| Verdict::Pin);
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
        operands: &[Operand<'_, 'b, V>],
        build: impl for<'r, 'v> FnOnce(Writer<'r>, &[Crossed<'r, 'v, V>]) -> T::At<'r>,
    ) -> Result<Sealed<'b, T>, StaleHandle>
    where
        T: Reattachable + DropFree,
        V: Reattachable + DropFree,
        Erased<V>: Copy,
    {
        let dest_slot = self.table.live_slot(dest)?;
        let (reach, crossed) = self.cross(dest_slot, operands);
        Ok(self.mint_and_build(dest_slot, reach, move |writer| {
            // SAFETY: see `crossed_views`. `mint_and_build` has already folded every pinned
            // operand's reach into the destination's hold set before it calls this closure, so
            // that storage outlives both `'r` and the destination.
            let views = unsafe { crossed_views(crossed) };
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

    /// Put a carrier to rest: register its reach in its home cell's resident table and hand back
    /// the lifetime-free form an embedder may keep between steps.
    ///
    /// The value is neither read nor moved — it stays where the placement wrote it. What changes
    /// is where its reach lives: off the carrier, which dies with this step, and into the table,
    /// where the seal transition rewrites it as the cells it names seal. The
    /// [`Resident`](crate::Resident) that comes back names that entry and carries no mask of its
    /// own, so nothing pairs a value with a reach outside the table.
    pub fn keep<T>(&mut self, carrier: Sealed<'b, T>) -> Resident<T>
    where
        T: Reattachable + DropFree,
    {
        let (value, reach, home) = carrier.into_parts();
        let cell = &mut self.table.slots[home as usize];
        let index = cell.residents.intern(reach);
        let key = ResidentKey {
            home: Handle::new(home, cell.generation),
            index,
        };
        Resident::new(value, key)
    }

    /// Redeem an at-rest carrier into this step, or refuse.
    ///
    /// This is the door both crossing shapes of [design/cellgraph.md § Passing values between
    /// cells](../design/cellgraph.md#passing-values-between-cells) complete through: a value the
    /// producer built into the consumer comes back in the consumer's own later step, and a value
    /// the consumer held its producer for comes back after the producer sealed.
    ///
    /// The executing cell must be entitled: it is the home, or its pin row or birth row names the
    /// home — both keep the home in the slab with its storage intact — or the home sealed into a
    /// record this cell holds. A value redeemed out of a record comes back reaching that record's
    /// id alone, which covers: a hold on a record keeps its whole aggregate alive transitively.
    pub fn redeem<T>(&self, resident: Resident<T>) -> Result<Sealed<'b, T>, RedeemError>
    where
        T: Reattachable + DropFree,
    {
        let table = &*self.table;
        let executing = self.handle.slot();
        let key = resident.key();
        // Decided before the value is touched: a resident rests as bytes precisely so that a
        // refusal costs nothing, including when the storage those bytes name is gone.
        let (reach, home) = match table.locate(key.home) {
            None => return Err(RedeemError::Gone),
            Some(Location::Slab { slot, base }) => {
                let entitled = slot == executing
                    || table.pins.test(executing, slot)
                    || table.birth.test(executing, slot);
                if !entitled {
                    return Err(RedeemError::Unheld);
                }
                let reach = table.slots[slot as usize]
                    .residents
                    .get(base + key.index)
                    .expect("a relocated key names an entry of the table it was forwarded to")
                    .clone();
                (reach, slot)
            }
            Some(Location::Record(id)) => {
                if !table.sealed_holds[executing as usize].contains(id) {
                    return Err(RedeemError::Unheld);
                }
                // The record's id alone: a hold on it keeps its aggregate alive transitively, and
                // a mask naming no slab bit has nothing that can go stale under a later `keep`.
                (Mask::single_sealed(table.cap, id), executing)
            }
        };
        // SAFETY: the match above resolved the key's home to storage that is still there — a live
        // slab slot this cell is the home of, holds, or descends from, or a record it holds — so
        // the referents parked in those bytes are live for the whole step, which is the contract
        // `take` asks for.
        let value = unsafe { resident.take() };
        Ok(Sealed::new(value, reach, home))
    }

    /// Read a carrier out at the reading borrow. The door hangs on the context, so a value with
    /// reach is only ever live inside an `enter` scope.
    pub fn read<'s, T>(&'s self, carrier: &'s Sealed<'b, T>) -> Opened<'s, T>
    where
        T: Reattachable + DropFree,
        Erased<T>: Copy,
    {
        // SAFETY: `carrier` is branded to this step, so it was either built by a door of this step
        // — whose mint folded its reach into the destination's hold set — or redeemed by one,
        // which checked that the executing cell keeps the storage the reach names, in the slab or
        // in the tier. Either way its referents are region storage that is live for the whole
        // step: nothing dies inside one. So they are live for all of `'s`, which the `&'s self`
        // borrow bounds inside the step brand, and the re-anchor shortens.
        let value: T::At<'s> = unsafe { carrier.erased().reattach::<'s>() };
        Opened::new(value)
    }

    /// Price every operand against `dest`, put each price to the embedder's verdict, and split the
    /// operands by the answer: the pinned reaches unioned for the mint, and each operand's erased
    /// form beside its verdict for the views.
    ///
    /// **The one path every placement over operands takes**, so the destination-homed placement and
    /// the capturing successor store consult the same closure with the same numbers and hand the
    /// build the same shapes. Every operand is consulted, including one whose pin price is zero.
    fn cross<V>(
        &mut self,
        dest: u32,
        operands: &[Operand<'_, 'b, V>],
    ) -> (Mask, Vec<(Erased<V>, Verdict)>)
    where
        V: Reattachable + DropFree,
        Erased<V>: Copy,
    {
        let table = &mut *self.table;
        let mut reach = Mask::empty(table.cap);
        let mut crossed = Vec::with_capacity(operands.len());
        for operand in operands {
            let crossing = Crossing {
                pin_bytes: table.pin_price(dest, operand.carrier.reach()),
                copy_bytes: operand.copy_bytes,
                occupied: table.cap - table.free.len() as u32,
                cap: table.cap,
                records: table.sealed.len(),
                retained_bytes: table.sealed.retained_bytes(),
                dest_bytes: table.cell_bytes(dest),
            };
            let verdict = (table.verdict)(crossing);
            if verdict == Verdict::Pin {
                reach.union_with(operand.carrier.reach());
            }
            crossed.push((operand.carrier.erased(), verdict));
        }
        (reach, crossed)
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
        Sealed::new(value, reach, dest_slot)
    }
}

/// Re-anchor each crossed operand at the brand its verdict allows: the destination's region brand
/// for a pin, an unrelated one for a copy.
///
/// # Safety
///
/// Every operand is a carrier branded to the executing step, so its referents are region storage
/// that is live for the whole step — nothing dies inside one, since `release` needs the table and
/// `enter` holds it exclusively — and a pinned operand's reach has additionally been minted into
/// the destination's hold set before this runs. The views live only for the build call, and the
/// caller's `for<'r, 'v>` quantifier keeps one from escaping it.
unsafe fn crossed_views<'r, 'v, V: Reattachable>(
    crossed: Vec<(Erased<V>, Verdict)>,
) -> Vec<Crossed<'r, 'v, V>> {
    crossed
        .into_iter()
        .map(|(erased, verdict)| match verdict {
            // SAFETY: see the function contract.
            Verdict::Pin => Crossed::Pinned(unsafe { erased.reattach::<'r>() }),
            // SAFETY: see the function contract.
            Verdict::Copy => Crossed::Copied(unsafe { erased.reattach::<'v>() }),
        })
        .collect()
}

impl<C: Reattachable> Drop for StepContext<'_, C> {
    fn drop(&mut self) {
        self.table.executing.clear(self.handle.slot());
    }
}
