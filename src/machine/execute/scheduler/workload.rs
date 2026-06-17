use std::rc::Rc;

/// A finalized terminal read together with the producer frame `Rc` backing it (`None` for a
/// frameless / run-arena value): the `read_result_with_frame` return shape, aliased so the
/// associated-type projection nest stays out of the method signatures. The error is borrowed —
/// the scheduler hands back a reference into the slot, never an owned error.
pub(in crate::machine::execute) type FramedRead<'a, W> = Result<
    (<W as Workload>::Value, Option<Rc<<W as Workload>::Frame>>),
    &'a <W as Workload>::Error,
>;

/// The Koan-agnostic interface the generic DAG scheduler is parameterized over: the four workload
/// types it stores opaquely and never inspects. The Koan instantiation is `KoanWorkload`.
pub(in crate::machine::execute) trait Workload {
    /// The per-node name-resolution payload the scheduler stores, installs ambient, and hands back.
    type Payload: Clone;
    /// The inter-node value carried along dep edges (stored in a finalized terminal).
    type Value: Copy;
    /// The terminal error type (stored in a finalized terminal; the scheduler only stores/borrows it).
    type Error;
    /// The per-node memory frame the scheduler manages by `Rc` (mints via the workload; never calls a method on it).
    type Frame;
}
