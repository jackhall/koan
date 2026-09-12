//! The cell graph: a slab capped at construction, the two hold relations over its slots, the
//! executing flag, the per-cell regions and reach tables, the sealed tier a still-reached cell
//! falls into, the relocation map that forwards a dormant carrier through a merge, and the
//! `create` / `enter` / `release` verbs. The embedder's crossing verdict is taken here too, at
//! construction, and consulted once per operand of every placement. See
//! [../README.md](../README.md) § Verbs and § The crossing verdict, and
//! [graph/README.md](graph/README.md) § The model.

#[cfg(test)]
mod tests;

use std::marker::PhantomData;

use smallvec::SmallVec;

use crate::carrier::{Active, CellHome, Ready};
use crate::dormant::{Dormant, DormantKey, ReachTable};
use crate::handle::{CellHandle, SlabHandle, Stale, TreeHandle};
use crate::matrix::{Bits, Matrix};
use crate::reach::GraphReach;
use crate::reattach::{DropFree, Erased, Reattachable};
use crate::region::{Region, Writer};
use crate::scratch::{Scratch, ScratchVec};
use crate::sealed::{ScratchSet, SealedCell, SealedId, SealedSet, SealedTier};
use crate::tree::{Ancestor, Ancestry, TreeForward, TreePool, TreeState};

/// Refusals from [`CellGraph::create`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CreateError {
    /// The slab is at its cap. What to do next is admission policy, and the embedder's.
    SlabFull,
    /// The named parent is not a live cell.
    StaleParent(Stale<SlabHandle>),
}

/// Refusals from [`CellGraph::enter`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EnterError {
    /// The named cell is not a live cell of either kind.
    Stale(Stale<CellHandle>),
    /// The cell is already executing; a cell is entered by one step at a time.
    AlreadyExecuting,
}

/// Refusals from [`CellGraph::release`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReleaseError {
    /// The named cell is not a live cell — a second release names a death already declared.
    Stale(Stale<SlabHandle>),
    /// The cell is executing. Death is declared from outside a step, never from within one.
    Executing,
}

/// Refusals from [`CellGraph::release_tree`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReleaseTreeError {
    /// The named cell is not a live tree cell — a second release names a death already declared.
    Stale(Stale<TreeHandle>),
    /// The cell is executing. Death is declared from outside a step, never from within one.
    Executing,
}

/// Refusals from [`StepContext::redeem`]. Never a panic: an at-rest carrier outlives the steps
/// around it, so meeting one whose home has moved on is ordinary, not a bug.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RedeemError {
    /// The storage the value names is gone — its home cell reclaimed, or the sealed cell it sealed
    /// into retired. Nothing could have read it, so nothing was lost by refusing.
    Gone,
    /// The storage is alive, but this cell has no claim on it: it is not the home, its pin row and
    /// birth row do not name the home, and it does not hold the sealed cell the home sealed into. A
    /// hold reached only transitively does not entitle — the entitling relations are the two that
    /// keep the home in the slab with its storage intact.
    Unheld,
}

/// Whether a dying cell's storage may fold into a unique live holder rather than mint a sealed cell
/// of its own — the embedder's per-release say over death-time absorption ([graph/README.md §
/// Locality tactics](graph/README.md#locality-tactics)).
///
/// The choice is recorded on the slot at the release and consulted when the slot *disposes*, which
/// may be later: a dead cell a descendant's birth row still names waits in the slab first.
///
/// **`Release` is in the name because three other things go by the word**, and none of them is
/// this one — none is refusable, and none is the embedder's to weigh:
///
/// - *Seal-time absorption* (`absorb_singletons`, `Merges::at_seal`) — a count-1 sealed cell
///   folding into the sealed cell sealing over it.
/// - *Fold into namer* (`fold_into_namer`, `Merges::into_namer`) — a column-zero cell sealing into
///   its single sealed namer.
/// - The *mechanism* all three run on: bumps spliced into a bundle (a cell region's absorbed list)
///   and the `TreeState::Absorbed` tombstone a tree cell leaves when its bytes move.
///
/// The two sealed-tier merges retain exactly what a plain seal retains, so there is nothing to
/// price and nothing to ask — which is why this one alone reaches the embedder.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReleaseAbsorption {
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
/// ([graph/README.md § Bounding the two tiers](graph/README.md#bounding-the-two-tiers)).
///
/// The substrate ships the numbers and no threshold. Whether the ramp is linear, a watermark step,
/// or a flat "always pin" is the embedder's call, and it is the one closure a graph is built with.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Prices {
    /// Bytes a pin would **newly** keep alive: what the walk from the operand's reach visits that
    /// the destination, its pin row, and its sealed-hold set do not already name, across both
    /// tiers. Zero for an operand the destination is already answerable for.
    pub pin_bytes: usize,
    /// What a copy costs, as the embedder passed it beside the operand. The substrate cannot know
    /// it: only the embedder knows how deep the value is.
    pub copy_bytes: usize,
    /// Slab slots occupied — live cells and dead-but-undisposed ones alike.
    pub occupied: u32,
    /// The slab's fixed cap.
    pub cap: u32,
    /// Cells in the sealed tier, which has no cap of its own.
    pub sealed_cells: usize,
    /// Chunk bytes those sealed cells retain between them.
    pub retained_bytes: usize,
    /// Chunk bytes the destination's own region bundle occupies. A loop's storage cell accretes
    /// only the values built into it, so this figure is the accretion signal a consolidation copy
    /// is decided on.
    pub destination_bytes: usize,
}

/// One operand of a placement: the carrier, and what the embedder says copying it would cost.
///
/// The two halves of the price meet here and nowhere else — the substrate walks the reach, the
/// embedder knows the depth — and neither is representable apart from the other.
pub struct Operand<'a, 'b, V: Reattachable + DropFree, const W: usize = 1> {
    pub carrier: &'a Ready<'b, V, W>,
    pub copy_bytes: usize,
}

/// An operand as the build closure receives it: at the destination's cell-region brand when the
/// verdict pinned it, at an unrelated brand when the verdict copied it.
///
/// The build closure is quantified over both brands, so a `Copied` view has no outlives relation to
/// the destination's region and embedding one is a compile error. A shallow copy is therefore
/// unrepresentable, and the only copy that typechecks is a deep one through the writer.
///
/// Embedding the pinned view is what a pin buys, and it compiles:
///
/// ```
/// use cellgraph::{CellGraph, CrossedOperand, DropFree, Operand, Verdict, reattachable};
/// struct Work;
/// reattachable!(Work => String);
/// struct Number;
/// reattachable!(Number => &'r u32);
/// impl DropFree for Number {}
///
/// let mut graph: CellGraph<Work> = CellGraph::new(2, |_| Verdict::Pin);
/// let cell = graph.create(None, None).unwrap();
/// let other = graph.create(None, None).unwrap();
/// let read = graph
///     .enter(cell, |context| {
///         let value = context.lift::<Number>(&context.writer().fill(1, |_| 41u32)[0]);
///         let placed = context
///             .alloc_into::<Number, Number>(
///                 other,
///                 &[Operand { carrier: &value, copy_bytes: usize::MAX }],
///                 |writer, views| match views[0] {
///                     // The borrow itself, stored in the destination's region.
///                     CrossedOperand::Pinned(value) => value,
///                     // A severed view can only be read and written again.
///                     CrossedOperand::Copied(value) => &writer.fill(1, |_| *value)[0],
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
/// use cellgraph::{CellGraph, CrossedOperand, DropFree, Operand, Verdict, reattachable};
/// struct Work;
/// reattachable!(Work => String);
/// struct Number;
/// reattachable!(Number => &'r u32);
/// impl DropFree for Number {}
///
/// let mut graph: CellGraph<Work> = CellGraph::new(2, |_| Verdict::Copy);
/// let cell = graph.create(None, None).unwrap();
/// let other = graph.create(None, None).unwrap();
/// graph
///     .enter(cell, |context| {
///         let value = context.lift::<Number>(&context.writer().fill(1, |_| 41u32)[0]);
///         context
///             .alloc_into::<Number, Number>(
///                 other,
///                 &[Operand { carrier: &value, copy_bytes: 0 }],
///                 |_writer, views| match views[0] {
///                     CrossedOperand::Pinned(value) => value,
///                     CrossedOperand::Copied(value) => value,
///                 },
///             )
///             .unwrap();
///     })
///     .unwrap();
/// ```
///
/// The views themselves die with the build call. They are handed over out of the graph's scratch
/// region, which the next verb's entry resets, and the `for<'r, 'v>` quantifier is what keeps one
/// from outliving the call that received it — a caller cannot name either brand, so it has nowhere
/// to put the slice:
///
/// ```compile_fail
/// use cellgraph::{CellGraph, CrossedOperand, DropFree, Operand, Verdict, reattachable};
/// struct Work;
/// reattachable!(Work => String);
/// struct Number;
/// reattachable!(Number => &'r u32);
/// impl DropFree for Number {}
///
/// let mut graph: CellGraph<Work> = CellGraph::new(2, |_| Verdict::Pin);
/// let cell = graph.create(None, None).unwrap();
/// let other = graph.create(None, None).unwrap();
/// let mut escaped: Option<&[CrossedOperand<'_, '_, Number>]> = None;
/// graph
///     .enter(cell, |context| {
///         let value = context.lift::<Number>(&context.writer().fill(1, |_| 41u32)[0]);
///         context
///             .alloc_into::<Number, Number>(
///                 other,
///                 &[Operand { carrier: &value, copy_bytes: usize::MAX }],
///                 |writer, views| {
///                     escaped = Some(views);
///                     &writer.fill(1, |_| 0)[0]
///                 },
///             )
///             .unwrap();
///     })
///     .unwrap();
/// ```
pub enum CrossedOperand<'r, 'v, V: Reattachable> {
    Pinned(V::At<'r>),
    Copied(V::At<'v>),
}

/// What a hold on one sealed region costs: the chunk bytes of everything it pins, which is
/// [`TransitivePins`] measured rather than listed. The pair is the split — that type is which
/// nodes, this one is what they come to.
///
/// The price spans both tiers. A live cell a pinned aggregate names is retention in waiting — it
/// will seal, or fold into its namer, when it dies — so its region is priced too, and the figure
/// only settles once nothing live is left in the closure.
#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct RetentionPrice {
    /// Chunk bytes of every region the closure spans, the priced region's own included. A region
    /// two branches of the closure both reach is counted once.
    pub bytes: usize,
    /// Whether the closure names no live cell, so `bytes` can never change again.
    pub frozen: bool,
}

/// Occupancy of both tiers at one instant — the input an embedder ramps a copy-versus-hold
/// threshold over. The substrate ships the numbers and no threshold: whether the ramp is linear or
/// a watermark step is the embedder's call
/// ([graph/README.md § Bounding the two tiers](graph/README.md#bounding-the-two-tiers)).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Occupancy {
    /// Slab slots occupied — live cells and dead-but-undisposed ones alike.
    pub occupied: u32,
    /// The slab's fixed cap.
    pub cap: u32,
    /// Cells in the sealed tier, which has no cap of its own.
    pub sealed_cells: usize,
    /// Chunk bytes those sealed cells retain between them.
    pub retained_bytes: usize,
}

/// A node of the hold graph, as the test-only ring walk reports it. The graph spans both tiers: a
/// live cell holds cells and sealed regions, and a sealed region's frozen aggregate holds both in
/// turn.
#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum HoldNode {
    Slab(SlabHandle),
    Sealed(SealedId),
}

/// The same node keyed by slab slot rather than handle, so a walk can visit it before deciding
/// which generation to report.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum GraphNode {
    Slab(u32),
    Sealed(SealedId),
}

/// Everything one node pins, transitively: the closure of the **pin relation** from a start node,
/// split by tier. A walk with no cells is frozen — nothing in it will ever seal, merge, or retire
/// again — and [`RetentionPrice`] is this same set weighed in bytes.
///
/// Pins and nothing else. The walk steps along `pins` rows and `sealed_holds`, and through a sealed
/// cell's aggregate; the birth relation is never traversed, so a cell a parent's row names but
/// nothing pins is not here.
///
/// Test-only: a walk reports each node as it visits it, and the two readings the crate takes —
/// a price and a memo — fold that stream rather than materialising it. The tests that check the
/// stream against a recorded set want the whole thing, and this is it.
#[cfg(test)]
struct TransitivePins {
    cells: Vec<u32>,
    sealed: Vec<SealedId>,
}

/// What a slab slot currently holds. `Dead` is the undisposed state: the embedder declared the
/// cell's death, but a descendant's birth row still names it, so the slot is not yet disposable.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SlabState {
    Free,
    Live,
    Dead,
}

/// Where a departed cell's dormant carriers ended up, so a key minted under its handle still finds
/// the mask that names its reach.
///
/// `Slab` is a merge into a live cell: the masks moved into that cell's reach table starting at
/// `first_index`, re-homed. `Sealed` is a seal or a fold: the masks are gone, and a redeemed
/// value's reach is derived from the sealed cell instead.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SlabForward {
    Slab { slot: u32, first_index: u32 },
    Sealed(SealedId),
}

/// Where one departed cell's dormant carriers went, plus the link to the next departed cell whose
/// dormant carriers the same target answers for.
///
/// The entry carries the generation rather than the whole handle: the list it sits in is indexed
/// by the handle's slot, so the generation is all that is left to tell two occupants of that slot
/// apart.
#[derive(Clone, Copy, Debug)]
struct Relocation {
    generation: u32,
    location: SlabForward,
    /// The next handle on the target's lineage chain, `None` at the chain's end.
    next: Option<SlabHandle>,
}

struct SlabCell<C: Reattachable, const W: usize> {
    generation: u32,
    state: SlabState,
    /// The slot this cell was created under, and `None` for a root. The birth matrix answers
    /// "is this cell an ancestor" in O(1) and the parent link answers "which cell is next up",
    /// which is the axis a disposal cascade walks: the slots one release can free are a prefix of
    /// this chain upward from the released cell.
    parent: Option<u32>,
    /// What the release of this cell said about death-time absorption. Read at the slot's
    /// disposal, which is why it rests here rather than travelling with the call.
    absorption: ReleaseAbsorption,
    /// The continuation at rest, erased. It carries no reach of its own: every reference it can
    /// capture is at the cell's `'cell` brand, which names storage the cell's hold set already
    /// covers — its own region, or a region a pinned crossing minted in.
    continuation: Option<Erased<C>>,
    /// The reach of every value kept in this cell's region, interned on content — the one durable
    /// habitat of a mask on the slab side, and what the seal transition's step 1 rewrites. One
    /// entry per distinct reach is what bounds that rewrite.
    reaches: ReachTable<W>,
    /// The head of the chain of departed cells whose dormant carriers this one absorbed, threaded
    /// through the relocation entries themselves. Bounded by merges, never by values, and one
    /// word rather than a vector: the links live where the entries already are.
    lineage: Option<SlabHandle>,
    /// Minted when the cell is first entered, since a step takes its writer at `enter`. An empty
    /// bump claims no chunk, so a cell that never writes still costs none. Freed whole at
    /// reclamation, and detached unmoved at a seal — which is what makes a cell's death O(1) in
    /// its dormant values either way.
    region: Option<Region>,
    /// Tree children whose chain tops out at this cell and that have not disposed — the birth
    /// tally's analogue for the pool. A released root waits dead-but-undisposed while any of them
    /// is still there, and the last one's disposal is what sets its own cascade off.
    tree_children: u32,
    /// The head of the list of tree tombstones whose bytes spliced into this cell's bundle. Travels
    /// onto the relocation entry when the cell leaves the slab, so a dormant carrier still keyed to
    /// one of them keeps resolving.
    tree_tombstones: Option<u32>,
}

