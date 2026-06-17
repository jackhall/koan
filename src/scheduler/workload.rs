use std::rc::Rc;

use super::Reattachable;

/// The live (caller-lifetime) form of the inter-node value for a workload `W`, re-anchored from the
/// scheduler's `Erased<W::Value>` store at the borrow under which the producer frame stays pinned.
/// `Live<'node, W>` is what a slot read hands back and what `finalize` is given.
pub(crate) type Live<'node, W> = <<W as Workload>::Value as Reattachable>::At<'node>;

/// A finalized terminal read together with the producer frame `Rc` backing it (`None` for a
/// frameless / run-arena value): the `read_result_with_frame` return shape, aliased so the
/// associated-type projection nest stays out of the method signatures. The value is re-anchored to
/// the `'node` read borrow; the error is borrowed — the scheduler hands back a reference into the
/// slot, never an owned error.
pub(crate) type FramedRead<'node, W> = Result<
    (Live<'node, W>, Option<Rc<<W as Workload>::Frame>>),
    &'node <W as Workload>::Error,
>;

/// The Koan-agnostic interface the generic DAG scheduler is parameterized over: the workload types
/// it stores opaquely and never inspects. The Koan instantiation is `machine::execute::KoanWorkload`.
pub(crate) trait Workload {
    /// The per-node name-resolution payload the scheduler stores, installs ambient, and hands back.
    type Payload: Clone;
    /// The inter-node value carried along dep edges. A one-lifetime [`Reattachable`] family: the
    /// scheduler stores it erased (`Erased<Self::Value>`) in a finalized terminal and re-anchors it
    /// to the read borrow, witnessed by the slot's co-stored frame `Rc`. `At<'static>: Copy` lets a
    /// `&self` read copy the erased carrier out before re-anchoring it.
    type Value: Reattachable<At<'static>: Copy>;
    /// The terminal error type (stored in a finalized terminal; the scheduler only stores/borrows it).
    type Error;
    /// The per-node memory frame the scheduler manages by `Rc` (minted by the workload; never calls a method on it).
    type Frame;
    /// The per-node return contract the scheduler stores on a slot's frame and hands back at the
    /// Done boundary, never inspecting it (the workload re-anchors it).
    type Contract: Copy;
    /// The per-node continuation the scheduler stores and hands back to run once, never inspecting
    /// it (the workload re-anchors + invokes it). Not `Clone` — a one-shot boxed closure.
    type Continuation;
}
