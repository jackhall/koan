use std::rc::Rc;

/// A finalized terminal read together with the producer frame `Rc` backing it (`None` for a
/// frameless / run-arena value): the `read_result_with_frame` return shape, aliased so the
/// associated-type projection nest stays out of the method signatures. The error is borrowed —
/// the scheduler hands back a reference into the slot, never an owned error.
pub(crate) type FramedRead<'a, W> = Result<
    (<W as Workload>::Value, Option<Rc<<W as Workload>::Frame>>),
    &'a <W as Workload>::Error,
>;

/// The Koan-agnostic interface the generic DAG scheduler is parameterized over: the workload types
/// it stores opaquely and never inspects. The Koan instantiation is `machine::execute::KoanWorkload`.
pub(crate) trait Workload {
    /// The per-node name-resolution payload the scheduler stores, installs ambient, and hands back.
    type Payload: Clone;
    /// The inter-node value carried along dep edges (stored in a finalized terminal).
    type Value: Copy;
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