impl<C: Reattachable, const W: usize> SlabCell<C, W> {
    /// A slot with no occupant, under the given generation.
    fn free(generation: u32) -> Self {
        SlabCell {
            generation,
            state: SlabState::Free,
            parent: None,
            absorption: ReleaseAbsorption::IntoHolder,
            continuation: None,
            reaches: ReachTable::default(),
            lineage: None,
            region: None,
            tree_children: 0,
            tree_tombstones: None,
        }
    }
}

/// One placement's destination, resolved: the cell whose region takes the bytes, and the slab slot
/// whose relations the placement writes.
///
/// **The one place the two kinds converge.** No relation names a tree cell, so a placement into one
/// mints into its root through the ordinary mint, and the reach the built value travels with is the
/// root's bit — which is why the pair travels together rather than one number standing for both.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Destination {
    /// The slab slot the placement mints into: the destination itself, or a tree destination's
    /// root.
    mint_slot: u32,
    /// The cell whose region bundle the value is written into.
    home: CellHome,
}

/// Where one operand's home stands relative to its destination — the classification
/// [`classify_crossing`](CellGraph::classify_crossing) takes before any price is asked for, and
/// the whole of the **ancestry rule** an operand homed in a tree cell is held to.
///
/// An operand homed in a slab cell or a sealed cell is always `Ordinary`: nothing about the tree
/// habitat touches it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Crossing {
    /// The destination dies before the operand's home, so a borrow embedded there stays valid with
    /// no promise: the ordinary price, and the ordinary verdict.
    Ordinary,
    /// The destination is an ancestor of the operand's home on its chain. The price is the splice
    /// price, and a `Pin` pledges the home and every intermediate to splice into it at death.
    Upward(Ancestor),
    /// The destination is neither on the home's chain nor under it, so nothing there may outlive
    /// the home while borrowing it. The verdict is **not consulted**: the crossing is a copy.
    Forced,
}

/// A capped slab of cells over the relations that decide when a slot may be reused, plus the
/// sealed tier that holds the regions whose slot came back while something still reached them.
///
/// `C` is the embedder's continuation family: a one-lifetime family the graph stores erased, hands
/// back re-anchored under [`enter`](CellGraph::enter), and never calls.
pub struct CellGraph<C: Reattachable, const W: usize = 1> {
    slots: Box<[SlabCell<C, W>]>,
    free: Vec<u32>,
    birth: Matrix<W>,
    /// The pin relation's slab half: row M is the set of live cells whose region storage M's own
    /// dormant values read. Written only by [`CellGraph::mint`], which is the mint OR of
    /// [graph/README.md § Reach as a hybrid
    /// mask](graph/README.md#reach-as-a-hybrid-mask).
    pins: Matrix<W>,
    /// The pin relation's sparse half: per slot, the sealed regions that cell's values read.
    sealed_holds: Box<[SealedSet]>,
    /// The reverse naming index: per slot, the sealed regions whose frozen aggregate names it.
    /// Written at a seal and read by the next one, so step 2 of the transition finds its namers
    /// without scanning the tier.
    naming: Box<[SealedSet]>,
    sealed: SealedTier<W>,
    /// The tree pool: the third region habitat, uncapped and outside every relation. See
    /// [tree](crate::tree).
    trees: TreePool<C>,
    /// Where the dormant carriers of a cell that has left the slab went, one list per slab slot. A
    /// departed handle maps to the live cell whose reach table absorbed its masks, or to the sealed
    /// cell its storage sealed into; a cell with an empty reach table leaves no entry. Rewritten at
    /// every merge and dropped at the target's reclamation, so a slot's list is bounded by merges
    /// rather than by values — which is what keeps a generation search short and two entries
    /// inline.
    ///
    /// No hashing: the handle's slot is the index and its generation is what the search compares.
    relocated: Box<[SmallVec<[Relocation; 2]>]>,
    /// The tree tombstone lists belonging to cells that have left the slab, keyed by the handle
    /// they still name. Beside the relocation map rather than on its entries: a departed cell with
    /// tombstones under it is rare, and a slot whose occupants relocate over and over — which is
    /// every producer of a chain — would otherwise pay for the field in every entry it keeps.
    departed_tombstones: Vec<(SlabHandle, u32)>,
    /// The embedder's crossing verdict, taken at construction. There is no verdict-free
    /// constructor: a graph that can place a value can price the placement. One indirect call per
    /// priced operand is nothing beside the walk that prices it, and keeping the closure here is
    /// what leaves every other signature free of the parameter.
    verdict: Box<dyn FnMut(Prices) -> Verdict>,
    /// The one place a verb's transients live, reset at the entry of `create`, `enter` and
    /// `release` and never inside one.
    ///
    /// **Nothing but the three verbs reads this field.** The cascade takes `&mut self`, so a
    /// transient borrowing the field would conflict with it; a verb therefore takes the scratch
    /// region off the graph for its whole length and passes it down as a parameter.
    ///
    /// `None` is what it leaves behind, so the rule is a panic rather than a convention: a method
    /// that reached for the scratch region mid-verb would find nothing there instead of quietly
    /// minting a second one. The test-only walkers reach for it directly, and run outside any verb.
    scratch: Option<Scratch>,
    executing: Bits<W>,
    cap: u32,
    /// Units of maintenance the seal transitions of this graph have performed — the quantity the
    /// bounded-transition test asserts is independent of a cell region's dormant value count.
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

impl<C: Reattachable, const W: usize> CellGraph<C, W> {
    /// A slab of `cap` cells. The cap is fixed here and the graph never grows past it, and it is at
    /// or below `64 · W`, the slab width the graph's *type* fixes: every slab relation is a row of
    /// `W` words held inline, so a relation costs `64 · W × W` words of the graph's own bytes — a
    /// dense matrix is quadratic in the width by construction — and a graph wide enough for that to
    /// matter is one the embedder boxes. The sealed tier grows in its own id space and takes no
    /// cap: retention is priced, not bounded.
    ///
    /// Admission is refused at the cap, not at the width, so a two-cell graph over a 64-cell row is
    /// full at two.
    ///
    /// `verdict` is the embedder's crossing verdict, consulted once per operand of every placement
    /// over operands. It is taken here rather than at a door so that no other signature names a
    /// price.
    ///
    /// # Panics
    ///
    /// If `cap` exceeds the width. A slot the relations cannot name is not a slot.
    pub fn new(cap: u32, verdict: impl FnMut(Prices) -> Verdict + 'static) -> Self {
        assert!(
            cap <= Bits::<W>::CELLS,
            "a cap of {cap} does not fit a {}-cell slab; widen the graph's word count",
            Bits::<W>::CELLS
        );
        let slots = (0..cap)
            .map(|_| SlabCell::free(0))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        CellGraph {
            slots,
            free: (0..cap).rev().collect(),
            birth: Matrix::new(),
            pins: Matrix::new(),
            sealed_holds: (0..cap).map(|_| SealedSet::new()).collect(),
            naming: (0..cap).map(|_| SealedSet::new()).collect(),
            sealed: SealedTier::new(cap),
            trees: TreePool::new(),
            relocated: (0..cap).map(|_| SmallVec::new()).collect(),
            departed_tombstones: Vec::new(),
            verdict: Box::new(verdict),
            scratch: Some(Scratch::new()),
            executing: Bits::new(),
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
        parent: Option<SlabHandle>,
        continuation: Option<C::At<'static>>,
    ) -> Result<SlabHandle, CreateError> {
        let parent_slot = match parent {
            Some(parent) => Some(self.live_slot(parent).map_err(CreateError::StaleParent)?),
            None => None,
        };
        self.take_scratch().reset();
        let slot = self.free.pop().ok_or(CreateError::SlabFull)?;
        let cell = &mut self.slots[slot as usize];
        cell.state = SlabState::Live;
        cell.parent = parent_slot;
        // A continuation handed in from outside is at `'static`: it captures nothing any region
        // owns, so it reaches nothing and takes no reach-table entry.
        cell.continuation = continuation.map(Erased::store);
        let generation = cell.generation;
        if let Some(parent_slot) = parent_slot {
            self.birth.inherit_row(slot, parent_slot);
            self.birth.set(slot, parent_slot);
        }
        Ok(SlabHandle::new(slot, generation))
    }

    /// Run `step` against the cell, with its executing flag set for the scope.
    ///
    /// The step receives a context, not the graph, so it can neither enter another cell nor
    /// declare a death; the graph stays exclusively borrowed for the whole call.
    ///
    /// **Both of the context's brands are fresh here**, quantified by this call because both are
    /// elided in `step`'s argument. `'b` brands the carriers this step's doors hand back; `'cell`
    /// names the executing cell's own region. `R` is chosen outside the call, so it can name
    /// neither — which is what makes handing region-borrowing values back re-anchored at them
    /// sound, and what keeps a `'cell` reference from leaving the step that minted it.
    ///
    /// Re-entering the executing cell is not representable: the step never holds the graph.
    ///
    /// ```compile_fail
    /// use cellgraph::{CellGraph, Verdict, reattachable};
    /// struct Owned;
    /// reattachable!(Owned => String);
    ///
    /// let mut graph: CellGraph<Owned> = CellGraph::new(4, |_| Verdict::Pin);
    /// let cell = graph.create(None, None).unwrap();
    /// graph
    ///     .enter(cell, |_context| {
    ///         // `graph` is already exclusively borrowed by the `enter` this closure runs under.
    ///         let _ = graph.enter(cell, |_| ());
    ///     })
    ///     .unwrap();
    /// ```
    pub fn enter<R>(
        &mut self,
        cell: impl Into<CellHandle>,
        step: impl FnOnce(&mut StepContext<'_, '_, C, W>) -> R,
    ) -> Result<R, EnterError> {
        let cell = cell.into();
        self.begin(cell)?;
        // The executing cell's region is minted here rather than at its first write, so the step's
        // own writer is a field copy and the one borrow-widening retype sits at the step's edge
        // beside `begin`. `Region::new` claims no chunk, so a step that never writes costs none.
        match cell {
            CellHandle::Slab(handle) => {
                self.slots[handle.slot() as usize]
                    .region
                    .get_or_insert_with(Region::new);
            }
            CellHandle::Tree(handle) => {
                self.trees.region_mut(handle.index());
            }
        }
        // Taken again through a **shared** path, and the exclusive borrow above is over. A writer
        // descended from an exclusive borrow of the region would not survive the step: a bump's
        // fields are interior-mutable, but an exclusive borrow is unique over them all the same,
        // so the first read of that bump through any other path — a price query walking the
        // graph — freezes it and forbids every write underneath. A shared borrow of the region
        // carries the interior mutability instead, and tolerates both.
        let region = match cell {
            CellHandle::Slab(handle) => self.slots[handle.slot() as usize].region.as_ref(),
            CellHandle::Tree(handle) => self.trees.region(handle.index()),
        }
        .expect("the executing cell's region was just minted");
        // SAFETY: `writer_at` asks that the region stay where it is, unmoved and undropped, for
        // the whole of the brand. That brand is `'cell`, quantified by this call and nameable
        // nowhere outside `step`, so it lies within this `enter` — and `enter` holds the graph
        // exclusively for its whole length, so `release`, `release_tree` and every disposal, the
        // only paths that take a region off its cell, cannot run under it.
        let writer = unsafe { region.writer_at() };
        // The scratch comes off the graph for the whole step: the doors take `&mut self`, so a
        // transient borrowing the field could not coexist with them. The context's `Drop` hands it
        // back, so a panicking step loses no chunk.
        let mut scratch = self.take_scratch_owned();
        scratch.reset();
        let mut context = StepContext {
            graph: self,
            cell,
            writer,
            scratch: Some(scratch),
            _cell: PhantomData,
        };
        Ok(step(&mut context))
    }

    /// Declare the cell's death: the embedder promises never to enter it again.
    ///
    /// The cell's own birth row releases wholesale — birth holds exist for execution, and the cell
    /// will not execute again. What happens to the slot then is the disposal's call: reclaimed if
    /// nothing reaches it, absorbed into a unique holder if `absorption` allows and one is there,
    /// sealed if something else reaches it, and left undisposed only while a descendant's birth row
    /// still names it.
    ///
    /// `absorption` is the embedder's say over that merge, recorded on the slot and applied
    /// whenever the slot actually disposes.
    pub fn release(
        &mut self,
        handle: SlabHandle,
        absorption: ReleaseAbsorption,
    ) -> Result<(), ReleaseError> {
        let slot = self.live_slot(handle).map_err(ReleaseError::Stale)?;
        if self.executing.test(slot) {
            return Err(ReleaseError::Executing);
        }
        self.birth.clear_row(slot);
        let cell = &mut self.slots[slot as usize];
        cell.state = SlabState::Dead;
        cell.absorption = absorption;
        // A release runs its cascade outside any step, so the scratch region is taken and reset
        // here for the same reason `enter` takes and resets it: a verb's transients start on empty
        // ground.
        self.park()
            .run(|graph, scratch| graph.dispose_chain(slot, scratch));
        Ok(())
    }

    /// Create a tree cell under `parent` — a slab cell, which becomes its **root**, or another
    /// tree cell, whose root it inherits.
    ///
    /// The cell owns its region outright and takes no slab slot, so this door has no full refusal:
    /// what bounds the pool is the depth of the call tree, not the slab's cap. It takes no row and
    /// no column either — a placement into it mints into its root, and a carrier homed in it
    /// travels with the root's bit ([tree/README.md](tree/README.md)).
    pub fn create_tree(
        &mut self,
        parent: impl Into<CellHandle>,
        continuation: Option<C::At<'static>>,
    ) -> Result<TreeHandle, Stale<CellHandle>> {
        let (root, tree_parent, depth) = match parent.into() {
            CellHandle::Slab(handle) => (self.live_slot(handle)?, Ancestor::Root, 1),
            CellHandle::Tree(handle) => {
                let index = self.trees.live_index(handle)?;
                (
                    self.trees.root(index),
                    Ancestor::Tree(index),
                    self.trees.depth(index) + 1,
                )
            }
        };
        self.take_scratch().reset();
        // A continuation handed in from outside is at `'static`: it captures nothing any region
        // owns, so it reaches nothing — and a tree cell records no reach in any case.
        let continuation = continuation.map(Erased::store);
        let handle = self.trees.create(root, tree_parent, depth, continuation);
        match tree_parent {
            Ancestor::Tree(index) => self.trees.add_child(index),
            Ancestor::Root => self.slots[root as usize].tree_children += 1,
        }
        Ok(handle)
    }

    /// Declare a tree cell's death. **No absorption argument**: where its bytes go was settled at
    /// the placement door that pinned a value homed here into an ancestor, and the pledge that door
    /// left is what disposal reads.
    ///
    /// A cell whose children have all disposed disposes at once — splicing its bump into its pledge
    /// or reclaiming it — and the disposal walks up through every dead-but-undisposed ancestor it
    /// unblocks, into the slab's own cascade at the root. One released while a child still lives
    /// goes **dead-but-undisposed**: stale to every door, region kept, disposed when its last child
    /// does.
    pub fn release_tree(&mut self, handle: TreeHandle) -> Result<(), ReleaseTreeError> {
        let index = self
            .trees
            .live_index(handle)
            .map_err(ReleaseTreeError::Stale)?;
        if self.trees.is_executing(index) {
            return Err(ReleaseTreeError::Executing);
        }
        self.trees.mark_dead(index);
        self.park()
            .run(|graph, scratch| graph.dispose_tree_chain(index, scratch));
        Ok(())
    }

    /// Dispose of the just-released tree cell and then of every dead-but-undisposed ancestor its
    /// disposal leaves with no undisposed child, innermost first — and, at the top, of the root
    /// through the slab's own cascade.
    ///
    /// The walk is complete for the same reason the slab's is: only creation and disposal move a
    /// child count, so the only cells this release can bring to zero are the ones on its own chain
    /// upward, and the first ancestor that is still live or still has another child stops it.
    fn dispose_tree_chain(&mut self, released: u32, scratch: &Scratch) {
        let mut next = Some(released);
        while let Some(index) = next {
            if self.trees.state(index) != TreeState::Dead || self.trees.children(index) > 0 {
                return;
            }
            // Read the links first: disposal recycles the slot or turns it into a tombstone, and
            // either way the chain fields stop answering for the cell that was there.
            let parent = self.trees.parent(index);
            let root = self.trees.root(index);
            self.dispose_tree(index, scratch);
            match parent {
                Ancestor::Tree(parent) => {
                    self.trees.drop_child(parent);
                    next = Some(parent);
                }
                Ancestor::Root => {
                    debug_assert!(
                        self.slots[root as usize].tree_children > 0,
                        "a tree cell disposed under a root that counted none"
                    );
                    self.slots[root as usize].tree_children -= 1;
                    if self.slots[root as usize].state == SlabState::Dead {
                        self.dispose_chain(root, scratch);
                    }
                    return;
                }
            }
        }
    }

    /// Take one tree cell out: its bump splices into the ancestor it pledged, or is dropped where
    /// it pledged none, and its identity either recycles or stays behind as a tombstone.
    ///
    /// O(1). No hold arithmetic and no seal transition runs — a tree cell holds nothing of its own
    /// — and the splice moves a `Bump` without moving a chunk byte.
    fn dispose_tree(&mut self, index: u32, scratch: &Scratch) {
        let region = self.trees.take_region(index);
        let into = match self.trees.pledge(index) {
            None => None,
            Some(Ancestor::Root) => {
                let root = self.trees.root(index);
                Region::splice_optional(&mut self.slots[root as usize].region, region);
                Some(CellHandle::Slab(self.occupant(root)))
            }
            Some(Ancestor::Tree(dest)) => {
                self.trees.splice_into(dest, region);
                Some(CellHandle::Tree(self.trees.occupant(dest)))
            }
        };
        match into {
            // The bytes moved and something may still name this cell: it stays as a tombstone
            // pointing at where they went, and the tombstones already hanging off it stay hanging
            // off it, so the chain lengthens rather than being repointed.
            Some(into) if self.trees.leaves_tombstone(index) => match into {
                CellHandle::Tree(dest) => {
                    self.trees.entomb(index, CellHandle::Tree(dest), None);
                    self.trees.adopt_tombstone(dest.index(), index);
                }
                CellHandle::Slab(root) => {
                    let head = self.slots[root.slot() as usize].tree_tombstones;
                    self.trees.entomb(index, CellHandle::Slab(root), head);
                    self.slots[root.slot() as usize].tree_tombstones = Some(index);
                }
            },
            // Either nothing can name it, or its bytes are gone. A reclaim takes the tombstones
            // that spliced into it with it: their bytes died in this bundle.
            _ => {
                let tombstones = self.trees.tombstones(index);
                self.trees.free_tombstones(tombstones, scratch);
                self.trees.recycle(index);
            }
        }
    }

    /// Whether the graph holds nothing at all: every slab slot free, no sealed region left, and no
    /// tree cell or tombstone in the pool.
    ///
    /// The embedder's end-of-program alarm, and the only one the substrate ships. After the last
    /// release, a non-empty graph means either a release was forgotten — a slot is still occupied
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
        vacant && self.sealed.is_empty() && self.trees.is_empty()
    }

