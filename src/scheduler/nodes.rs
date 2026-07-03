//! The generic per-node state the scheduler stores: a node's [`work`](self::NodeWork) (its deps and
//! the one-shot continuation that runs over them), its opaque workload [`payload`](self::Node), and
//! its [`frame`](self::NodeFrame) (the per-call memory cart it runs against). All three are
//! parametric over the [`Workload`](super::Workload) — the scheduler stores and hands them back but
//! inspects no field beyond the dep wiring.

use std::rc::Rc;

use super::{Erased, Reattachable, ResolvedDeps, Workload};

/// What a scheduler node will run: wait on `deps`, then run `cont` over their resolved terminals.
/// `deps` is a [`ResolvedDeps`] — a `[park_producers..., owned_subs...]` layout the scheduler owns
/// (assembled only through the [`Deps`](super::Deps) builder): parks install `Notify` edges (kept
/// alive), owned deps install `Owned` (cascade-freed at success). `carrier` is the deadlock-report
/// sample (a workload-supplied expression summary, else `None`). The continuation is stored opaquely
/// (`W::Continuation`) and handed back to run once; the node itself never branches and names no
/// workload type.
pub(crate) struct NodeWork<W: Workload> {
    pub(crate) deps: ResolvedDeps,
    /// The slot's continuation, stored erased to `'static` (`Erased<W::Continuation>`) so the node it
    /// sits on pins no borrow. Handed back to run once; never inspected — the workload re-anchors it
    /// once per step via the consuming externally-witnessed
    /// [`SealedExtern::open`](crate::witnessed::SealedExtern::open), against the held cart `Rc`.
    pub(crate) continuation: Erased<W::Continuation>,
    pub(crate) carrier: Option<String>,
}

impl<W: Workload> NodeWork<W> {
    /// Build node work from a **live** continuation, erasing it to `'static` for storage here — the
    /// scheduler owns the erase (peer of `finalize`'s value erase). The continuation is handed back
    /// re-anchored to run once; the scheduler never inspects it.
    pub(crate) fn new(
        deps: ResolvedDeps,
        continuation: <W::Continuation as Reattachable>::At<'_>,
        carrier: Option<String>,
    ) -> Self {
        NodeWork {
            deps,
            continuation: Erased::erase(continuation),
            carrier,
        }
    }

    /// Decompose a popped node's work by value for the run loop: the resolved dep list (read in
    /// delivery order), the erased continuation, and the deadlock-summary carrier.
    pub(crate) fn into_run_parts(self) -> (ResolvedDeps, Erased<W::Continuation>, Option<String>) {
        (self.deps, self.continuation, self.carrier)
    }
}

/// A node's per-call frame state: the execution cart, its ping-pong reserve, and the opaque return
/// contract. Lifetime-free — the cart `Rc` pins everything its members point at, and the contract is
/// stored opaquely as `W::Contract` (the workload re-anchors it at the Done boundary witnessed by
/// `cart`). Every node owns a `NodeFrame`: the cart is the per-node memory the slot's step runs
/// against. `reserve` and `contract` are sparse.
pub(crate) struct NodeFrame<W: Workload> {
    /// The cart this slot's step runs against. The workload mints it and the `Rc` pins it for the
    /// step; the scheduler stores and hands it back but calls no method on it.
    pub(crate) cart: Rc<W::Cart>,
    /// Per-slot reserve cart for the ping-pong rotation that lets a stateful resume reuse a frame
    /// across iterations.
    pub(crate) reserve: Option<Rc<W::Cart>>,
    /// Return contract enforced at the Done boundary, stored erased to `'static`
    /// (`Erased<W::Contract>`). The workload re-anchors it against `cart` at the step brand — opened
    /// alongside the continuation by the consuming externally-witnessed
    /// [`SealedExtern::open`](crate::witnessed::SealedExtern::open). `None` for slots with no
    /// declared-return obligation.
    pub(crate) contract: Option<Erased<W::Contract>>,
}

pub(crate) struct Node<W: Workload> {
    pub(crate) work: NodeWork<W>,
    /// The slot's opaque workload payload, stored and handed back but never inspected by the
    /// scheduler.
    pub(crate) payload: W::Payload,
    /// The slot's per-call frame state (cart + reserve + opaque contract) — never absent, see
    /// [`NodeFrame`].
    pub(crate) frame: NodeFrame<W>,
}
