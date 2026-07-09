//! Slot-table state. A single `slots` vector of [`SlotState`] enums encodes the per-slot lifecycle:
//! every slot moves through `alloc_slot -> take_for_run -> reinstall -> finalize -> free_one`.
//!
//! ## Invariants
//!
//! - `alloc_slot` is the only path that picks an index (recycle from
//!   `free_list` or extend `slots`).
//! - `slots` is wrapped in [`SlotVec<T>`], which only impls `Index<NodeId>` /
//!   `IndexMut<NodeId>`, so a `NodeId` always names a live slot.
//! - `free_one` is the sole pusher onto `free_list`. Outer `Scheduler`
//!   orchestrates the notify-walk and cascade-free across this store and
//!   `DepGraph`.

use std::ops::{Index, IndexMut};
use std::rc::Rc;

use super::nodes::{Node, NodeWork};
use super::{Live, NodeId, SealedTerminal, Terminal, Workload};
use crate::witnessed::{Carrier, Sealed, Witnessed};
// `Erased` re-anchors a test-only result through `set_result`; the production store path takes a
// pre-built `Witnessed`, so the import is test-scoped.
#[cfg(any(test, feature = "test-hooks"))]
use super::Erased;

/// `Vec`-backed slot store keyed by [`NodeId`]. `NodeId`s are minted only
/// by [`NodeStore::alloc_slot`].
struct SlotVec<T>(Vec<T>);

impl<T> SlotVec<T> {
    fn new() -> Self {
        Self(Vec::new())
    }
    fn push(&mut self, v: T) {
        self.0.push(v);
    }
    fn len(&self) -> usize {
        self.0.len()
    }
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    fn get(&self, id: NodeId) -> Option<&T> {
        self.0.get(id.index())
    }
    fn iter(&self) -> impl Iterator<Item = &T> {
        self.0.iter()
    }
}

impl<T> Index<NodeId> for SlotVec<T> {
    type Output = T;
    fn index(&self, id: NodeId) -> &T {
        &self.0[id.index()]
    }
}

impl<T> IndexMut<NodeId> for SlotVec<T> {
    fn index_mut(&mut self, id: NodeId) -> &mut T {
        &mut self.0[id.index()]
    }
}

enum SlotState<W: Workload> {
    PreRun(Node<W>),
    /// Node payload has been moved out by `take_for_run`. A matching
    /// `reinstall` / `finalize` / `free_one` exits this state.
    Running,
    /// A finalized terminal: a [`Sealed`] carrier bundling the value (erased to `'static`) with its
    /// reference-only reach witness, sealed as-is — the carrier pins nothing. What keeps the value's
    /// backing alive is the scheduler's retention hold on the producer frame (seeded at finalize,
    /// released at pull-count zero), so every read re-anchors under that retained owner
    /// (`Sealed::open_with`) and a drained root re-homed into the run region reads under the empty
    /// pin. The error carries no frame (it owns its data).
    Done(Result<SealedTerminal<W>, W::Error>),
    /// A bare-name forward spliced out: this slot's result *is* `producer`'s. `read_result` /
    /// `is_result_ready` follow the alias through to `producer` (which holds the sole copy). The
    /// slot's consumers were moved onto `producer`'s notify list at splice time, so `producer`'s
    /// fire wakes them directly.
    Aliased(NodeId),
    /// Distinct from `Running` so the cascade-free walk's idempotency
    /// guard can be precise about "already freed".
    Free,
}