    /// Whether the name is a cell of either kind that is still live — false for a slot or pool
    /// index that is free, holds a later generation, or holds a cell whose death was already
    /// declared, and false for a tree tombstone.
    pub fn is_live(&self, cell: impl Into<CellHandle>) -> bool {
        match cell.into() {
            CellHandle::Slab(handle) => self.live_slot(handle).is_ok(),
            CellHandle::Tree(handle) => self.trees.live_index(handle).is_ok(),
        }
    }

    /// The slot a handle names, if that slot still holds the live cell the handle was minted for.
    fn live_slot(&self, handle: SlabHandle) -> Result<u32, Stale<SlabHandle>> {
        match self.slots.get(handle.slot() as usize) {
            Some(cell)
                if cell.state == SlabState::Live && cell.generation == handle.generation() =>
            {
                Ok(handle.slot())
            }
            _ => Err(Stale(handle)),
        }
    }

    /// The scratch region, for a verb that resets it and builds nothing of its own.
    fn take_scratch(&mut self) -> &mut Scratch {
        self.scratch
            .as_mut()
            .expect("the scratch region is on the graph outside a verb")
    }

    /// The scratch region, off the graph for the length of a verb. What it leaves behind is
    /// `None`, so any other reader of the field fails loudly rather than minting a second region.
    fn take_scratch_owned(&mut self) -> Scratch {
        self.scratch
            .take()
            .expect("the scratch region is on the graph outside a verb")
    }

    /// The scratch region off the graph and reset, under a guard that hands it back however the
    /// verb ends.
    fn park(&mut self) -> Parked<'_, C, W> {
        let mut scratch = self.take_scratch_owned();
        scratch.reset();
        Parked {
            graph: self,
            scratch: Some(scratch),
        }
    }

    /// Set the executing flag, or refuse. Paired with the clear in [`StepContext`]'s `Drop`, so
    /// the flag falls even if the step panics.
    fn begin(&mut self, cell: CellHandle) -> Result<(), EnterError> {
        match cell {
            CellHandle::Slab(handle) => {
                let slot = self
                    .live_slot(handle)
                    .map_err(|stale| EnterError::Stale(stale.into()))?;
                if self.executing.test(slot) {
                    return Err(EnterError::AlreadyExecuting);
                }
                self.executing.set(slot);
            }
            CellHandle::Tree(handle) => {
                let index = self
                    .trees
                    .live_index(handle)
                    .map_err(|stale| EnterError::Stale(stale.into()))?;
                if self.trees.is_executing(index) {
                    return Err(EnterError::AlreadyExecuting);
                }
                self.trees.set_executing(index, true);
            }
        }
        Ok(())
    }

