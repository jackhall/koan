use std::rc::Rc;

use super::{DropFree, EdgeId, Reattachable};
use crate::witnessed::{
    Carrier, Delivered, PinsRegion, Region, RegionHandleFamily, RegionOwner, Retained,
    StorageProfile, Witnessed,
};

/// The live (caller-lifetime) form of the inter-node value for a workload `W`, re-anchored from a
/// delivered edge's stored cell at the borrow under which that edge's destination region stays
/// pinned.
pub type Live<'node, W> = <<W as Workload>::Value as Reattachable>::At<'node>;

/// The per-slot memory anchor's contract: the scheduler holds one `Rc<W::Frame>` per slot and never
/// inspects it beyond the owner projection, which retention and delivery need at the true owner
/// type. Koan's anchor is the per-call slot frame; its owner is `FrameStorage`.
pub trait Anchor: 'static {
    /// The delivery-envelope and reach-set member type. [`PinsRegion`] is the reach-set member
    /// contract; the scheduler retains and drops the `Rc` but calls no method on it. Koan's is
    /// `FrameStorage`.
    type Owner: PinsRegion + 'static;
    fn owner(&self) -> &Rc<Self::Owner>;
}

/// The anchor's projected region-owner type — the delivery-envelope and reach-set member type.
pub type OwnerOf<W> = <<W as Workload>::Frame as Anchor>::Owner;

/// A finalized terminal: the erased inter-node value bundled with the reference-only [`Carrier`]
/// naming the regions it reaches (empty for a frameless / run-region value).
pub type Terminal<W> = Witnessed<<W as Workload>::Value, Carrier<OwnerOf<W>>>;

/// A finalized terminal in its dormant [`Retained`] form — what a delivered edge holds, read back
/// under the destination region's own pin.
pub type SealedTerminal<W> = Retained<<W as Workload>::Value, Carrier<OwnerOf<W>>>;

/// A terminal **in transit**: the carrier bundled with the owned coverage pinning every region it
/// reaches, its own residence among them. This is the currency of the terminal door — what a
/// [`StepVerdict::Done`](super::StepVerdict::Done) carries, and the operand [`Workload::deliver`]
/// adopts inside the walk. Value and reach travel as one value derived from one envelope, so the
/// door cannot be handed a coverage belonging to some other terminal.
pub type DeliveredTerminal<W> = Delivered<<W as Workload>::Value, Carrier<OwnerOf<W>>, OwnerOf<W>>;

/// A **destination operand**: a bare handle on the region a delivery lands in, sealed into the
/// envelope the composition verbs take. The delivery walk builds one per distinct destination and
/// hands it to [`Workload::deliver`].
pub type DeliveryDestination<W> =
    Delivered<RegionHandleFamily<<W as Workload>::Profile>, Carrier<OwnerOf<W>>, OwnerOf<W>>;

/// The Koan-agnostic interface the generic DAG scheduler is parameterized over: the workload types
/// it stores opaquely and never inspects. The Koan instantiation is `machine::execute::KoanWorkload`.
///
/// Two behavioural hooks: [`deliver`](Workload::deliver), where the delivery walk owns *when* and
/// *where* a terminal lands and the embedder owns *how* it gets there, and
/// [`retiring`](Workload::retiring), the slot's owned-edge release.
pub trait Workload: Sized
where
    <Self::Frame as Anchor>::Owner: RegionOwner<Region = Region<Self::Profile>>,
{
    /// The inter-node value carried along dep edges. A one-lifetime [`Reattachable`] family, stored
    /// erased and re-anchored to each read borrow. `At<'static>: Copy` lets a `&self` read copy the
    /// erased carrier out before re-anchoring it.
    type Value: Reattachable<At<'static>: Copy> + DropFree;
    /// The terminal error type. `Clone` because delivery is per destination: an errored producer's
    /// terminal lands on every live edge waiting on it, one clone apiece, where a value terminal is
    /// adopted once per distinct destination region.
    type Error: Clone;
    /// The storage profile the destination regions are built over — what the walk's per-destination
    /// handle and the [`deliver`](Self::deliver) operand are typed against. Koan's is
    /// `KoanStorageProfile`.
    type Profile: StorageProfile<FrameOwner = OwnerOf<Self>> + 'static;
    /// The per-slot memory anchor the scheduler manages by `Rc` (minted by the workload), calling
    /// only [`Anchor::owner`]. The row holds the anchor from alloc until finalize: delivery moves
    /// the terminal into its destinations, so the slot's reclaim drops the anchor unconditionally
    /// and the scheduler keeps no pin of its own
    /// ([design/reach.md § Retention model](../../design/reach.md#retention-model)).
    type Frame: Anchor;
    /// The per-node continuation: a one-lifetime [`Reattachable`] family the scheduler rests on the
    /// owned tier (`SealedPinned<Self::Continuation, Rc<Self::Frame>>`), sealed against the node's
    /// anchor at install and handed back once per step for the workload to open and run. Never
    /// inspected. Neither `Copy` — a one-shot boxed closure consumed by value — nor `DropFree`: the
    /// owned tier keeps the value's drop glue, so a continuation owning heap contents rests soundly
    /// and drops soundly if its slot is never opened.
    type Continuation: Reattachable;

    /// **Adopt `terminal` at `dest`** — the delivery walk's per-destination relocation.
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

    /// **The edges a retiring slot still owns.** [`Scheduler::drain`](super::Scheduler::drain)
    /// invokes it exactly once per slot, at the one point the slot stops being able to release its
    /// edges — before the delivery walk on a terminal, after the forward read, after the splice
    /// re-point — and releases every edge it returns. The impl drains whatever owned-edge record
    /// its anchor keeps (and does its own bookkeeping for those names); a workload whose anchors
    /// own no edges takes the default.
    fn retiring(_anchor: &Self::Frame) -> Vec<EdgeId> {
        Vec::new()
    }
}
