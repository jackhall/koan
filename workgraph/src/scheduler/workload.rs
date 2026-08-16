use std::rc::Rc;

use super::{DropFree, EdgeId, Reattachable};
use crate::witnessed::{
    Carrier, Delivered, PinsRegion, Region, RegionHandleFamily, RegionOwner, Retained,
    StorageProfile, Witnessed,
};

/// The live (caller-lifetime) form of the inter-node value for a workload `W`, re-anchored from the
/// scheduler's `Witnessed<W::Value, _>` slot at the borrow under which the producer frame stays
/// pinned. `Live<'node, W>` is what a slot read hands back and what `finalize` is given.
pub type Live<'node, W> = <<W as Workload>::Value as Reattachable>::At<'node>;

/// The per-slot memory anchor's contract: the scheduler holds one `Rc<W::Frame>` per slot, never
/// inspects it beyond this projection, and projects its region owner where retention and delivery
/// need the true owner type. Koan's anchor is the per-call slot frame; its owner is `FrameStorage`.
pub trait Anchor: 'static {
    /// The projected region-owner type — the delivery-envelope and reach-set member type. Koan's
    /// is `FrameStorage`. [`PinsRegion`] is the reach-set member contract; the
    /// scheduler retains and drops the `Rc` but calls no method on it.
    type Owner: PinsRegion + 'static;
    /// The anchor's region owner, projected for retention and delivery.
    fn owner(&self) -> &Rc<Self::Owner>;
}

/// The anchor's projected region-owner type — the delivery-envelope and reach-set member type.
pub type OwnerOf<W> = <<W as Workload>::Frame as Anchor>::Owner;

/// A finalized terminal as the workload's finalize hook delivers it: the erased inter-node value
/// bundled with the reference-only [`Carrier`] naming the regions it reaches (empty for a
/// frameless / run-region value). The store seals it for dormant storage between steps.
pub type Terminal<W> = Witnessed<<W as Workload>::Value, Carrier<OwnerOf<W>>>;

/// A finalized terminal in its dormant [`Retained`] form — what a delivered edge holds and what
/// [`Scheduler::edge_resident`](super::Scheduler::edge_resident) duplicates, read back under the
/// destination region's own pin.
pub type SealedTerminal<W> = Retained<<W as Workload>::Value, Carrier<OwnerOf<W>>>;

/// A terminal **in transit**: the carrier bundled with the owned coverage pinning every region it
/// reaches, its own residence among them. This is the currency of the terminal door — the value the
/// workload's finalize hook returns ([`Scheduler::finalize`](super::Scheduler::finalize)) and the
/// operand [`Workload::deliver`] adopts inside the walk. Value and reach travel as one value derived
/// from one envelope, so the door cannot be handed a coverage belonging to some other terminal, and
/// the transit bundle covering the walk's adopts is derived here rather than beside the call.
pub type DeliveredTerminal<W> = Delivered<<W as Workload>::Value, Carrier<OwnerOf<W>>, OwnerOf<W>>;

/// A **destination operand**: a bare handle on the region a delivery lands in, sealed into the
/// envelope the composition verbs take. The delivery walk builds one per distinct destination and
/// hands it to [`Workload::deliver`].
pub type DeliveryDestination<W> =
    Delivered<RegionHandleFamily<<W as Workload>::Profile>, Carrier<OwnerOf<W>>, OwnerOf<W>>;

/// The Koan-agnostic interface the generic DAG scheduler is parameterized over: the workload types
/// it stores opaquely and never inspects. The Koan instantiation is `machine::execute::KoanWorkload`.
///
/// The one behavioural hook is [`deliver`](Workload::deliver): the delivery walk owns *when* and
/// *where* a terminal lands, the embedder owns *how* it gets there.
pub trait Workload: Sized
where
    <Self::Frame as Anchor>::Owner: RegionOwner<Region = Region<Self::Profile>>,
{
    /// The inter-node value carried along dep edges. A one-lifetime [`Reattachable`] family: the
    /// scheduler stores it in a finalized terminal's `Witnessed<Self::Value, _>` (the value erased,
    /// bundled with the producer frame `Rc`) and re-anchors it to the read borrow through
    /// `Witnessed::read`. `At<'static>: Copy` lets a `&self` read copy the erased carrier out before
    /// re-anchoring it.
    type Value: Reattachable<At<'static>: Copy> + DropFree;
    /// The terminal error type. `Clone` because delivery is per destination: an errored producer's
    /// terminal lands on every live edge waiting on it, one clone apiece, where a value terminal is
    /// adopted once per distinct destination region.
    type Error: Clone;
    /// The storage profile the destination regions are built over — what the walk's per-destination
    /// handle and the [`deliver`](Self::deliver) operand are typed against. Koan's is
    /// `KoanStorageProfile`.
    type Profile: StorageProfile<FrameOwner = OwnerOf<Self>> + 'static;
    /// The per-slot memory anchor the scheduler manages by `Rc` (minted by the workload). The
    /// scheduler stores it, hands it back from [`take_for_run`](super::Scheduler::take_for_run), and
    /// calls only [`Anchor::owner`] — projecting the region owner the delivery envelope and the
    /// destination operands are typed against. The row holds the anchor from alloc until finalize:
    /// delivery moves the terminal into its destinations, so the slot's reclaim drops the anchor
    /// unconditionally and the scheduler keeps no pin of its own
    /// (design/reach.md § Retention model).
    type Frame: Anchor;
    /// The per-node continuation: a one-lifetime [`Reattachable`] family the scheduler rests on the
    /// owned tier (`SealedPinned<Self::Continuation, Rc<Self::Frame>>`), sealed against the node's
    /// anchor at install and handed back once per step for the workload to open and run. Never
    /// inspected. Not `Copy` — a one-shot boxed closure consumed by value — and not `DropFree`
    /// either: the owned tier keeps the value's drop glue, so a continuation owning heap contents
    /// (a boxed closure) rests soundly and drops soundly if its slot is never opened.
    type Continuation: Reattachable;

    /// **Adopt `terminal` at `dest`** — the delivery walk's per-destination relocation, and the one
    /// place the scheduler asks the embedder a question about a value.
    ///
    /// The walk decides *which* destinations a terminal lands in (one adopt per distinct destination
    /// region, fanned out to every edge in that bucket) and holds the source envelope's pins across
    /// the whole walk. What crossing the boundary *costs* is the embedder's: a structural copy, a
    /// pointer-preserving pin, and the retention claim each implies. Koan's impl is `relocate_seam`.
    ///
    /// The product is the terminal as it exists **at the destination** — its residence is `dest`'s
    /// own region, its coverage what the relocation still reaches — so the walk rests it there and
    /// keeps no pin of its own.
    fn deliver(
        terminal: &DeliveredTerminal<Self>,
        dest: DeliveryDestination<Self>,
    ) -> DeliveredTerminal<Self>;

    /// **The edges a retiring slot still owns** — the workload's slot-retirement hook.
    /// [`Scheduler::drain`](super::Scheduler::drain) invokes it exactly once per slot, at the one
    /// point the slot stops being able to release its edges — before the delivery walk on a
    /// terminal, after the forward read, after the splice re-point — and releases every edge it
    /// returns. The workload's impl drains whatever owned-edge record its anchor keeps (and does
    /// its own bookkeeping for those names); a workload whose anchors own no edges takes the
    /// default.
    fn retiring(_anchor: &Self::Frame) -> Vec<EdgeId> {
        Vec::new()
    }
}