    /// The slots currently occupied — live cells and dead-but-undisposed ones alike.
    ///
    /// A dead-but-undisposed cell counts as a holder: its hold set releases when its slot goes, not
    /// when its death is declared, so its holds outlive it exactly as long as it does.
    fn occupied(&self) -> impl Iterator<Item = u32> + '_ {
        (0..self.cap).filter(|slot| self.slots[*slot as usize].state != SlabState::Free)
    }

    /// Whether a dead cell's slot may leave the slab now: no occupant's birth row still names it,
    /// and no tree cell under it is undisposed. Birth holds are the one relation that keeps a dead
    /// cell in place — a descendant that can still walk to it has not finished with it, and the
    /// relation has no sealed half for the walk to follow — and a tree child is the same relation
    /// counted rather than rowed, since no matrix names a tree cell.
    ///
    /// Execution does not enter the question: `release` refuses an executing cell and `begin`
    /// refuses a dead one, so a dead cell is never executing.
    fn disposable(&self, slot: u32) -> bool {
        // A free slot's birth row is cleared before the slot is recycled, so no free row names
        // anything and the count across every row is the count across the occupied ones.
        if self.slots[slot as usize].tree_children > 0 {
            return false;
        }
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
    fn occupant(&self, slot: u32) -> SlabHandle {
        SlabHandle::new(slot, self.slots[slot as usize].generation)
    }

    /// Where the dormant carriers a key names live now, or `None` when their storage is gone.
    ///
    /// A handle whose slot still holds it names that slot directly, at first index zero. Otherwise
    /// the cell has left the slab, and the relocation map answers — or does not, which means the
    /// cell reclaimed or left an empty reach table behind.
    fn locate(&self, home: SlabHandle) -> Option<SlabForward> {
        let cell = &self.slots[home.slot() as usize];
        if cell.state != SlabState::Free && cell.generation == home.generation() {
            return Some(SlabForward::Slab {
                slot: home.slot(),
                first_index: 0,
            });
        }
        self.relocation(home).map(|entry| entry.location)
    }

    /// The relocation entry for one departed handle, found by walking its slot's list for the
    /// matching generation.
    fn relocation(&self, handle: SlabHandle) -> Option<&Relocation> {
        self.relocated[handle.slot() as usize]
            .iter()
            .find(|entry| entry.generation == handle.generation())
    }

    fn relocation_mut(&mut self, handle: SlabHandle) -> Option<&mut Relocation> {
        self.relocated[handle.slot() as usize]
            .iter_mut()
            .find(|entry| entry.generation == handle.generation())
    }

    /// Point `handle` at `location` and push it onto a lineage chain whose current head is
    /// `chain_head`. The caller stores `Some(handle)` as the new head, so the chain is threaded
    /// through the entries rather than collected beside them.
    fn relocate(
        &mut self,
        handle: SlabHandle,
        location: SlabForward,
        chain_head: Option<SlabHandle>,
    ) {
        match self.relocation_mut(handle) {
            Some(entry) => {
                entry.location = location;
                entry.next = chain_head;
            }
            None => self.relocated[handle.slot() as usize].push(Relocation {
                generation: handle.generation(),
                location,
                next: chain_head,
            }),
        }
    }

    /// Drop every entry on a chain — the storage those keys named is gone, so a redeem under one
    /// of them refuses rather than finding a stale answer.
    fn forget_chain(&mut self, head: Option<SlabHandle>, scratch: &Scratch) {
        let mut next = head;
        while let Some(handle) = next {
            let list = &mut self.relocated[handle.slot() as usize];
            let at = list
                .iter()
                .position(|entry| entry.generation == handle.generation())
                .expect("a chain entry is in its slot's list");
            next = list[at].next;
            list.swap_remove(at);
            // The bytes this entry forwarded to are gone, so the tree tombstones that pointed
            // through it go with them: a redeem under one of their keys must answer `Gone`.
            if let Some(at) = self
                .departed_tombstones
                .iter()
                .position(|(named, _)| *named == handle)
            {
                let (_, head) = self.departed_tombstones.swap_remove(at);
                self.trees.free_tombstones(Some(head), scratch);
            }
        }
    }

    /// Move every handle on `head`'s chain onto `onto`'s chain, pointing each at `target`. What an
    /// absorbed sealed cell's lineage takes, and what a run of freshly departed handles takes at a
    /// seal.
    fn relink_chain(
        &mut self,
        head: Option<SlabHandle>,
        target: SealedId,
        onto: &mut Option<SlabHandle>,
    ) {
        let mut next = head;
        while let Some(handle) = next {
            let entry = self
                .relocation_mut(handle)
                .expect("a chain entry is in its slot's list");
            next = entry.next;
            entry.location = SlabForward::Sealed(target);
            entry.next = *onto;
            *onto = Some(handle);
        }
    }

    /// Every departed handle whose dormant carriers `slot` answers for, plus `slot`'s own occupant
    /// if anything still has to find its way to it — a dormant carrier kept in it, or a tree
    /// tombstone whose bytes spliced into its bundle. Taken off the slot: the caller is moving them
    /// somewhere else.
    ///
    /// The occupant comes last when it comes at all, and the flag says whether it came, which is
    /// what lets a caller that has to treat it differently from the inherited entries split the run
    /// rather than re-derive it.
    fn take_lineage<'s>(
        &mut self,
        slot: u32,
        scratch: &'s Scratch,
    ) -> (&'s mut [SlabHandle], bool) {
        let cell = &self.slots[slot as usize];
        let occupant = (!cell.reaches.is_empty() || cell.tree_tombstones.is_some())
            .then(|| self.occupant(slot));
        let head = self.slots[slot as usize].lineage.take();
        let mut walk = head;
        let mut length = 0;
        while let Some(handle) = walk {
            length += 1;
            walk = self
                .relocation(handle)
                .expect("a chain entry is in its slot's list")
                .next;
        }
        let mut next = head;
        let run = scratch.slice_with(length + usize::from(occupant.is_some()), |_| match next {
            Some(handle) => {
                next = self
                    .relocation(handle)
                    .expect("a chain entry is in its slot's list")
                    .next;
                handle
            }
            None => occupant.expect("only the occupant sits past the slot's own entries"),
        });
        (run, occupant.is_some())
    }

    /// Move a departing cell's tree tombstones onto the relocation entry that now forwards its
    /// handle, so they resolve through wherever its bundle went and are freed when that entry is.
    fn carry_tree_tombstones(&mut self, slot: u32, departing: SlabHandle) {
        if let Some(head) = self.slots[slot as usize].tree_tombstones.take() {
            debug_assert!(
                self.relocation(departing).is_some(),
                "a departing occupant with tombstones has just been relocated"
            );
            self.departed_tombstones.push((departing, head));
        }
    }

    /// Point every handle of `lineage` at `target`, and record them on the target so its own
    /// retirement can drop them again.
    fn relocate_to_sealed(&mut self, lineage: &[SlabHandle], target: SealedId) {
        // The head comes out of the sealed cell for the walk and goes back after it: the walk
        // writes the relocation lists, and holding a borrow of the sealed cell across that would
        // name two fields of the graph at once.
        let mut head = std::mem::take(
            &mut self
                .sealed
                .get_mut(target)
                .expect("the relocation target is in the tier")
                .lineage,
        );
        for handle in lineage {
            self.relocate(*handle, SlabForward::Sealed(target), head);
            head = Some(*handle);
        }
        self.sealed
            .get_mut(target)
            .expect("the relocation target is in the tier")
            .lineage = head;
    }

    /// Take a disposable dead cell out of the slab, by the four exits it has: reclamation when
    /// nothing reaches its storage, absorption into a unique slab holder, a seal into a single
    /// sealed namer, and the plain seal everything else takes
    /// ([graph/README.md § Locality tactics](graph/README.md#locality-tactics)).
    ///
    /// The two merges are the degenerate shapes the model is designed around — a chain of
    /// single-consumer producers — and each one is a sealed cell the tier never mints. A refused
    /// release falls through to the plain seal.
    fn dispose(&mut self, slot: u32, scratch: &Scratch) {
        let namers = std::mem::take(&mut self.naming[slot as usize]);
        // Nothing holds the storage, so no merge has anything to fold it into and the identity of
        // a holder is not wanted: the pin tally settles that without reading down the column.
        if self.pins.holders(slot) == 0 && namers.is_empty() {
            self.reclaim(slot, scratch);
            return;
        }
        // A run the tally sizes, filled off a scan: the count of occupied rows naming this slot is
        // exactly what the pin tally holds, so the scan running short or long is the tally being
        // wrong rather than the run being the wrong shape. The short side holds in every profile
        // — a run has to be filled, so there is no cheaper answer than the panic — and the long
        // side is a debug assert, since a run already full has nowhere to put the surplus.
        let holders = {
            let mut naming_rows = self.occupied().filter(|other| self.pins.test(*other, slot));
            let holders = scratch.slice_with(self.pins.holders(slot) as usize, |_| {
                naming_rows
                    .next()
                    .expect("the pin tally outruns a scan across the occupied rows")
            });
            debug_assert!(
                naming_rows.next().is_none(),
                "a scan across the occupied rows outruns the pin tally"
            );
            holders
        };
        let refused = self.slots[slot as usize].absorption == ReleaseAbsorption::Refused;
        match (&*holders, namers.len()) {
            ([into], 0) if !refused => self.absorb_into_cell(slot, *into, scratch),
            ([], 1) => {
                let namer = namers.iter().next().expect("the set holds one id");
                self.fold_into_namer(slot, namer, scratch);
            }
            _ => self.seal(slot, holders, &namers, scratch),
        }
    }

    /// Merge 1: fold a dying cell's storage and holds into the one slab occupant that holds it,
    /// minting no sealed cell at all.
    ///
    /// The target is any occupant, live or dead-but-undisposed: "live holder" in the design means
    /// the slab tier as opposed to the sealed one, and a dead-but-undisposed cell's row is still a
    /// maintained row. Its holds become the target's — the slab half through the standard mint,
    /// whose and-not is what makes a hold the dead cell had *on its own holder* land nowhere,
    /// dissolving a two-cell ring rather than sealing it.
    ///
    /// Reads of the absorbed values stay on the per-value-mask path: the chunks are now the
    /// target's own storage, which its stored mask already names, so no id enters the picture.
    fn absorb_into_cell(&mut self, dead: u32, into: u32, scratch: &Scratch) {
        let holds = self.take_holds(dead);
        // The target's hold on the dead cell is structural from here on: the storage is its own.
        self.pins.clear(into, dead);
        self.migrate_reaches(dead, into, scratch);
        self.pins.hold(into, &holds);

        // The sparse half moves holder without changing count, except where the target already
        // held the same region: there the dead cell's hold simply vanishes.
        // A list, not a set: the source ids arrive distinct and ascending, so pushing them keeps
        // both properties without a second sorted insert.
        let mut dups = scratch.vec();
        for id in holds.sealed().iter() {
            if !self.sealed_holds[into as usize].insert(id) {
                dups.push(id);
            }
        }
        let storage = self.slots[dead as usize].region.take();
        Region::splice_optional(&mut self.slots[into as usize].region, storage);

        self.vacate(dead, &dups, scratch);
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

    /// Move a dying cell's dormant carriers' masks into the cell absorbing it, re-homed: bit `dead`
    /// becomes bit `into` throughout, since that storage is the target's own bundle from here on.
    ///
    /// Every key minted under a handle the dead cell answered for is forwarded to the target's
    /// reach table at its new first index, so a dormant carrier survives any number of merges. A
    /// dead cell with an empty reach table forwards nothing and leaves no entry behind. The moved
    /// block is appended without interning: its position is what forwards the keys minted under it.
    fn migrate_reaches(&mut self, dead: u32, into: u32, scratch: &Scratch) {
        // The target's own dormant carriers lose the dead cell's bit: those chunks are its
        // storage now.
        for mask in self.slots[into as usize].reaches.iter_mut() {
            mask.remove_slot(dead);
        }
        let first_index = self.slots[into as usize].reaches.len();
        // Taken before the reach table is, so the run carries the departing occupant exactly when
        // something — a kept dormant carrier or a tree tombstone — still has to find its way to it.
        let (lineage, has_occupant) = self.take_lineage(dead, scratch);
        let moved = self.slots[dead as usize].reaches.take();
        for mut mask in moved {
            mask.remove_slot(dead);
            mask.add(into);
            self.slots[into as usize].reaches.append(mask);
        }

        // The departing occupant is the run's last entry and the only one landing at the moved
        // block's own first index; every other entry was minted under an earlier merge and moves by
        // the block's offset.
        let (departing, inherited) = match has_occupant {
            true => {
                let (occupant, rest) = lineage
                    .split_last()
                    .expect("an included occupant is on the run");
                (Some(*occupant), rest)
            }
            false => (None, &*lineage),
        };
        let mut head = self.slots[into as usize].lineage;
        for handle in inherited {
            let SlabForward::Slab {
                first_index: old, ..
            } = self
                .relocation(*handle)
                .expect("a slot's lineage entry points at that slot")
                .location
            else {
                unreachable!("a slot's lineage entry points at a slab slot, not a sealed cell");
            };
            self.relocate(
                *handle,
                SlabForward::Slab {
                    slot: into,
                    first_index: first_index + old,
                },
                head,
            );
            head = Some(*handle);
        }
        if let Some(departing) = departing {
            self.relocate(
                departing,
                SlabForward::Slab {
                    slot: into,
                    first_index,
                },
                head,
            );
            // The tombstones that spliced into the dead cell's bundle keep naming its handle; the
            // entry just written is what forwards them into the target's bundle from here on.
            self.carry_tree_tombstones(dead, departing);
            head = Some(departing);
        }
        self.slots[into as usize].lineage = head;
    }

    /// Merge 3: a cell nothing in the slab holds, named by exactly one sealed aggregate, folds into
    /// that sealed cell instead of minting one beside it.
    ///
    /// No stored mask needs rewriting: a stored mask naming a slot implies a pin hold on it, and
    /// this cell's slab column is empty by the precondition.
    fn fold_into_namer(&mut self, dead: u32, namer: SealedId, scratch: &Scratch) {
        let holds = self.take_holds(dead);
        let storage = self.slots[dead as usize].region.take();
        // The namer's hold on the dead cell is structural; the slots its row named trade the dead
        // cell's bit for the sealed cell's own name inside the fold.
        self.sealed
            .get_mut(namer)
            .expect("the namer came out of the reverse index")
            .aggregate
            .remove_slot(dead);
        let (_, dups) = self.fold_into_sealed(namer, holds, storage, scratch);
        let (lineage, has_occupant) = self.take_lineage(dead, scratch);
        self.relocate_to_sealed(lineage, namer);
        if has_occupant {
            let departing = *lineage.last().expect("an included occupant is on the run");
            self.carry_tree_tombstones(dead, departing);
        }

        self.vacate(dead, &dups, scratch);
        #[cfg(test)]
        {
            self.seal_work += 1 + dups.len() as u64;
            self.merges.into_namer += 1;
        }
        // The dead cell may have been the namer's last holder — a ring whose final cell just died.
        if self.reclaim_if_unheld(namer, scratch) {
            return;
        }
        self.absorb_singletons(namer, scratch);
    }

    /// Fold a hold set and a region into an existing sealed cell: the shared body of merges 2 and
    /// 3.
    ///
    /// Returns the sealed ids that *transferred* (absent from the target's aggregate, so the hold
    /// changed owner without changing count) and the ones that *duplicated* (already there, so one
    /// hold on each vanishes). The caller releases the duplicates, since the borrow of the sealed
    /// cell has to end first, and then checks whether the target still has a holder: a source that
    /// held its own target contributes a self-hold, which has no representation and drops the
    /// count.
    fn fold_into_sealed<'s>(
        &mut self,
        target: SealedId,
        holds: GraphReach<W>,
        storage: Option<Region>,
        scratch: &'s Scratch,
    ) -> (ScratchVec<'s, SealedId>, ScratchVec<'s, SealedId>) {
        let naming = &mut self.naming;
        let sealed_cell = self
            .sealed
            .get_mut(target)
            .expect("the merge target is in the tier");
        // The slots the fold newly reaches register the target, so the next seal of one of them
        // finds it. Registering before the union is what makes "newly" a reading off the
        // aggregate: after it, every slot the fold names is one the aggregate names.
        for slot in holds
            .slab_slots()
            .filter(|slot| !sealed_cell.aggregate.names(*slot))
        {
            naming[slot as usize].insert(target);
        }
        sealed_cell.aggregate.union_slab_with(&holds);

        // Two lists, not sets: the source's sealed half is already distinct and ascending, and a
        // fold puts each id in exactly one of them.
        let mut transferred = scratch.vec();
        let mut duplicated = scratch.vec();
        for id in holds.sealed().iter() {
            if id == target {
                // The source held its own target. The hold becomes a self-hold, which no aggregate
                // can express, so it simply goes.
                debug_assert!(
                    sealed_cell.holders >= 1,
                    "a sealed cell with no holder is still in the tier"
                );
                sealed_cell.holders -= 1;
                continue;
            }
            if sealed_cell.aggregate.add_sealed(id) {
                transferred.push(id);
            } else {
                duplicated.push(id);
            }
        }
        debug_assert!(
            !sealed_cell.aggregate.names_sealed(target),
            "a sealed cell's aggregate names itself"
        );
        self.sealed.splice_storage(target, storage);
        (transferred, duplicated)
    }

    /// Merge 2: absorb every count-1 sealed region the sealed cell `target` holds, to a fixpoint.
    ///
    /// A count of 1 on a sealed cell the target names means the target *is* that holder, so the
    /// cell region is reachable through this sealed cell and nothing else — exactly the chain of
    /// single-consumer producers the tier would otherwise keep as a chain of sealed cells. The
    /// candidate set is a worklist rather than one pass: a fold transfers ids the target did not
    /// hold before, and drops a duplicate's count, either of which can newly qualify.
    fn absorb_singletons(&mut self, target: SealedId, scratch: &Scratch) {
        let mut pending = scratch.vec();
        {
            let Some(sealed_cell) = self.sealed.get(target) else {
                return;
            };
            pending.extend(sealed_cell.aggregate.sealed().iter());
        }
        while let Some(source) = pending.pop() {
            if source == target {
                continue;
            }
            match self.sealed.get(source) {
                Some(sealed_cell) if sealed_cell.holders == 1 => {}
                _ => continue,
            }
            let absorbed = self
                .sealed
                .remove(source)
                .expect("the sealed cell was just read");
            {
                // The absorbed sealed cell's chain moves onto the target's whole, each entry
                // repointed as it goes. The target's head comes out for the walk and goes back
                // after it.
                let mut head = std::mem::take(
                    &mut self
                        .sealed
                        .get_mut(target)
                        .expect("the target is in the tier")
                        .lineage,
                );
                self.relink_chain(absorbed.lineage, target, &mut head);
                self.sealed
                    .get_mut(target)
                    .expect("the target is in the tier")
                    .lineage = head;
            }
            // The aggregate is an owned local, so its slots are walked straight into `naming`
            // rather than collected first. The tally reads the count before the fold moves it.
            #[cfg(test)]
            let named = absorbed.aggregate.slab_slots().count() as u64;
            for slot in absorbed.aggregate.slab_slots() {
                self.naming[slot as usize].remove(source);
            }
            self.sealed
                .get_mut(target)
                .expect("the target is in the tier")
                .aggregate
                .remove_sealed(source);
            #[cfg(test)]
            let sealed_width = absorbed.aggregate.sealed().len() as u64;
            let (transferred, duplicated) =
                self.fold_into_sealed(target, absorbed.aggregate, Some(absorbed.storage), scratch);
            // Each duplicate had at least two holders — the target and the absorbed sealed cell —
            // so none of these counts reaches zero, and the target survives the call.
            self.release_sealed_holds(&duplicated, scratch);
            #[cfg(test)]
            {
                self.seal_work += 1 + named + sealed_width;
                self.merges.at_seal += 1;
            }

            // The absorbed sealed cell held its own holder, and was its last: the ring dissolves.
            if self.reclaim_if_unheld(target, scratch) {
                return;
            }
            pending.extend(transferred.iter().copied());
            pending.extend(duplicated.iter().copied());
        }
    }

    /// Dispose of the just-released cell and then of every dead ancestor the release left with no
    /// birth holder, innermost first — the whole cascade one death can set off, walked rather than
    /// scanned for.
    ///
    /// The walk is complete because only `create` and `release` write the birth relation: no
    /// disposal changes any slot's birth-holder count, so the only slots this release can bring to
    /// zero are the ones its own row named, its ancestors. Each ancestor's row names everything
    /// the row below it names ([graph/README.md § Two relations](graph/README.md#two-relations-two-structures)),
    /// so the dead ancestors this release zeroes are a prefix of the chain upward: a live
    /// ancestor, or one another branch's row still names, stops the walk, and everything above it
    /// is still held.
    fn dispose_chain(&mut self, released: u32, scratch: &Scratch) {
        let mut next = Some(released);
        while let Some(slot) = next {
            if self.slots[slot as usize].state != SlabState::Dead || !self.disposable(slot) {
                return;
            }
            // Read the link first: disposal recycles the slot, which clears it.
            next = self.slots[slot as usize].parent;
            self.dispose(slot, scratch);
        }
    }

    /// Return a slot to the free list under a fresh generation, so every handle minted for the
    /// departing occupant is stale from here on.
    fn recycle(&mut self, slot: u32) {
        let cell = &mut self.slots[slot as usize];
        *cell = SlabCell::free(cell.generation.wrapping_add(1));
        self.free.push(slot);
    }

    /// Reclaim a cell nothing reaches: its storage goes, and its hold set releases wholesale.
    ///
    /// Releasing is only ever wholesale — there is no mid-life, per-reason release — which is what
    /// makes the mint's bit-setting idempotence safe.
    fn reclaim(&mut self, slot: u32, scratch: &Scratch) {
        // Nothing reaches this cell's storage, so every mask its dormant carriers named dies with
        // it and the handles it answered for stop resolving. Its own handle was never in the map.
        let lineage = self.slots[slot as usize].lineage.take();
        self.forget_chain(lineage, scratch);
        // The tree bumps spliced into this bundle go with it, so the tombstones that named them
        // stop resolving too.
        let tombstones = self.slots[slot as usize].tree_tombstones.take();
        self.trees.free_tombstones(tombstones, scratch);
        let released = std::mem::take(&mut self.sealed_holds[slot as usize]);
        self.vacate(slot, released.as_slice(), scratch);
    }

    /// Freeze a dying cell's hold set, both halves: the slab row copied, the sparse half taken off
    /// the slot so the ids it names change holder without changing count.
    fn take_holds(&mut self, slot: u32) -> GraphReach<W> {
        GraphReach::from_parts(
            *self.pins.row(slot),
            std::mem::take(&mut self.sealed_holds[slot as usize]),
        )
    }

    /// The tail every exit from the slab shares: the row clears, the slot recycles under a fresh
    /// generation, and one hold on each of `released` drops.
    fn vacate(&mut self, slot: u32, released: &[SealedId], scratch: &Scratch) {
        self.pins.clear_row(slot);
        self.recycle(slot);
        self.release_sealed_holds(released, scratch);
    }

    /// The seal transition: convert every representation of the dying cell from slab bit to sealed
    /// id, detach its storage, and recycle its slot
    /// ([graph/README.md § The seal transition](graph/README.md#the-seal-transition)).
    ///
    /// The work is bounded by `holders`, `namers`, and the aggregate's width — never by what the
    /// region stores. Nothing here reads a region byte: monotone holds make the frozen row
    /// exactly the union of every reach ever minted in, so the aggregate is a word copy.
    fn seal(&mut self, slot: u32, holders: &[u32], namers: &SealedSet, scratch: &Scratch) {
        let id = self.sealed.mint_id();
        // The cell's hold set, both halves, frozen rather than cleared. Its sealed half moves from
        // the cell to the sealed cell, so the ids it names change holder without changing count.
        let aggregate = self.take_holds(slot);
        let storage = self.slots[slot as usize]
            .region
            .take()
            .unwrap_or_else(Region::new);
        let count = (holders.len() + namers.len()) as u32;

        // 1. Holders convert: the slab bit becomes the id, in the hold set and in every mask of
        //    the holder's reach table — the only durable habitat a mask has on the slab side. The
        //    scan is bounded by the holders' entry counts, never by what the cell region stores.
        for holder in holders {
            self.pins.clear(*holder, slot);
            self.sealed_holds[*holder as usize].insert(id);
            let reaches = &mut self.slots[*holder as usize].reaches;
            for mask in reaches.iter_mut() {
                mask.replace_slot(slot, id);
            }
            #[cfg(test)]
            {
                self.seal_work += self.slots[*holder as usize].reaches.len() as u64;
            }
        }
        // 2. Frozen aggregates convert, located through the reverse naming index.
        for namer in namers.iter() {
            if let Some(sealed_cell) = self.sealed.get_mut(namer) {
                sealed_cell.aggregate.replace_slot(slot, id);
            }
        }
        // 3. The new sealed cell registers under every slab bit it names, so the next seal of one
        //    of those slots finds it. The aggregate is a local and `naming` is a field, so the walk
        //    down one writes the other with no copy of the bits in between.
        for named in aggregate.slab_slots() {
            self.naming[named as usize].insert(id);
        }
        #[cfg(test)]
        {
            self.seal_work +=
                (holders.len() + namers.len()) as u64 + aggregate.slab_slots().count() as u64;
        }

        self.sealed.insert(
            id,
            SealedCell {
                aggregate,
                storage,
                holders: count,
                #[cfg(test)]
                peak_holders: count,
                lineage: None,
            },
        );
        // The cell's own dormant carriers' masks are dead bytes from here on: the storage they
        // named is in the sealed cell, and a redeem under one of these keys derives its reach from
        // the sealed cell's id.
        let (lineage, has_occupant) = self.take_lineage(slot, scratch);
        self.relocate_to_sealed(lineage, id);
        if has_occupant {
            let departing = *lineage.last().expect("an included occupant is on the run");
            self.carry_tree_tombstones(slot, departing);
        }
        // The dying cell's sealed half moved into the sealed cell above, so the exit releases
        // nothing.
        self.vacate(slot, &[], scratch);
        // 4. Every count-1 region the new sealed cell holds folds into it: a chain of
        //    single-consumer producers collapses to the one sealed cell at its head rather than one
        //    sealed cell per link.
        self.absorb_singletons(id, scratch);
    }

    /// Drop one hold on each of `released`, reclaiming every sealed cell whose count reaches zero
    /// and cascading through the holds that sealed cell's own aggregate named.
    fn release_sealed_holds(&mut self, released: &[SealedId], scratch: &Scratch) {
        let mut pending = scratch.vec_with_capacity(released.len());
        pending.extend_from_slice(released);
        while let Some(id) = pending.pop() {
            let Some(sealed_cell) = self.sealed.get_mut(id) else {
                continue;
            };
            debug_assert!(
                sealed_cell.holders >= 1,
                "a sealed cell with no holder is still in the tier"
            );
            sealed_cell.holders -= 1;
            if sealed_cell.holders > 0 {
                continue;
            }
            pending.extend(self.retire_sealed(id, scratch).iter());
        }
    }

    /// Retire a sealed cell whose count has reached zero: out of the tier, out of the reverse
    /// naming index, and its storage dropped. Hands back the holds its aggregate named, which the
    /// caller releases in turn.
    fn retire_sealed(&mut self, id: SealedId, scratch: &Scratch) -> SealedSet {
        let Some(sealed_cell) = self.sealed.remove(id) else {
            return SealedSet::new();
        };
        for slot in sealed_cell.aggregate.slab_slots() {
            self.naming[slot as usize].remove(id);
        }
        self.forget_chain(sealed_cell.lineage, scratch);
        // The sealed cell's storage drops here: nothing reaches these chunks any more.
        sealed_cell.aggregate.into_sealed()
    }

    /// Reclaim a sealed cell nothing holds any more — the zero-count exit, reached directly when a
    /// merge dissolves the last hold on its own target rather than through a holder's release.
    fn reclaim_sealed(&mut self, id: SealedId, scratch: &Scratch) {
        let released = self.retire_sealed(id, scratch);
        self.release_sealed_holds(released.as_slice(), scratch);
    }

    /// Reclaim `id` if a fold has just taken its last holder. `true` when it did, so the caller
    /// stops working on a sealed cell that is no longer in the tier.
    fn reclaim_if_unheld(&mut self, id: SealedId, scratch: &Scratch) -> bool {
        let unheld = self
            .sealed
            .get(id)
            .is_some_and(|sealed_cell| sealed_cell.holders == 0);
        if unheld {
            self.reclaim_sealed(id, scratch);
        }
        unheld
    }

    /// The mint: fold a value's reach into the hold set of the region that now stores it, minus
    /// that region's own bit. **The only write into the pin relation.** Private to the graph, so
    /// every path that puts a value in a region passes through here.
    ///
    /// A sealed id already in the destination's set is not a second hold — a hold set names a
    /// region at most once — which is what keeps the count in step with the wholesale release.
    fn mint(&mut self, into: u32, reach: &GraphReach<W>) {
        self.pins.hold(into, reach);
        for id in reach.sealed().iter() {
            if self.sealed_holds[into as usize].insert(id)
                && let Some(sealed_cell) = self.sealed.get_mut(id)
            {
                sealed_cell.holders += 1;
                #[cfg(test)]
                {
                    sealed_cell.peak_holders = sealed_cell.peak_holders.max(sealed_cell.holders);
                }
            }
        }
    }

    /// The price queries. Read-only, and crate-private until the dormant-carrier crossing verdict
    /// consumes them — the tests are their only caller today.
    ///
    /// Bytes the sealed region `id` still occupies, or `None` if nothing holds it any more.
    ///
    /// The slab is bounded by its cap; this tier is bounded only by what programs retain, so its
    /// occupancy is the number worth asking for
    /// ([graph/README.md § Bounding the two tiers](graph/README.md#bounding-the-two-tiers)).
    #[cfg(test)]
    pub(crate) fn sealed_retained_bytes(&self, id: SealedId) -> Option<usize> {
        self.sealed.get(id).map(SealedCell::retained_bytes)
    }

    /// The slice of each candidate's closure that no *other* candidate reaches — the marginal price
    /// of releasing one hold, with the part it shares with another candidate billed to neither.
    ///
    /// The walk spans both tiers, so a live cell a closure names is priced at its region and walked
    /// through in turn. An answer is [`frozen`](RetentionPrice::frozen) once no live cell is left
    /// in the whole closure, and a frozen answer is memoized on the sealed cell and reused forever
    /// — nothing inside a frozen closure can change, which is the argument the memo field carries.
    ///
    /// One answer per input position, `None` where the id is no longer in the tier; a repeated id
    /// gets the same answer at every position naming it. A single candidate's slice is its whole
    /// closure.
    ///
    /// Uniqueness is **relative to the candidate set**. A holder outside the set that also reaches
    /// a node is not discounted, so a candidate lying inside another candidate's closure is shared
    /// throughout and prices at zero — the honest marginal price of releasing both.
    #[cfg(test)]
    pub(crate) fn unique_retentions(&self, candidates: &[SealedId]) -> Vec<Option<RetentionPrice>> {
        let mut seen = SealedSet::new();
        let mut walks: Vec<(SealedId, TransitivePins)> = Vec::new();
        for id in candidates {
            if seen.insert(*id)
                && let Some(pins) = self.transitive_pins_of(*id)
            {
                walks.push((*id, pins));
            }
        }

        // How many candidates' closures each node lies in. A count of one is what makes it unique.
        // The slab half indexes by slot; the sparse half is an association list, scanned rather
        // than hashed — a candidate set is a handful of ids, and the crate keeps no hash map.
        let mut cell_count = vec![0u32; self.cap as usize];
        let mut sealed_count: Vec<(SealedId, u32)> = Vec::new();
        for (_, pins) in &walks {
            for slot in &pins.cells {
                cell_count[*slot as usize] += 1;
            }
            for id in &pins.sealed {
                match sealed_count.iter_mut().find(|(named, _)| named == id) {
                    Some((_, count)) => *count += 1,
                    None => sealed_count.push((*id, 1)),
                }
            }
        }
        let counted = |id: &SealedId| {
            sealed_count
                .iter()
                .find(|(named, _)| named == id)
                .map_or(0, |(_, count)| *count)
        };

        let priced: Vec<(SealedId, RetentionPrice)> = walks
            .iter()
            .map(|(id, pins)| {
                let cells: usize = pins
                    .cells
                    .iter()
                    .filter(|slot| cell_count[**slot as usize] == 1)
                    .map(|slot| self.cell_bytes(*slot))
                    .sum();
                let sealed: usize = pins
                    .sealed
                    .iter()
                    .filter(|inner| counted(inner) == 1)
                    .map(|inner| self.sealed_bytes(*inner))
                    .sum();
                (
                    *id,
                    RetentionPrice {
                        bytes: cells + sealed,
                        // The whole closure's, not the slice's: a live cell anywhere in it can
                        // still redraw the partition.
                        frozen: pins.cells.is_empty(),
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

    /// The tree pool, for the assertions that read a cell's chain links, its pledge and the
    /// tombstones hanging off it — none of which is observable through a public verb.
    #[cfg(test)]
    pub(crate) fn trees(&self) -> &TreePool<C> {
        &self.trees
    }

    /// Tree children of a slab cell that have not disposed.
    #[cfg(test)]
    pub(crate) fn tree_children_of(&self, handle: SlabHandle) -> u32 {
        self.slots[handle.slot() as usize].tree_children
    }

    /// The head of the tree tombstones hanging off a slab cell's bundle.
    #[cfg(test)]
    pub(crate) fn tree_tombstones_of(&self, handle: SlabHandle) -> Option<u32> {
        self.slots[handle.slot() as usize].tree_tombstones
    }

    /// Every tree tombstone head that hangs off a departed cell's handle rather than off a live
    /// slot.
    #[cfg(test)]
    pub(crate) fn relocated_tree_tombstones(&self) -> Vec<u32> {
        self.departed_tombstones
            .iter()
            .map(|(_, head)| *head)
            .collect()
    }

    /// How many relocation entries the graph carries in all — the figure that has to come back to
    /// zero once every departed cell's storage is gone.
    #[cfg(test)]
    pub(crate) fn relocations(&self) -> usize {
        self.relocated.iter().map(SmallVec::len).sum()
    }

    /// Where one departed handle's dormant carriers live now, straight off the map rather than
    /// through [`locate`](Self::locate)'s live-cell shortcut.
    #[cfg(test)]
    pub(crate) fn relocation_of(&self, handle: SlabHandle) -> Option<SlabForward> {
        self.relocation(handle).map(|entry| entry.location)
    }

    /// The handles on a sealed cell's lineage chain, collected. Chain order is reverse insertion,
    /// so a caller comparing more than one handle compares sets.
    #[cfg(test)]
    pub(crate) fn lineage_of(&self, id: SealedId) -> Vec<SlabHandle> {
        self.chain(
            self.sealed
                .get(id)
                .and_then(|sealed_cell| sealed_cell.lineage),
        )
    }

    /// The handles on a slot's lineage chain, collected.
    #[cfg(test)]
    pub(crate) fn slot_lineage(&self, slot: u32) -> Vec<SlabHandle> {
        self.chain(self.slots[slot as usize].lineage)
    }

    #[cfg(test)]
    fn chain(&self, head: Option<SlabHandle>) -> Vec<SlabHandle> {
        let mut walk = head;
        let mut handles = Vec::new();
        while let Some(handle) = walk {
            handles.push(handle);
            walk = self
                .relocation(handle)
                .expect("a chain entry is in its slot's list")
                .next;
        }
        handles
    }

    /// Every relocation entry, as the handle it answers for and where that handle now points.
    #[cfg(test)]
    pub(crate) fn relocation_entries(&self) -> Vec<(SlabHandle, SlabForward)> {
        self.relocated
            .iter()
            .enumerate()
            .flat_map(|(slot, list)| {
                list.iter().map(move |entry| {
                    (
                        SlabHandle::new(slot as u32, entry.generation),
                        entry.location,
                    )
                })
            })
            .collect()
    }

    /// Chunk bytes a live cell's region bundle occupies, absorbed bumps included. `0` for a cell
    /// that never allocated.
    #[cfg(test)]
    pub(crate) fn region_bytes(&self, handle: SlabHandle) -> Result<usize, Stale<SlabHandle>> {
        let slot = self.live_slot(handle)?;
        Ok(self.cell_bytes(slot))
    }

    /// How full both tiers are right now. The slab is bounded by its cap and the sealed tier by
    /// nothing, so an embedder ramps its copy-versus-hold threshold on these two numbers together.
    pub(crate) fn occupancy(&self) -> Occupancy {
        Occupancy {
            occupied: self.cap - self.free.len() as u32,
            cap: self.cap,
            sealed_cells: self.sealed.len(),
            retained_bytes: self.sealed.retained_bytes(),
        }
    }

    /// Chunk bytes of a placement destination's own region bundle, whichever habitat it lives in.
    fn destination_bytes(&self, dest: Destination) -> usize {
        match dest.home {
            CellHome::Slab(slot) => self.cell_bytes(slot),
            CellHome::Tree(index) => self.trees.region_bytes(index),
        }
    }

    /// Chunk bytes of the region in one slab slot, `0` where the slot never allocated.
    fn cell_bytes(&self, slot: u32) -> usize {
        self.slots[slot as usize]
            .region
            .as_ref()
            .map_or(0, Region::allocated_bytes)
    }

    /// Chunk bytes a sealed cell retains, `0` for an id no longer in the tier.
    fn sealed_bytes(&self, id: SealedId) -> usize {
        self.sealed.get(id).map_or(0, SealedCell::retained_bytes)
    }

    #[cfg(test)]
    fn bytes_of(&self, pins: &TransitivePins) -> usize {
        pins.cells
            .iter()
            .map(|slot| self.cell_bytes(*slot))
            .sum::<usize>()
            + pins
                .sealed
                .iter()
                .map(|id| self.sealed_bytes(*id))
                .sum::<usize>()
    }

    /// Walk from a sealed cell and memoize the result when it comes back frozen. `None` for an id
    /// no longer in the tier.
    #[cfg(test)]
    fn transitive_pins_of(&self, id: SealedId) -> Option<TransitivePins> {
        let sealed_cell = self.sealed.get(id)?;
        if let Some(memo) = sealed_cell.memo() {
            return Some(TransitivePins {
                cells: Vec::new(),
                sealed: memo.to_vec(),
            });
        }
        let pins = self.transitive_pins(GraphNode::Sealed(id), true);
        if pins.cells.is_empty() {
            self.sealed.prime(id, &pins.sealed);
        }
        Some(pins)
    }

    /// Record `id`'s frozen closure if it has one and has not been asked before, so a later walk
    /// folds the memo in rather than descending. Written once and never cleared: a closure that has
    /// frozen is walked exactly once for the graph's whole life.
    fn prime_memo(&self, id: SealedId, scratch: &Scratch) {
        let Some(sealed_cell) = self.sealed.get(id) else {
            return;
        };
        if sealed_cell.memo().is_some() {
            return;
        }
        // Priming wants the sealed-cell set and nothing else: a closure that names a live cell is
        // not frozen and is not recorded, so a cell only has to be noticed, and the set the walk
        // reports moves into the memo rather than being copied into it.
        let mut sealed_ids = scratch.vec();
        let mut frozen = true;
        let mut worklist = scratch.vec_with_capacity(1);
        worklist.push(id);
        self.walk(
            Bits::new(),
            worklist,
            Bits::new(),
            scratch.ids(),
            true,
            |node| match node {
                GraphNode::Slab(_) => frozen = false,
                GraphNode::Sealed(inner) => sealed_ids.push(inner),
            },
        );
        // The memo is durable and lands in the sealed cell's own region, so the bytes it costs are
        // bytes the sealed cell's price already counts.
        if frozen {
            self.sealed.prime(id, &sealed_ids);
        }
    }

    /// Every node of the hold graph reachable from `start`, `start` itself included, over both
    /// tiers. A ring terminates on the seen sets rather than looping.
    ///
    /// A sealed cell that already carries a memo *is* its own frozen closure, so with `use_memos`
    /// the walk folds the memo's sealed-cell set in instead of descending. The set is merged, never
    /// summed: two branches of one closure may share a sub-tier, and adding two memoized totals
    /// would bill the shared part twice. `use_memos` is false only where a test recomputes a memo
    /// from scratch to check it against what was recorded.
    #[cfg(test)]
    fn transitive_pins(&self, start: GraphNode, use_memos: bool) -> TransitivePins {
        let mut frontier = Bits::new();
        // The walkers run outside every verb, so they take the scratch region off the field
        // directly and leave it to the next verb's reset.
        let mut worklist = self.scratch_at_rest().vec();
        match start {
            GraphNode::Slab(slot) => {
                frontier.set(slot);
            }
            GraphNode::Sealed(id) => worklist.push(id),
        }
        let mut pins = TransitivePins {
            cells: Vec::new(),
            sealed: Vec::new(),
        };
        self.walk(
            frontier,
            worklist,
            Bits::new(),
            self.scratch_at_rest().ids(),
            use_memos,
            |node| match node {
                GraphNode::Slab(slot) => pins.cells.push(slot),
                GraphNode::Sealed(id) => pins.sealed.push(id),
            },
        );
        pins
    }

    /// The walk itself, reporting every node it reaches to `visit` exactly once.
    ///
    /// The working state is handed in, so each caller seeds it the way its question wants and
    /// nothing is built that the answer does not read. A node either seen set already names is
    /// neither reported nor descended through, which is how a marginal price prunes at what the
    /// destination already holds.
    ///
    /// The two tiers are walked in the shape each one is stored in. The slab half is a matrix, so
    /// its frontier and its seen set are rows and descending through a cell is one word-wise
    /// `frontier |= row & !seen` — no expansion of a row into per-node entries. Only the sealed
    /// half, which is sparse and unbounded, wants a worklist.
    fn walk<'s>(
        &self,
        mut frontier: Bits<W>,
        mut worklist: ScratchVec<'s, SealedId>,
        mut seen_cells: Bits<W>,
        mut seen_sealed: ScratchSet<'s>,
        use_memos: bool,
        mut visit: impl FnMut(GraphNode),
    ) {
        loop {
            if let Some(slot) = frontier.take_one() {
                if !seen_cells.set(slot) {
                    continue;
                }
                visit(GraphNode::Slab(slot));
                frontier.union_not_with(self.pins.row(slot), &seen_cells);
                worklist.extend(self.sealed_holds[slot as usize].iter());
                continue;
            }
            let Some(id) = worklist.pop() else { return };
            if !seen_sealed.insert(id) {
                continue;
            }
            visit(GraphNode::Sealed(id));
            let memo = use_memos
                .then(|| self.sealed.get(id).and_then(SealedCell::memo))
                .flatten();
            match memo {
                Some(memo) => {
                    for inner in memo {
                        if seen_sealed.insert(*inner) {
                            visit(GraphNode::Sealed(*inner));
                        }
                    }
                }
                None => {
                    if let Some(sealed_cell) = self.sealed.get(id) {
                        frontier.union_not_with(sealed_cell.aggregate.slab(), &seen_cells);
                        worklist.extend(sealed_cell.aggregate.sealed().iter());
                    }
                }
            }
        }
    }

    /// Bytes that pinning a value with reach `reach` into `dest` would **newly** keep alive,
    /// given `pinned` — the reach of everything already pinned into `dest` by this placement.
    ///
    /// The walk starts from the reach with `dest` itself, everything `dest`'s pin row names, every
    /// sealed cell `dest` holds, and both halves of `pinned` already marked as seen, so what the
    /// destination is answerable for anyway, or has just become answerable for, is billed to
    /// nobody. An operand homed in the destination, or in a cell the destination holds directly,
    /// prices at zero — and so does one whose whole reach an earlier operand of the same placement
    /// already brought in. The sum over a placement's operands is therefore what that placement
    /// newly retains, once, and the first operand from a shared source is the one shown the shared
    /// cost.
    ///
    /// The pruning is at *direct* holds, not at the destination's whole closure: a node the
    /// destination reaches only through a directly held node is still billed. The figure therefore
    /// only ever over-bills, which biases the verdict toward copying and never toward a pin whose
    /// cost the embedder was not shown.
    fn pin_price(
        &self,
        dest: u32,
        reach: &GraphReach<W>,
        pinned: &GraphReach<W>,
        scratch: &Scratch,
    ) -> usize {
        // Every seed already seen is a walk that reports nothing and a sum over nothing, so the
        // price is zero without building the walk's state at all. Priming a sealed cell's memo only
        // fills a cache no reading depends on, so skipping it changes no answer either.
        let row = self.pins.row(dest);
        let held = &self.sealed_holds[dest as usize];
        let covered = reach
            .slab_slots()
            .all(|slot| slot == dest || row.test(slot) || pinned.names(slot))
            && reach
                .sealed()
                .iter()
                .all(|id| held.contains(id) || pinned.names_sealed(id));
        if covered {
            return 0;
        }

        let mut seen_cells = *row;
        seen_cells.set(dest);
        seen_cells.union_with(pinned.slab());
        let mut seen_sealed = scratch.ids_from(held);
        seen_sealed.union_with(pinned.sealed());
        for id in reach.sealed().iter() {
            self.prime_memo(id, scratch);
        }
        let mut frontier = Bits::new();
        frontier.union_not_with(reach.slab(), &seen_cells);
        let mut worklist = scratch.vec_with_capacity(reach.sealed().len());
        worklist.extend(reach.sealed().iter());
        let mut bytes = 0;
        self.walk(frontier, worklist, seen_cells, seen_sealed, true, |node| {
            bytes += match node {
                GraphNode::Slab(slot) => self.cell_bytes(slot),
                GraphNode::Sealed(id) => self.sealed_bytes(id),
            };
        });
        bytes
    }

    /// Walk the hold graph from `start` and report a cycle if one is reachable — the fail-safe
    /// diagnostic for a ring, which keeps everything on it alive forever rather than dangling.
    ///
    /// Test-only, and **not consulted on any mint or release path**: preventing rings is the
    /// embedder's crossing discipline, not a mint-time reachability check. The walk spans both
    /// tiers, since a ring among live cells becomes a ring among sealed cells the moment they die,
    /// which is also why a seed may be a sealed region: every hold on one is an id, and a stale
    /// handle can no longer reach it.
    #[cfg(test)]
    pub(crate) fn debug_ring_from(&self, start: HoldNode) -> Option<Vec<HoldNode>> {
        let start = match start {
            HoldNode::Slab(handle) => GraphNode::Slab(handle.slot()),
            HoldNode::Sealed(id) => GraphNode::Sealed(id),
        };
        let mut path = Vec::new();
        let mut settled = std::collections::HashSet::new();
        self.walk_for_ring(start, &mut path, &mut settled)
            .map(|cycle| cycle.into_iter().map(|node| self.name(node)).collect())
    }

    #[cfg(test)]
    fn name(&self, node: GraphNode) -> HoldNode {
        match node {
            GraphNode::Slab(slot) => {
                HoldNode::Slab(SlabHandle::new(slot, self.slots[slot as usize].generation))
            }
            GraphNode::Sealed(id) => HoldNode::Sealed(id),
        }
    }

    /// What one node of the hold graph holds: for a cell, its two hold-set halves; for a sealed
    /// region, the two halves of its frozen aggregate.
    ///
    /// The expansion the test-only ring walk wants, which descends one node at a time and reports
    /// the path it took. The pricing walk descends the slab half a whole row at a time and never
    /// builds this.
    #[cfg(test)]
    fn holds_of(&self, node: GraphNode) -> Vec<GraphNode> {
        match node {
            GraphNode::Slab(slot) => self
                .pins
                .held_by(slot)
                .map(GraphNode::Slab)
                .chain(
                    self.sealed_holds[slot as usize]
                        .iter()
                        .map(GraphNode::Sealed),
                )
                .collect(),
            GraphNode::Sealed(id) => match self.sealed.get(id) {
                Some(sealed_cell) => sealed_cell
                    .aggregate
                    .slab_slots()
                    .map(GraphNode::Slab)
                    .chain(sealed_cell.aggregate.sealed().iter().map(GraphNode::Sealed))
                    .collect(),
                None => Vec::new(),
            },
        }
    }

    #[cfg(test)]
    fn walk_for_ring(
        &self,
        node: GraphNode,
        path: &mut Vec<GraphNode>,
        settled: &mut std::collections::HashSet<GraphNode>,
    ) -> Option<Vec<GraphNode>> {
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

    /// Where one operand's home sits relative to its destination — **the ancestry rule for a
    /// tree-homed operand**, applied at every placement door before the verdict.
    ///
    /// Into the home or a tree cell under it, the ordinary door: the destination dies first. Into
    /// an ancestor on the home's chain, or into its root: the verdict is consulted with the splice
    /// price, and a `Pin` pledges the home and every intermediate. Into anything else — a cousin
    /// under the same root, a cell under another root, an unrelated slab cell — a forced copy,
    /// because nothing outside the chain may outlive the home while borrowing it.
    ///
    /// The classification costs the level distance between home and destination, and two homes
    /// under different roots settle in O(1).
    fn classify_crossing(&self, home: CellHome, dest: Destination) -> Crossing {
        let CellHome::Tree(home) = home else {
            return Crossing::Ordinary;
        };
        match dest.home {
            CellHome::Slab(slot) => match slot == self.trees.root(home) {
                true => Crossing::Upward(Ancestor::Root),
                false => Crossing::Forced,
            },
            CellHome::Tree(dest) if self.trees.root(dest) != self.trees.root(home) => {
                Crossing::Forced
            }
            CellHome::Tree(dest) => match self.trees.ancestry(home, dest) {
                Ancestry::Under => Crossing::Ordinary,
                Ancestry::Above => Crossing::Upward(Ancestor::Tree(dest)),
                Ancestry::Apart => Crossing::Forced,
            },
        }
    }

    /// What an upward pin costs: the bundle bytes of the operand's home and of every intermediate
    /// up to the destination that is not already pledged at least that shallow.
    ///
    /// Marginal across the operands of one placement for free — the pledge is applied the moment a
    /// verdict comes back `Pin`, before the next operand is priced — so a second operand from the
    /// same home, or from a cell the first one's walk already pledged, is shown nothing.
    fn splice_price(&self, home: u32, dest: Ancestor) -> usize {
        self.trees
            .unpledged_up(home, dest)
            .map(|index| self.trees.region_bytes(index))
            .sum()
    }

    /// Price every operand against `dest`, put each price to the embedder's verdict, and hand back
    /// the union of the pinned reaches beside the answers.
    ///
    /// **Pledging is not a side note, which is why it is in the name.** An upward `Pin` is a
    /// promise the price was quoted for, so it is applied the moment that verdict comes back and
    /// before the next operand is priced — which is what makes a second operand from the same home
    /// marginal rather than double-billed, and what makes the sum over one placement's operands
    /// exact. The pledges an appraisal leaves stand whether or not the build that follows succeeds.
    ///
    /// **The one path every placement over operands takes**, so the destination-homed placement and
    /// the capturing successor consult the same verdict with the same numbers and hand the build
    /// the same shapes. Every operand is consulted, including one whose pin price is zero.
    ///
    /// Lives on the graph rather than on the step context because it reads nothing but the graph:
    /// keeping it here is what lets a door split its borrows and hand the scratch in beside them.
    fn appraise_and_pledge<'s, 'b, V>(
        &mut self,
        dest: Destination,
        operands: &[Operand<'_, 'b, V, W>],
        scratch: &'s Scratch,
    ) -> (GraphReach<W>, &'s [Verdict])
    where
        V: Reattachable + DropFree,
    {
        let mut reach = GraphReach::empty();
        // The tiers read once: they do not change between operands.
        let Occupancy {
            occupied,
            cap,
            sealed_cells,
            retained_bytes,
        } = self.occupancy();
        // The answers alone. The operands are still to hand where the views are built, so carrying
        // their erased forms through here would be a second copy of a slice the caller already has.
        let verdicts = scratch.slice_with(operands.len(), |index| {
            let operand = &operands[index];
            let home = operand.carrier.home();
            let reaching = self.classify_crossing(home, dest);
            if reaching == Crossing::Forced {
                // The verdict is not consulted: no pin of this operand into this destination
                // typechecks, so there is no choice to put to the embedder.
                return Verdict::Copy;
            }
            let pin_bytes = match reaching {
                Crossing::Upward(pledge) => {
                    let CellHome::Tree(home) = home else {
                        unreachable!("only a tree-homed operand reaches upward")
                    };
                    self.splice_price(home, pledge)
                }
                // Priced against what this placement has already pinned as well as what the
                // destination held before it, so a second operand homed in the same source as the
                // first is shown the marginal cost and the sum over the operands is exact.
                _ => self.pin_price(dest.mint_slot, operand.carrier.reach(), &reach, scratch),
            };
            let prices = Prices {
                // Priced against what this placement has already pinned as well as what the
                // destination held before it, so a second operand homed in the same source as the
                // first is shown the marginal cost and the sum over the operands is exact.
                pin_bytes,
                copy_bytes: operand.copy_bytes,
                occupied,
                cap,
                sealed_cells,
                retained_bytes,
                destination_bytes: self.destination_bytes(dest),
            };
            let verdict = (self.verdict)(prices);
            if verdict == Verdict::Pin {
                reach.union_with(operand.carrier.reach());
                // The promise the price was quoted for, made before the next operand is priced, so
                // the walk it shares with this one is billed once.
                if let (Crossing::Upward(pledge), CellHome::Tree(home)) = (reaching, home) {
                    self.trees.pledge_up(home, pledge);
                }
            }
            verdict
        });
        (reach, verdicts)
    }

    /// The one path from a built value into a region: fold `reach` into the destination's hold
    /// set, write the value there, and hand back the carrier that pairs it with its own reach —
    /// the destination's bit plus everything the operands reached. The pair is bundled before it
    /// is returned, so no loose value-plus-reach exists anywhere.
    ///
    /// The carrier's brand is free here and fixed by the door that calls it: every caller is a
    /// [`StepContext`] method whose return type names the step's own brand.
    fn mint_and_build<'b, T>(
        &mut self,
        dest: Destination,
        mut reach: GraphReach<W>,
        build: impl for<'r> FnOnce(Writer<'r>) -> T::At<'r>,
    ) -> Ready<'b, T, W>
    where
        T: Reattachable + DropFree,
    {
        // A bump releases its chunks whole and never walks a value, so a family with drop glue
        // would leak whatever it owns. `DropFree` declares the absence; this is the check.
        const { assert!(!std::mem::needs_drop::<T::At<'static>>()) };
        self.mint(dest.mint_slot, &reach);
        let value = {
            let region = match dest.home {
                CellHome::Slab(slot) => self.slots[slot as usize]
                    .region
                    .get_or_insert_with(Region::new),
                CellHome::Tree(index) => self.trees.region_mut(index),
            };
            Erased::<T>::erase(build(region.writer()))
        };
        // The mint slot, not the home: a value homed in a tree cell reaches its root, which is what
        // every hold on its behalf was minted into.
        reach.add(dest.mint_slot);
        Ready::new(value, reach, dest.home)
    }

    /// The scratch region as the test-only walkers reach it: they run outside every verb, so the
    /// scratch region is on the graph and their working state goes in it like a verb's would.
    /// Nothing resets it until the next verb, which is why no walker is on a measured path.
    #[cfg(test)]
    fn scratch_at_rest(&self) -> &Scratch {
        self.scratch
            .as_ref()
            .expect("a walker runs outside every verb")
    }

    /// Whether `holder` holds `held` in the pin relation.
    #[cfg(test)]
    fn holds(&self, holder: SlabHandle, held: SlabHandle) -> bool {
        self.pins.test(holder.slot(), held.slot())
    }
}

/// The view of the graph a step gets: its own cell's continuation slot, the cell-region doors, and
/// its own identity.
///
/// **Two brands, and no outlives relation between them.**
///
/// `'b` is the step's: the lifetime of the graph borrow the enclosing
/// [`enter`](CellGraph::enter) holds, and what every carrier this step's doors hand back is
/// branded to. Nothing carrying it escapes the call.
///
/// `'cell` is the executing cell's. A reference at `'cell` names storage the cell's hold set
/// covers for the cell's whole life: its own region, or a region a pinned crossing into this cell
/// has minted into its holds. It is **invariant** — the `PhantomData` below — and quantified per
/// `enter`, so a step can neither widen it nor let a reference at it out; and a foreign carrier's
/// [`read`](Self::read) lands at a borrow strictly inside the step, which cannot coerce to it. The
/// one door from a `'cell` reference into anything that outlives the step is
/// [`lift`](Self::lift).
pub struct StepContext<'b, 'cell, C: Reattachable, const W: usize = 1> {
    graph: &'b mut CellGraph<C, W>,
    /// The cell this step is running in, of either kind.
    cell: CellHandle,
    /// The executing cell's write surface, minted once at `enter`. A `Copy` field, so
    /// [`writer`](Self::writer) is a read rather than a door onto the region.
    writer: Writer<'cell>,
    /// The graph's scratch region, held here for the length of the step and handed back by `Drop`.
    /// A door splits this off the graph borrow so a transient and a `&mut CellGraph` coexist.
    ///
    /// An `Option` so the hand-back is a move out and not a swap against a fresh region: minting
    /// one to leave behind would be the one allocation the step machinery does not need.
    scratch: Option<Scratch>,
    _cell: PhantomData<fn(&'cell ()) -> &'cell ()>,
}

impl<'b, 'cell, C: Reattachable, const W: usize> StepContext<'b, 'cell, C, W> {
    /// The cell this step is running in, of either kind.
    pub fn cell(&self) -> CellHandle {
        self.cell
    }

    /// The slab slot whose relations this step's placements write: the executing cell itself when
    /// it is a slab cell, and its root when it is a tree cell.
    fn executing_slot(&self) -> u32 {
        match self.cell {
            CellHandle::Slab(handle) => handle.slot(),
            CellHandle::Tree(handle) => self.graph.trees.root(handle.index()),
        }
    }

    /// The executing cell as a placement destination — where [`alloc`](Self::alloc) builds and what
    /// a successor store writes into.
    fn executing_dest(&self) -> Destination {
        match self.cell {
            CellHandle::Slab(handle) => Destination {
                mint_slot: handle.slot(),
                home: CellHome::Slab(handle.slot()),
            },
            CellHandle::Tree(handle) => Destination {
                mint_slot: self.graph.trees.root(handle.index()),
                home: CellHome::Tree(handle.index()),
            },
        }
    }

    /// The executing cell's own write surface, at the cell brand.
    ///
    /// **This is the own-region write.** It takes no closure and no build: a value written here is
    /// a plain `&'cell` reference, needing no carrier, because its reach is the executing cell and
    /// the cell keeps itself. The writer is minted once, at `enter`, so this is a field read — a
    /// step may take it as many times as it likes, and one taken before a door still writes after
    /// it.
    ///
    /// ```
    /// use cellgraph::{CellGraph, Verdict, reattachable};
    /// struct Work;
    /// reattachable!(Work => String);
    ///
    /// let mut graph: CellGraph<Work> = CellGraph::new(2, |_| Verdict::Pin);
    /// let cell = graph.create(None, None).unwrap();
    /// let read = graph
    ///     .enter(cell, |context| {
    ///         let counters = context.writer().fill(3, |index| index as u32);
    ///         // Written a moment ago, and embeddable in what is written next.
    ///         let spine: &[&u32] = context.writer().fill(1, |_| &counters[2]);
    ///         *spine[0]
    ///     })
    ///     .unwrap();
    /// assert_eq!(read, 2);
    /// ```
    ///
    /// A foreign carrier's read cannot be embedded here. It arrives at a borrow strictly inside the
    /// step, which has no outlives relation to `'cell`, so the only reference that lands in a
    /// cell's region without passing the crossing verdict is one into that same region. The brand
    /// is what carries this: `Writer` is covariant, so a write may always be taken at a *shorter*
    /// brand than `'cell` — and a run written at a shorter one is unreadable past it, so nothing
    /// it holds can be named again. What cannot happen is the reverse, a foreign read reaching
    /// `'cell`, which is where every door that outlives the step asks for its value:
    ///
    /// ```compile_fail
    /// use cellgraph::{CellGraph, DropFree, Verdict, reattachable};
    /// struct Work;
    /// reattachable!(Work => String);
    /// struct Number;
    /// reattachable!(Number => &'r u32);
    /// impl DropFree for Number {}
    /// struct Spine;
    /// reattachable!(Spine => &'r [&'r u32]);
    /// impl DropFree for Spine {}
    ///
    /// let mut graph: CellGraph<Work> = CellGraph::new(2, |_| Verdict::Pin);
    /// let cell = graph.create(None, None).unwrap();
    /// let other = graph.create(None, None).unwrap();
    /// graph
    ///     .enter(cell, |context| {
    ///         let foreign = context
    ///             .alloc_into::<Number, Number>(other, &[], |writer, _| &writer.fill(1, |_| 41)[0])
    ///             .unwrap();
    ///         let read = context.read(&foreign);
    ///         let spine = context.writer().fill(1, |_| read.value());
    ///         // The spine is only at `'cell` if the read's borrow is, and it is not.
    ///         context.lift::<Spine>(spine);
    ///     })
    ///     .unwrap();
    /// ```
    ///
    /// And a `'cell` reference cannot leave the `enter` that minted it: `R` is chosen outside the
    /// call, so it cannot name the brand.
    ///
    /// ```compile_fail
    /// use cellgraph::{CellGraph, Verdict, reattachable};
    /// struct Work;
    /// reattachable!(Work => String);
    ///
    /// let mut graph: CellGraph<Work> = CellGraph::new(2, |_| Verdict::Pin);
    /// let cell = graph.create(None, None).unwrap();
    /// let escaped: Option<&u32> = graph
    ///     .enter(cell, |context| Some(&context.writer().fill(1, |_| 41u32)[0]))
    ///     .unwrap();
    /// ```
    pub fn writer(&self) -> Writer<'cell> {
        self.writer
    }

    /// Take the cell's continuation, re-anchored at the cell brand.
    ///
    /// **This is the sealed tier's accessor.** A continuation captured over values in cells that
    /// have since sealed comes back reading storage those sealed cells still retain. The door hangs
    /// on the step context and nowhere else, so a value reaching sealed storage is only ever live
    /// inside an `enter` scope.
    ///
    /// The slot is left empty: a continuation is one-shot, and a step that wants the cell entered
    /// again stores a successor. A multi-shot embedder takes its continuation, runs it, and stores
    /// it back — which costs nothing, since a store prices nothing.
    ///
    /// There is no graph-level twin, so a value reaching sealed storage cannot be read from
    /// outside a step:
    ///
    /// ```compile_fail
    /// use cellgraph::{CellGraph, Verdict, reattachable};
    /// struct Work;
    /// reattachable!(Work => String);
    ///
    /// let mut graph: CellGraph<Work> = CellGraph::new(2, |_| Verdict::Pin);
    /// let cell = graph.create(None, None).unwrap();
    /// let _ = graph.continuation(cell);
    /// ```
    pub fn continuation(&mut self) -> Option<C::At<'cell>> {
        let stored = match self.cell {
            CellHandle::Slab(handle) => {
                self.graph.slots[handle.slot() as usize].continuation.take()
            }
            CellHandle::Tree(handle) => self.graph.trees.take_continuation(handle.index()),
        }?;
        // SAFETY: the value came in through `store_successor` at some earlier step's `'cell`, so
        // its referents are storage that brand covers: this cell's own region, whose chunks are
        // pointer-stable and which a seal detaches and a merge absorbs unmoved, or a region a
        // pinned crossing minted into this cell's hold set — a live cell, or a sealed cell the
        // hold has followed into the tier, both of which keep their chunks. The cell is live for
        // all of this step (it is the one executing) and its holds are monotone for its life, so
        // every one of them is still there. For a tree step the hold set is the root's, and a
        // capture's own storage is the root's, one of the cells it holds, or a tree cell on this
        // one's chain — an ancestor, which outlives it, or the executing cell itself. `'cell` is
        // quantified by the enclosing `enter` and nameable nowhere outside it, which discharges
        // the invariant-family condition: nothing anchored at it escapes the step.
        Some(unsafe { stored.reattach::<'cell>() })
    }

    /// Store the continuation the next step of this cell receives.
    ///
    /// **The store prices nothing and records no reach.** A continuation's captures are `'cell`
    /// references, and `'cell` names only storage this cell's hold set already covers: its own
    /// region, which the cell keeps by being alive, or a region a pinned crossing minted into its
    /// holds when the reference arrived through [`alloc_here`](Self::alloc_here). So there is
    /// nothing left for a store to appraise, and no mask for the seal transition to rewrite —
    /// a cell's continuation takes no reach-table entry.
    ///
    /// A continuation that captures nothing is the same door: `'cell` is satisfied by an owned
    /// value or an `&'static` one just as it is by a region reference.
    pub fn store_successor(&mut self, continuation: C::At<'cell>) {
        let stored = Erased::<C>::erase(continuation);
        match self.cell {
            CellHandle::Slab(handle) => {
                self.graph.slots[handle.slot() as usize].continuation = Some(stored)
            }
            CellHandle::Tree(handle) => self
                .graph
                .trees
                .set_continuation(handle.index(), Some(stored)),
        }
    }

    /// The own-cell crossing: price `operands` into the executing cell, mint what pins into its
    /// hold set, and run `build` with the cell's own writer beside the views.
    ///
    /// This is how a value homed elsewhere becomes reachable at `'cell`. A pinned view arrives as
    /// `V::At<'cell>` and **may leave the build** — that is exactly what the pin bought, since its
    /// reach went into this cell's holds before `build` ran, so the cell keeps that storage for as
    /// long as it lives. A copied view arrives severed, at a brand bound by the call, so embedding
    /// one or handing it back is a compile error and the only copy that typechecks is a deep one
    /// through the writer.
    ///
    /// `build` returns whatever it returns: a `'cell` value needs no carrier and no family. A
    /// build that uses no writer — `alloc_here(&[operand], |_, views| pinned(&views[0]))` — is the
    /// "pin this into my holds and give me the borrow" operation.
    ///
    /// An own-region write with nothing to cross does not come here at all: that is
    /// [`writer`](Self::writer). A `'cell` reference is not an operand either — it crosses nothing,
    /// so there is no price to take.
    ///
    /// Operands are priced in the order they are given, each against what the ones before it have
    /// already pinned.
    ///
    /// ```
    /// use cellgraph::{CellGraph, CrossedOperand, DropFree, Operand, Verdict, reattachable};
    /// struct Work;
    /// reattachable!(Work => String);
    /// struct Number;
    /// reattachable!(Number => &'r u32);
    /// impl DropFree for Number {}
    ///
    /// let mut graph: CellGraph<Work> = CellGraph::new(2, |_| Verdict::Pin);
    /// let cell = graph.create(None, None).unwrap();
    /// let other = graph.create(None, None).unwrap();
    /// let read = graph
    ///     .enter(cell, |context| {
    ///         let foreign = context
    ///             .alloc_into::<Number, Number>(other, &[], |writer, _| &writer.fill(1, |_| 41)[0])
    ///             .unwrap();
    ///         let pinned: &u32 = context.alloc_here(
    ///             &[Operand { carrier: &foreign, copy_bytes: usize::MAX }],
    ///             |_writer, views| match views[0] {
    ///                 CrossedOperand::Pinned(value) => value,
    ///                 CrossedOperand::Copied(_) => unreachable!("the verdict always pins"),
    ///             },
    ///         );
    ///         *pinned
    ///     })
    ///     .unwrap();
    /// assert_eq!(read, 41);
    /// ```
    pub fn alloc_here<R, V>(
        &mut self,
        operands: &[Operand<'_, 'b, V, W>],
        build: impl for<'v> FnOnce(Writer<'cell>, &[CrossedOperand<'cell, 'v, V>]) -> R,
    ) -> R
    where
        V: Reattachable + DropFree,
        Erased<V>: Copy,
    {
        // The scratch splits off the graph borrow: the verdicts and the views live in it while the
        // appraisal below holds the graph exclusively.
        let dest = self.executing_dest();
        let StepContext {
            graph,
            writer,
            scratch,
            ..
        } = &mut *self;
        let scratch = scratch
            .as_ref()
            .expect("a step holds the scratch region for its whole length");
        let (reach, verdicts) = graph.appraise_and_pledge(dest, operands, scratch);
        // Nothing is placed here — no value is erased and no carrier comes back — so the mint the
        // placement path performs on its way into a region happens directly. The self rule makes
        // the executing cell's own bit a no-op, so an operand already homed here costs no hold.
        graph.mint(dest.mint_slot, &reach);
        // SAFETY: see `reanchor_operands`. Every pinned operand's reach is now in the executing
        // cell's hold set, so the storage it names is kept for as long as the cell lives — which
        // covers `'cell` — and the copied views live only for the `build` call, whose `for<'v>`
        // quantifier keeps one from escaping it.
        let views = unsafe { reanchor_operands(operands, verdicts, scratch) };
        build(*writer, views)
    }

    /// Destination-homed placement: build a value **in `dest`'s region**, embedding the views of
    /// `operands`, and fold every operand's reach into `dest`'s hold set.
    ///
    /// This is the push shape of [../README.md § Passing values between
    /// cells](../README.md#passing-values-between-cells): the producer builds straight
    /// into the consumer, the consumer's row takes the reach, and the producer can then die.
    /// Operands share one family `V` and arrive as carriers, never as values beside a mask.
    ///
    /// Operands are priced in the order they are given, each against what the ones before it have
    /// already pinned: the first operand from a shared source carries the shared cost and the rest
    /// price at the margin, so the prices sum to what the placement newly retains rather than
    /// billing a shared source once per operand.
    pub fn alloc_into<T, V>(
        &mut self,
        dest: impl Into<CellHandle>,
        operands: &[Operand<'_, 'b, V, W>],
        build: impl for<'r, 'v> FnOnce(Writer<'r>, &[CrossedOperand<'r, 'v, V>]) -> T::At<'r>,
    ) -> Result<Ready<'b, T, W>, Stale<CellHandle>>
    where
        T: Reattachable + DropFree,
        V: Reattachable + DropFree,
        Erased<V>: Copy,
    {
        // The scratch splits off the graph borrow: the crossed operands and the views live in it
        // while the placement below holds the graph exclusively.
        let StepContext { graph, scratch, .. } = &mut *self;
        let scratch = scratch
            .as_ref()
            .expect("a step holds the scratch region for its whole length");
        let dest = match dest.into() {
            CellHandle::Slab(handle) => {
                let slot = graph.live_slot(handle).map_err(Stale::<CellHandle>::from)?;
                Destination {
                    mint_slot: slot,
                    home: CellHome::Slab(slot),
                }
            }
            CellHandle::Tree(handle) => {
                let index = graph
                    .trees
                    .live_index(handle)
                    .map_err(Stale::<CellHandle>::from)?;
                Destination {
                    mint_slot: graph.trees.root(index),
                    home: CellHome::Tree(index),
                }
            }
        };
        let (reach, verdicts) = graph.appraise_and_pledge(dest, operands, scratch);
        Ok(graph.mint_and_build(dest, reach, move |writer| {
            // SAFETY: see `reanchor_operands`. `mint_and_build` has already folded every pinned
            // operand's reach into the destination's hold set before it calls this closure, so
            // that storage outlives both `'r` and the destination.
            let views = unsafe { reanchor_operands(operands, verdicts, scratch) };
            build(writer, views)
        }))
    }

    /// The bridge from an own-region value to a carrier, reaching the executing cell.
    ///
    /// A `'cell` value is carrier-free while it stays in the cell that built it; it needs a carrier
    /// the moment it must be an operand, be [`keep`](Self::keep)ed, or be built into another cell.
    /// The reach it takes on is the executing cell alone, which covers its referents: they are the
    /// cell's own region, or storage a pinned crossing put in the cell's hold set, and a hold on a
    /// cell keeps its hold set with it — through the cell's death, since a seal keeps the whole
    /// aggregate.
    ///
    /// The consequence a consumer sees is that the hold it takes on a lifted value is a **direct**
    /// one on the producer. A [`redeem`](Self::redeem) of something homed in a cell the producer
    /// merely holds is `Unheld` from the consumer — a refusal, never a dangle.
    ///
    /// ```
    /// use cellgraph::{CellGraph, DropFree, Verdict, reattachable};
    /// struct Work;
    /// reattachable!(Work => String);
    /// struct Number;
    /// reattachable!(Number => &'r u32);
    /// impl DropFree for Number {}
    ///
    /// let mut graph: CellGraph<Work> = CellGraph::new(2, |_| Verdict::Pin);
    /// let cell = graph.create(None, None).unwrap();
    /// let read = graph
    ///     .enter(cell, |context| {
    ///         let value = context.lift::<Number>(&context.writer().fill(1, |_| 41u32)[0]);
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
    /// use cellgraph::{CellGraph, DropFree, Ready, Verdict, reattachable};
    /// struct Work;
    /// reattachable!(Work => String);
    /// struct Number;
    /// reattachable!(Number => &'r u32);
    /// impl DropFree for Number {}
    ///
    /// let mut graph: CellGraph<Work> = CellGraph::new(2, |_| Verdict::Pin);
    /// let cell = graph.create(None, None).unwrap();
    /// let escaped: Ready<'_, Number> = graph
    ///     .enter(cell, |context| {
    ///         context.lift::<Number>(&context.writer().fill(1, |_| 41u32)[0])
    ///     })
    ///     .unwrap();
    /// ```
    pub fn lift<T>(&self, value: T::At<'cell>) -> Ready<'b, T, W>
    where
        T: Reattachable + DropFree,
    {
        // A bump releases its chunks whole and never walks a value, so a family with drop glue
        // would leak whatever it owns. `DropFree` declares the absence; this is the check, the
        // same one `mint_and_build` makes at the placement doors.
        const { assert!(!std::mem::needs_drop::<T::At<'static>>()) };
        // The mint slot, not the home: a value homed in a tree cell reaches its root, which is what
        // every hold on its behalf is minted into.
        let dest = self.executing_dest();
        Ready::new(
            Erased::<T>::erase(value),
            GraphReach::single(dest.mint_slot),
            dest.home,
        )
    }

    /// Mint a bare hold on another live cell — the pull shape's first half: the executing cell
    /// takes a hold with no value crossing, so the held cell seals rather than reclaims when it
    /// dies, and this cell can read out of it later.
    pub fn hold(&mut self, other: SlabHandle) -> Result<(), Stale<SlabHandle>> {
        let other_slot = self.graph.live_slot(other)?;
        let reach = GraphReach::single(other_slot);
        let into = self.executing_slot();
        self.graph.mint(into, &reach);
        Ok(())
    }

    /// Put a carrier to rest: register its reach in its home cell's reach table and hand back
    /// the lifetime-free form an embedder may keep between steps.
    ///
    /// The value is neither read nor moved — it stays where the placement wrote it. What changes is
    /// where its reach lives: off the carrier, which dies with this step, and into the reach table,
    /// where the seal transition rewrites it as the cells it names seal. The
    /// [`Dormant`](crate::Dormant) that comes back names that entry and carries no mask of its own,
    /// so nothing pairs a value with a reach outside the reach table.
    pub fn keep<T>(&mut self, carrier: Ready<'b, T, W>) -> Dormant<T>
    where
        T: Reattachable + DropFree,
    {
        let (value, reach, home) = carrier.into_parts();
        let key = match home {
            CellHome::Slab(slot) => {
                let cell = &mut self.graph.slots[slot as usize];
                let index = cell.reaches.intern(reach);
                DormantKey {
                    home: CellHandle::Slab(SlabHandle::new(slot, cell.generation)),
                    index,
                }
            }
            // A tree cell has no reach table and interns nothing: the value reaches its root and
            // nothing else, so the redeem derives that reach rather than reading one back. What the
            // keep does record is that the cell is now nameable, which is what decides whether its
            // death leaves a tombstone.
            CellHome::Tree(index) => {
                self.graph.trees.mark_kept(index);
                DormantKey {
                    home: CellHandle::Tree(self.graph.trees.occupant(index)),
                    index: 0,
                }
            }
        };
        Dormant::new(value, key)
    }

    /// Redeem an at-rest carrier into this step, or refuse.
    ///
    /// This is the door both crossing shapes of [../README.md § Passing values between
    /// cells](../README.md#passing-values-between-cells) complete through: a value the
    /// producer built into the consumer comes back in the consumer's own later step, and a value
    /// the consumer held its producer for comes back after the producer sealed.
    ///
    /// The executing cell must be entitled: it is the home, or its pin row or birth row names the
    /// home — both keep the home in the slab with its storage intact — or the home sealed into a
    /// sealed cell this cell holds. A value redeemed out of a sealed cell comes back reaching that
    /// sealed cell's id alone, which covers: a hold on a sealed cell keeps its whole aggregate
    /// alive transitively.
    pub fn redeem<T>(&self, dormant: Dormant<T>) -> Result<Ready<'b, T, W>, RedeemError>
    where
        T: Reattachable + DropFree,
    {
        let graph = &*self.graph;
        let executing = self.executing_slot();
        let key = dormant.key();
        // Decided before the value is touched: a dormant carrier rests as bytes precisely so that a
        // refusal costs nothing, including when the storage those bytes name is gone.
        // A tree home resolves through its tombstone chain first: one hop per splice its bytes have
        // been through since the keep, ending at a tree cell that still holds them or at the slab
        // handle the relocation map answers for from there.
        let slab_home = match key.home {
            CellHandle::Slab(handle) => handle,
            CellHandle::Tree(handle) => match graph.trees.resolve(handle) {
                None => return Err(RedeemError::Gone),
                Some(TreeForward::Tree(index)) => {
                    // Entitled by root identity, which is O(1): a slab cell is its own root, and
                    // every hold a value homed in a tree cell can take points up its own chain.
                    if graph.trees.root(index) != executing {
                        return Err(RedeemError::Unheld);
                    }
                    // SAFETY: the home is a tree cell under this step's own root, live or dead
                    // but undisposed, so its region is still there and nothing dies inside a step.
                    let value = unsafe { dormant.take() };
                    let reach = GraphReach::single(graph.trees.root(index));
                    return Ok(Ready::new(value, reach, CellHome::Tree(index)));
                }
                Some(TreeForward::Slab(handle)) => handle,
            },
        };
        let (reach, home) = match graph.locate(slab_home) {
            None => return Err(RedeemError::Gone),
            Some(SlabForward::Slab { slot, first_index }) => {
                let entitled = slot == executing
                    || graph.pins.test(executing, slot)
                    || graph.birth.test(executing, slot);
                if !entitled {
                    return Err(RedeemError::Unheld);
                }
                // A key that started in a tree cell indexes no reach table: its value reached the
                // root alone, and the slot the chain ends at is where those bytes are now.
                let reach = match key.home {
                    CellHandle::Tree(_) => GraphReach::single(slot),
                    CellHandle::Slab(_) => graph.slots[slot as usize]
                        .reaches
                        .get(first_index + key.index)
                        .expect("a relocated key names an entry of the reach table it landed in")
                        .clone(),
                };
                (reach, CellHome::Slab(slot))
            }
            Some(SlabForward::Sealed(id)) => {
                if !graph.sealed_holds[executing as usize].contains(id) {
                    return Err(RedeemError::Unheld);
                }
                // The sealed cell's id alone: a hold on it keeps its aggregate alive transitively,
                // and a mask naming no slab bit has nothing that can go stale under a later `keep`.
                (GraphReach::single_sealed(id), CellHome::Slab(executing))
            }
        };
        // SAFETY: the match above resolved the key's home to storage that is still there — a live
        // slab slot this cell is the home of, holds, or descends from, or a sealed cell it holds —
        // so the referents parked in those bytes are live for the whole step, which is the contract
        // `take` asks for.
        let value = unsafe { dormant.take() };
        Ok(Ready::new(value, reach, home))
    }

    /// Read a carrier out at the reading borrow. The door hangs on the context, so a value with
    /// reach is only ever live inside an `enter` scope.
    pub fn read<'s, T>(&'s self, carrier: &'s Ready<'b, T, W>) -> Active<'s, T>
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
        Active::new(value)
    }
}

/// Re-anchor each crossed operand at the brand its verdict allows: the destination's region brand
/// for a pin, an unrelated one for a copy.
///
/// # Safety
///
/// Every operand is a carrier branded to the executing step, so its referents are region storage
/// that is live for the whole step — nothing dies inside one, since `release` needs the graph and
/// `enter` holds it exclusively — and a pinned operand's reach has additionally been minted into
/// the destination's hold set before this runs. The views live only for the build call, and the
/// caller's `for<'r, 'v>` quantifier keeps one from escaping it.
unsafe fn reanchor_operands<'r, 'v, 's, V, const W: usize>(
    operands: &[Operand<'_, '_, V, W>],
    verdicts: &[Verdict],
    scratch: &'s Scratch,
) -> &'s [CrossedOperand<'r, 'v, V>]
where
    V: Reattachable + DropFree,
    Erased<V>: Copy,
{
    scratch.slice_with(verdicts.len(), |index| {
        let erased = operands[index].carrier.erased();
        match verdicts[index] {
            // SAFETY: see the function contract.
            Verdict::Pin => CrossedOperand::Pinned(unsafe { erased.reattach::<'r>() }),
            // SAFETY: see the function contract.
            Verdict::Copy => CrossedOperand::Copied(unsafe { erased.reattach::<'v>() }),
        }
    })
}

/// The scratch region, off the graph for the length of a verb that is not a step.
///
/// `enter` parks it on the [`StepContext`] it builds; a `release` cascades outside any step and
/// parks it here for the same reason, and hands it back the same way. The take leaves `None`
/// behind, so a hand-back written after the cascade would be the one thing a panic in it skips,
/// and every later verb would then fail on the missing scratch region rather than on the original
/// fault.
struct Parked<'t, C: Reattachable, const W: usize> {
    graph: &'t mut CellGraph<C, W>,
    scratch: Option<Scratch>,
}

impl<C: Reattachable, const W: usize> Parked<'_, C, W> {
    /// Run one verb's body against the graph and the scratch region parked off it.
    fn run(&mut self, body: impl FnOnce(&mut CellGraph<C, W>, &Scratch)) {
        let Parked { graph, scratch } = self;
        let scratch = scratch
            .as_ref()
            .expect("a verb holds the scratch region for its whole length");
        body(graph, scratch);
    }
}

impl<C: Reattachable, const W: usize> Drop for Parked<'_, C, W> {
    fn drop(&mut self) {
        self.graph.scratch = self.scratch.take();
    }
}

impl<C: Reattachable, const W: usize> Drop for StepContext<'_, '_, C, W> {
    fn drop(&mut self) {
        match self.cell {
            CellHandle::Slab(handle) => {
                self.graph.executing.clear(handle.slot());
            }
            CellHandle::Tree(handle) => self.graph.trees.set_executing(handle.index(), false),
        }
        // Back on the graph, chunk and all, so the next verb starts warm — and so a panicking step
        // hands it back exactly as an ordinary one does.
        self.graph.scratch = self.scratch.take();
    }
}