/// The drain-end deadlock-sample contribution of one parked/pending slot's work.
/// `unresolved` shows the first `Preferred` (a workload-supplied expression) across all stuck slots,
/// falling back to the first `Fallback` (a generic work-shape tag) only when no slot carries an
/// expression — so a stuck named work always out-renders a bare `<wait>`.
enum DeadlockSample {
    Preferred(String),
    Fallback(&'static str),
}

/// Map a stuck slot's `work` to its deadlock-sample contribution. A `Some`-carrier wait carries a
/// renderable expression summary (`Preferred`); a carrier-less wait carries only a generic tag
/// (`Fallback`).
fn work_deadlock_sample<W: Workload>(work: &NodeWork<W>) -> DeadlockSample {
    let NodeWork { carrier, .. } = work;
    match carrier {
        Some(carrier) => DeadlockSample::Preferred(carrier.clone()),
        None => DeadlockSample::Fallback("<wait>"),
    }
}

pub(in crate::scheduler) struct NodeStore<W: Workload> {
    slots: SlotVec<SlotState<W>>,
    /// Reclaimed slot indices. `alloc_slot` pulls from here before
    /// extending `slots`, giving constant scheduler memory across
    /// tail-recursive bodies.
    free_list: Vec<NodeId>,
}

impl<W: Workload> NodeStore<W> {
    pub(super) fn new() -> Self {
        Self {
            slots: SlotVec::new(),
            free_list: Vec::new(),
        }
    }

    /// The only path that picks an index. `DepGraph::install_for_slot`
    /// mirrors the recycle-vs.-extend choice via
    /// `consumer.index() < notify_list.len()`.
    pub(super) fn alloc_slot(&mut self, node: Node<W>) -> NodeId {
        match self.free_list.pop() {
            Some(id) => {
                self.slots[id] = SlotState::PreRun(node);
                id
            }
            None => {
                let id = NodeId(self.slots.len());
                self.slots.push(SlotState::PreRun(node));
                id
            }
        }
    }

    /// Panics if the slot wasn't `PreRun`.
    pub(super) fn take_for_run(&mut self, id: NodeId) -> Node<W> {
        match std::mem::replace(&mut self.slots[id], SlotState::Running) {
            SlotState::PreRun(node) => node,
            _ => panic!("scheduler must not revisit a completed node"),
        }
    }

    /// Tail-call path: reuse the slot index for a new node. The workload built the slot's `payload`.
    pub(super) fn reinstall(&mut self, id: NodeId, node: Node<W>) {
        self.slots[id] = SlotState::PreRun(node);
    }

    /// Replace a finalized terminal in place, dropping any pinned producer frame. The drain
    /// boundary uses this to re-home a consumer-less root into a surviving region (`output` already
    /// lifted there), releasing the per-call frame the producer kept it in.
    pub(super) fn rehome_terminal(&mut self, id: NodeId, output: Result<Terminal<W>, W::Error>) {
        debug_assert!(
            matches!(self.slots[id], SlotState::Done(..)),
            "rehome_terminal expects a finalized slot",
        );
        // The terminal arrives already relocated and re-sealed under its surviving-source witness set
        // (the run region drops out of the union), so just store the seal.
        self.slots[id] = SlotState::Done(output.map(Sealed::seal));
    }

    /// Callers must pair this with the dep-graph notify-walk so consumers wake atomically with the
    /// write. The terminal arrives already bundled with its witness set (built by the workload's
    /// finalize hook: the producer frame ∪ every region the value reaches; the empty set for a
    /// frameless / run-region terminal), so this just seals it for dormant storage. On `Err` the
    /// erased error owns its data and carries no witness.
    pub(super) fn finalize(&mut self, id: NodeId, output: Result<Terminal<W>, W::Error>) {
        self.slots[id] = SlotState::Done(output.map(Sealed::seal));
    }

    /// Duplicate the finalized terminal's sealed carrier — value + witness set — leaving the slot's
    /// own seal intact for other consumers. The consumer-pull lift hands this to a construction finish
    /// so the dep arrives **witnessed** (its reach named on the carrier), ready to fold via
    /// [`Delivered::transfer_into`](crate::witnessed::Delivered::transfer_into) — rather than the
    /// value read out bare and re-paired with a separately-read witness in an asserted co-location
    /// bundle.
    pub(super) fn dep_carrier(&self, id: NodeId) -> Result<SealedTerminal<W>, &W::Error> {
        match &self.slots[id] {
            SlotState::Done(Ok(sealed), ..) => Ok(sealed.duplicate()),
            SlotState::Done(Err(e), ..) => Err(e),
            _ => panic!("result must be ready by the time its carrier is taken"),
        }
    }

