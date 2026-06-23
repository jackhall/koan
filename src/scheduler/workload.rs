use std::rc::Rc;

use super::Reattachable;

/// The live (caller-lifetime) form of the inter-node value for a workload `W`, re-anchored from the
/// scheduler's `Witnessed<W::Value, _>` slot at the borrow under which the producer frame stays
/// pinned. `Live<'node, W>` is what a slot read hands back and what `finalize` is given.
pub(crate) type Live<'node, W> = <<W as Workload>::Value as Reattachable>::At<'node>;

/// A finalized terminal read together with the producer frame `Rc` backing it (`None` for a
/// frameless / run-region value): the `read_result_with_frame` return shape, aliased so the
/// associated-type projection nest stays out of the method signatures. The value is re-anchored to
/// the `'node` read borrow; the error is borrowed — the scheduler hands back a reference into the
/// slot, never an owned error.
pub(crate) type FramedRead<'node, W> =
    Result<(Live<'node, W>, Option<Rc<<W as Workload>::Cart>>), &'node <W as Workload>::Error>;

/// The Koan-agnostic interface the generic DAG scheduler is parameterized over: the workload types
/// it stores opaquely and never inspects. The Koan instantiation is `machine::execute::KoanWorkload`.
pub(crate) trait Workload {
    /// The per-node name-resolution payload the scheduler stores, installs ambient, and hands back.
    type Payload: Clone;
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
    /// The per-node return contract: a one-lifetime [`Reattachable`] family the scheduler stores
    /// erased (`Erased<Self::Contract>`) on a slot's frame and re-anchors at the Done boundary via
    /// [`vend_carrier`](super::vend_carrier), witnessed by the frame `Rc`. Never inspected.
    /// `At<'static>: Copy` lets a tail chain keep-first the erased contract by copy.
    type Contract: Reattachable<At<'static>: Copy>;
    /// The per-node continuation: a one-lifetime [`Reattachable`] family the scheduler stores erased
    /// (`Erased<Self::Continuation>`) on the node and re-anchors once at step entry via
    /// [`vend_carrier`](super::vend_carrier), witnessed by the node's cart `Rc`, then hands back to
    /// run once. Never inspected. Not `Copy` — a one-shot boxed closure consumed by value.
    type Continuation: Reattachable;
}
