use super::Reattachable;
use crate::witnessed::{Carrier, PinsRegion, Sealed, Witnessed};

/// The live (caller-lifetime) form of the inter-node value for a workload `W`, re-anchored from the
/// scheduler's `Witnessed<W::Value, _>` slot at the borrow under which the producer frame stays
/// pinned. `Live<'node, W>` is what a slot read hands back and what `finalize` is given.
pub type Live<'node, W> = <<W as Workload>::Value as Reattachable>::At<'node>;

/// A finalized terminal as the workload's finalize hook delivers it: the erased inter-node value
/// bundled with the reference-only [`Carrier`] naming the regions it reaches (empty for a
/// frameless / run-region value). The store seals it for dormant storage between steps.
pub type Terminal<W> = Witnessed<<W as Workload>::Value, Carrier<<W as Workload>::Frame>>;

/// A finalized terminal in its dormant [`Sealed`] form — what a result slot stores and a
/// consumer pull duplicates, read back under the retention hold ([`Sealed::open_with`]).
pub type SealedTerminal<W> = Sealed<<W as Workload>::Value, Carrier<<W as Workload>::Frame>>;

/// The Koan-agnostic interface the generic DAG scheduler is parameterized over: the workload types
/// it stores opaquely and never inspects. The Koan instantiation is `machine::execute::KoanWorkload`.
pub trait Workload {
    /// The per-node name-resolution payload the scheduler stores, installs ambient, and hands back.
    type Payload;
    /// The inter-node value carried along dep edges. A one-lifetime [`Reattachable`] family: the
    /// scheduler stores it in a finalized terminal's `Witnessed<Self::Value, _>` (the value erased,
    /// bundled with the producer frame `Rc`) and re-anchors it to the read borrow through
    /// `Witnessed::read`. `At<'static>: Copy` lets a `&self` read copy the erased carrier out before
    /// re-anchoring it.
    type Value: Reattachable<At<'static>: Copy>;
    /// The terminal error type (stored in a finalized terminal; the scheduler only stores/borrows it).
    type Error;
    /// The per-node memory frame the scheduler manages by `Rc` (minted by the workload; never calls a method on it).
    type Cart;
    /// The producer-frame owner the scheduler retains for delivery-driven frame retention: it holds
    /// an `Rc<Self::Frame>` for a finalized producer until every destination has pulled the terminal,
    /// releasing at pull-count zero (design/witness-hosting.md § Retention model). [`PinsRegion`] is
    /// the reach-set member contract; the scheduler retains and drops the `Rc` but calls no method on
    /// it. The Koan instantiation is `FrameStorage`.
    type Frame: PinsRegion + 'static;
    /// The per-node return contract: a one-lifetime [`Reattachable`] family the scheduler stores
    /// erased (`Erased<Self::Contract>`) on a slot's frame and hands back at the Done boundary; the
    /// workload re-anchors it, witnessed by the frame `Rc`. Never inspected. `At<'static>: Copy` lets
    /// a tail chain keep-first the erased contract by copy.
    type Contract: Reattachable<At<'static>: Copy>;
    /// The per-node continuation: a one-lifetime [`Reattachable`] family the scheduler stores erased
    /// (`Erased<Self::Continuation>`) on the node and hands back once per step; the workload
    /// re-anchors it, witnessed by the node's cart `Rc`, then runs it once. Never inspected. Not
    /// `Copy` — a one-shot boxed closure consumed by value.
    type Continuation: Reattachable;
}