    /// Idempotent on already-`Free` slots when paired with the cascade-free
    /// walk's `is_reclaimed` guard. Pairs with the `notify_list[id]` /
    /// `dep_edges[id]` free-time clears in `DepGraph`.
    pub(super) fn free_one(&mut self, id: NodeId) {
        self.slots[id] = SlotState::Free;
        self.free_list.push(id);
    }

    /// The alias target of a spliced-out bare-name forward, or `None`. The single follow step the
    /// `Scheduler`-level [`resolve_alias`](super::Scheduler::resolve_alias) walks; resolution lives
    /// there (with `DepGraph`), not in the store.
    pub(super) fn alias_target(&self, id: NodeId) -> Option<NodeId> {
        match self.slots.get(id) {
            Some(SlotState::Aliased(to)) => Some(*to),
            _ => None,
        }
    }

    /// Raw readiness — callers pass an already alias-resolved id.
    pub(super) fn is_result_ready(&self, id: NodeId) -> bool {
        matches!(self.slots.get(id), Some(SlotState::Done(..)))
    }

    /// Read a finalized terminal at a **rank-2** brand: the value is opened through [`Sealed::open`]
    /// and handed to `f` as `Result<Live<'b>, &W::Error>`, so the re-anchored carrier nests inside
    /// the access rather than escaping up-stack. The destination-verb form of the value read — the
    /// consumer copies out what it needs (a scalar, a cloned error) from inside the closure. Callers
    /// pass an already alias-resolved id; the slot must be finalized.
    pub(super) fn read_result_with<R>(
        &self,
        id: NodeId,
        pin: Option<&Rc<W::Frame>>,
        f: impl for<'b> FnOnce(Live<'b, W>) -> R,
    ) -> Result<R, &W::Error> {
        match &self.slots[id] {
            // Re-anchor under the retained frame owner (`open_with`) — the carrier's own witness is
            // reference-only and pins nothing. A slot with no retained owner (a drained root re-homed
            // into the run region) is externally pinned, so the read opens under the empty pin.
            SlotState::Done(Ok(w), ..) => Ok(match pin {
                Some(p) => w.open_with(p, f),
                None => w.open_with(&crate::witnessed::RegionSet::<W::Frame>::empty(), f),
            }),
            SlotState::Done(Err(e), ..) => Err(e),
            _ => panic!("result must be ready by the time it's read"),
        }
    }

    /// The terminal's error, or `Ok(())` for a value terminal — the borrow-free probe the many
    /// consumers that only branch on success/failure use, reading no value (no `open`). Callers pass
    /// an already alias-resolved id; the slot must be finalized.
    pub(super) fn result_error(&self, id: NodeId) -> Result<(), &W::Error> {
        match &self.slots[id] {
            SlotState::Done(Ok(_), ..) => Ok(()),
            SlotState::Done(Err(e), ..) => Err(e),
            _ => panic!("result must be ready by the time it's read"),
        }
    }

    /// Scan for slots still parked (`PreRun`) after the work queues drained — each
    /// is a node waiting on a dependency that can no longer fire (a dependency
    /// cycle). Returns `(count, sample)` where `sample` summarizes the first such
    /// node, or `None` when every slot is terminal (`Done`) or reclaimed (`Free`).
    pub(super) fn unresolved(&self) -> Option<(usize, String)> {
        let mut count = 0usize;
        let mut expr_sample: Option<String> = None;
        let mut fallback_sample: Option<String> = None;
        for slot in self.slots.iter() {
            if let SlotState::PreRun(node) = slot {
                count += 1;
                match work_deadlock_sample(&node.work) {
                    DeadlockSample::Preferred(s) if expr_sample.is_none() => expr_sample = Some(s),
                    DeadlockSample::Fallback(s) if fallback_sample.is_none() => {
                        fallback_sample = Some(s.to_string());
                    }
                    _ => {}
                }
            }
        }
        if count == 0 {
            return None;
        }
        Some((count, expr_sample.or(fallback_sample).unwrap_or_default()))
    }

