use std::rc::Rc;

use super::{DropFree, Reattachable};
use crate::witnessed::{Carrier, Delivered, PinsRegion, Sealed, Witnessed};

/// The live (caller-lifetime) form of the inter-node value for a workload `W`, re-anchored from the
/// scheduler's `Witnessed<W::Value, _>` slot at the borrow under which the producer frame stays
/// pinned. `Live<'node, W>` is what a slot read hands back and what `finalize` is given.
pub type Live<'node, W> = <<W as Workload>::Value as Reattachable>::At<'node>;

/// The per-slot memory anchor's contract: the scheduler holds one `Rc<W::Frame>` per slot, never
/// inspects it beyond this projection, and projects its region owner where retention and delivery
/// need the true owner type. Koan's anchor is the per-call slot frame; its owner is `FrameStorage`.
pub trait Anchor: 'static {
    /// The projected region-owner type — the retention-hold, delivery-envelope, and reach-set
    /// member type. Koan's is `FrameStorage`. [`PinsRegion`] is the reach-set member contract; the
    /// scheduler retains and drops the `Rc` but calls no method on it.
    type Owner: PinsRegion + 'static;
    /// The anchor's region owner, projected for retention and delivery.
    fn owner(&self) -> &Rc<Self::Owner>;
}

/// The anchor's projected region-owner type — the retention-hold and delivery-envelope member type.
pub type OwnerOf<W> = <<W as Workload>::Frame as Anchor>::Owner;

/// A finalized terminal as the workload's finalize hook delivers it: the erased inter-node value
/// bundled with the reference-only [`Carrier`] naming the regions it reaches (empty for a
/// frameless / run-region value). The store seals it for dormant storage between steps.
pub type Terminal<W> = Witnessed<<W as Workload>::Value, Carrier<OwnerOf<W>>>;

/// A finalized terminal in its dormant [`Sealed`] form — what a result slot stores and a
/// consumer pull duplicates, read back under the retention hold ([`Sealed::open_with`]).
pub type SealedTerminal<W> = Sealed<<W as Workload>::Value, Carrier<OwnerOf<W>>>;

/// A terminal **in transit**: the carrier bundled with the owned coverage pinning every region it
/// reaches, its own residence among them. This is the currency of both terminal doors — the value
/// the workload's finalize hook returns ([`Scheduler::finalize`](super::Scheduler::finalize),
/// [`Scheduler::rehome_terminal`](super::Scheduler::rehome_terminal)) and the one a consumer pulls
/// ([`Scheduler::dep_delivered`](super::Scheduler::dep_delivered)). Value and reach travel as one
/// value derived from one envelope, so neither door can be handed a coverage belonging to some
/// other terminal, and the retention hold's foreign bundle is derived here rather than beside the
/// call.
pub type DeliveredTerminal<W> = Delivered<<W as Workload>::Value, Carrier<OwnerOf<W>>, OwnerOf<W>>;

/// The Koan-agnostic interface the generic DAG scheduler is parameterized over: the workload types
/// it stores opaquely and never inspects. The Koan instantiation is `machine::execute::KoanWorkload`.
pub trait Workload {
    /// The inter-node value carried along dep edges. A one-lifetime [`Reattachable`] family: the
    /// scheduler stores it in a finalized terminal's `Witnessed<Self::Value, _>` (the value erased,
    /// bundled with the producer frame `Rc`) and re-anchors it to the read borrow through
    /// `Witnessed::read`. `At<'static>: Copy` lets a `&self` read copy the erased carrier out before
    /// re-anchoring it.
    type Value: Reattachable<At<'static>: Copy> + DropFree;
    /// The terminal error type (stored in a finalized terminal; the scheduler only stores/borrows it).
    type Error;
    /// The per-slot memory anchor the scheduler manages by `Rc` (minted by the workload). The
    /// scheduler stores it, hands it back from [`take_for_run`](super::Scheduler::take_for_run), and
    /// calls only [`Anchor::owner`] — projecting the region owner it retains for delivery-driven
    /// frame retention. It holds an `Rc<Self::Frame>` for a finalized producer until every
    /// destination has pulled the terminal, releasing at pull-count zero
    /// (design/reach.md § Retention model).
    type Frame: Anchor;
    /// The per-node continuation: a one-lifetime [`Reattachable`] family the scheduler rests on the
    /// owned tier (`SealedPinned<Self::Continuation, Rc<Self::Frame>>`), sealed against the node's
    /// anchor at install and handed back once per step for the workload to open and run. Never
    /// inspected. Not `Copy` — a one-shot boxed closure consumed by value — and not `DropFree`
    /// either: the owned tier keeps the value's drop glue, so a continuation owning heap contents
    /// (a boxed closure) rests soundly and drops soundly if its slot is never opened.
    type Continuation: Reattachable;
}
