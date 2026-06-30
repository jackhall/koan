use super::Reattachable;

/// The live (caller-lifetime) form of the inter-node value for a workload `W`, re-anchored from the
/// scheduler's `Witnessed<W::Value, _>` slot at the borrow under which the producer frame stays
/// pinned. `Live<'node, W>` is what a slot read hands back and what `finalize` is given.
pub(crate) type Live<'node, W> = <<W as Workload>::Value as Reattachable>::At<'node>;

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
    /// The finalized-value witness: the set of region owners pinning a stored terminal's backing
    /// (empty for a frameless / run-region value, which is already in a surviving region). The result
    /// slot stores `Sealed<Self::Value, Self::Witness>`; a `Default` empty value re-homes a drained
    /// root that needs no pin, and `Clone` hands the set out to the consumer-pull lift. The Koan
    /// instantiation is `FrameSet`.
    type Witness: crate::witnessed::Witness + Clone + Default;
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