    pub(super) fn len(&self) -> usize {
        self.slots.len()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub(super) fn is_live(&self, id: NodeId) -> bool {
        matches!(self.slots[id], SlotState::PreRun(_))
    }

    /// Returns true for any non-`Done` state so the cascade-free walk does
    /// not double-push onto `free_list`. Assumes `is_live` has already
    /// excluded `PreRun` upstream.
    pub(super) fn is_reclaimed(&self, id: NodeId) -> bool {
        !matches!(self.slots[id], SlotState::Done(..))
    }

    /// Splice a bare-name forward out: the running slot becomes an alias of `producer` (a
    /// downstream real producer). `read_result` / `is_result_ready` follow the alias; the slot's
    /// consumers were already moved onto `producer`'s notify list, so this just records the redirect.
    pub(super) fn alias(&mut self, id: NodeId, producer: NodeId) {
        self.slots[id] = SlotState::Aliased(producer);
    }

    // --- Test-only helpers for synthetic-state setup. ---

    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) fn clear_node(&mut self, id: NodeId) {
        self.slots[id] = SlotState::Running;
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) fn set_result(&mut self, id: NodeId, output: Result<Live<'_, W>, W::Error>) {
        self.slots[id] = SlotState::Done(
            output
                .map(Erased::erase)
                .map(|e| Sealed::seal(Witnessed::from_erased(e, Carrier::default()))),
        );
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) fn result_is_some(&self, id: NodeId) -> bool {
        matches!(self.slots[id], SlotState::Done(..))
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) fn result_is_none(&self, id: NodeId) -> bool {
        !matches!(self.slots[id], SlotState::Done(..))
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) fn free_list_snapshot(&self) -> Vec<NodeId> {
        self.free_list.clone()
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) fn free_list_len(&self) -> usize {
        self.free_list.len()
    }

    /// The live slot's opaque payload, or `None` once it has terminalized. The workload extracts
    /// the field it wants (e.g. the lexical chain). Test-only.
    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) fn payload_of(&self, id: NodeId) -> Option<&W::Payload> {
        match self.slots.get(id) {
            Some(SlotState::PreRun(node)) => Some(&node.payload),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::witnessed::reattachable;

    /// A lifetime-free `Reattachable` family for the trivial test value.
    struct U32Value;
    /// A lifetime-free `Reattachable` family standing in for the contract / continuation carriers.
    struct UnitCarrier;
    // Both are lifetime-free, so `At<'r>` is the same type for every `'r`; the shared `reattachable!`
    // macro discharges the obligation.
    reattachable! {
        U32Value => u32,
        UnitCarrier => (),
    }

    /// A minimal workload for the white-box store tests: every associated type is trivial, so the
    /// generic store can be exercised without naming any Koan type.
    struct TestWorkload;
    impl Workload for TestWorkload {
        type Payload = ();
        type Value = U32Value;
        type Error = ();
        type Cart = ();
        // A trivial `PinsRegion` frame owner — the retention hold's `Rc<Frame>` type, which these
        // white-box tests never construct.
        type Frame = crate::witnessed::doctest_fixture::Cart;
        type Contract = UnitCarrier;
        type Continuation = UnitCarrier;
    }

    fn sample_wait(carrier: Option<String>) -> NodeWork<TestWorkload> {
        NodeWork::new(super::super::ResolvedDeps::new(), (), carrier)
    }

    #[test]
    fn some_carrier_wait_prefers_the_carrier() {
        let work = sample_wait(Some("PARKED-EXPR".to_string()));
        assert!(
            matches!(work_deadlock_sample(&work), DeadlockSample::Preferred(s) if s.contains("PARKED")),
            "a Some-carrier wait must surface its carrier",
        );
    }

    #[test]
    fn carrier_less_wait_falls_back_to_a_tag() {
        let work = sample_wait(None);
        assert!(
            matches!(
                work_deadlock_sample(&work),
                DeadlockSample::Fallback("<wait>")
            ),
            "a carrier-less wait must surface a generic tag, not an empty sample",
        );
    }
}
