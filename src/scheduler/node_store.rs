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

use super::nodes::{Node, NodeWork};
use super::{Erased, FramedRead, Live, NodeId, Workload};
use crate::witnessed::{Sealed, Witnessed};

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

/// A finalized value terminal: the erased inter-node value bundled with the [`Workload::Witness`] set
/// pinning its backing (empty for a frameless / run-region value), held in its dormant [`Sealed`] form
/// between steps and read back through [`Sealed::read`].
type FinalizedValue<W> = Sealed<<W as Workload>::Value, <W as Workload>::Witness>;

enum SlotState<W: Workload> {
    PreRun(Node<W>),
    /// Node payload has been moved out by `take_for_run`. A matching
    /// `reinstall` / `finalize` / `free_one` exits this state.
    Running,
    /// A finalized terminal: a [`Sealed`] carrier bundling the value (erased to `'static`) with the
    /// producer frame `Rc` that backs it (`None` for a frameless / run-region value). A read
    /// re-anchors the value to the read borrow through `Sealed::read`, witnessed by the bundled
    /// frame. Holding the frame `Rc` inside the carrier pins the producer's per-call memory until the
    /// slot is freed, so frame death moves from Done to free and a read's re-anchored lifetime cannot
    /// outlive the backing region. The pin is read by the consumer-pull lift, which copies the
    /// terminal out of this frame into the consumer's. The error carries no frame (it owns its data).
    Done(Result<FinalizedValue<W>, W::Error>),
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
    pub(super) fn rehome_terminal(&mut self, id: NodeId, output: Result<Live<'_, W>, W::Error>) {
        debug_assert!(
            matches!(self.slots[id], SlotState::Done(..)),
            "rehome_terminal expects a finalized slot",
        );
        self.slots[id] = SlotState::Done(
            output
                .map(Erased::erase)
                .map(|e| Sealed::seal(Witnessed::from_erased(e, W::Witness::default()))),
        );
    }

    /// Callers must pair this with the dep-graph notify-walk so consumers
    /// wake atomically with the write. `witness` is the producer frame's witness set, pinned in the
    /// slot until it is freed (the empty set for a frameless / run-region terminal).
    pub(super) fn finalize(
        &mut self,
        id: NodeId,
        output: Result<Live<'_, W>, W::Error>,
        witness: W::Witness,
    ) {
        // Bundle the live terminal with its producer-frame witness into a `Witnessed`, then seal it
        // for dormant storage: the value is erased to `'static` and the co-stored witness set pins its
        // backing region until the slot frees, so a later `read` / `open` can re-anchor it soundly.
        // On `Err` the witness is dropped now — the error owns its data and needs no pin.
        self.slots[id] = SlotState::Done(
            output
                .map(Erased::erase)
                .map(|e| Sealed::seal(Witnessed::from_erased(e, witness))),
        );
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

    /// Only safe on IDs whose slot has been finalized; internal slots may have been eagerly freed by
    /// their parent. Raw — callers pass an already alias-resolved id.
    pub(super) fn read_result(&self, id: NodeId) -> Result<Live<'_, W>, &W::Error> {
        match &self.slots[id] {
            // `Sealed::read` re-anchors the carrier to this `&self` borrow, bounded by the bundled
            // frame `Rc`: `free_one`/`finalize` need `&mut self`, so for the whole `&self` borrow the
            // frame cannot drop, so the read borrow cannot outlive the backing region.
            SlotState::Done(Ok(w), ..) => Ok(w.read()),
            SlotState::Done(Err(e), ..) => Err(e),
            _ => panic!("result must be ready by the time it's read"),
        }
    }

    /// Read a finalized terminal together with the witness set that backs it (empty for a
    /// frameless / run-region value, which is already in a surviving region). The consumer-pull lift
    /// copies the value out of the producer frame the set names into the consumer's region before the
    /// producer slot frees.
    pub(super) fn read_result_with_frame(&self, id: NodeId) -> FramedRead<'_, W> {
        match &self.slots[id] {
            // `read` re-anchors to the `&self` read borrow; the bundled witness set (cloned out here
            // for the consumer-pull lift) pins the backing region over that borrow.
            SlotState::Done(Ok(w), ..) => Ok((w.read(), w.witness().clone())),
            SlotState::Done(Err(e), ..) => Err(e),
            _ => panic!("result must be ready by the time it's read"),
        }
    }

    pub(super) fn read(&self, id: NodeId) -> Live<'_, W> {
        match &self.slots[id] {
            SlotState::Done(Ok(w), ..) => w.read(),
            // The scheduler stores the opaque error but never inspects it, so the misuse panic
            // names the node, not the error value.
            SlotState::Done(Err(_), ..) => panic!("read called on errored node"),
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

    #[cfg(test)]
    pub(super) fn clear_node(&mut self, id: NodeId) {
        self.slots[id] = SlotState::Running;
    }

    #[cfg(test)]
    pub(super) fn set_result(&mut self, id: NodeId, output: Result<Live<'_, W>, W::Error>) {
        self.slots[id] = SlotState::Done(
            output
                .map(Erased::erase)
                .map(|e| Sealed::seal(Witnessed::from_erased(e, W::Witness::default()))),
        );
    }

    #[cfg(test)]
    pub(super) fn result_is_some(&self, id: NodeId) -> bool {
        matches!(self.slots[id], SlotState::Done(..))
    }

    #[cfg(test)]
    pub(super) fn result_is_none(&self, id: NodeId) -> bool {
        !matches!(self.slots[id], SlotState::Done(..))
    }

    #[cfg(test)]
    pub(super) fn free_list_snapshot(&self) -> Vec<NodeId> {
        self.free_list.clone()
    }

    #[cfg(test)]
    pub(super) fn free_list_len(&self) -> usize {
        self.free_list.len()
    }

    /// The live slot's opaque payload, or `None` once it has terminalized. The workload extracts
    /// the field it wants (e.g. the lexical chain). Test-only.
    #[cfg(test)]
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

    use std::rc::Rc;

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
        type Contract = UnitCarrier;
        type Continuation = UnitCarrier;
        // A trivial finalized-value witness: `Option<Rc<()>>` is a `Witness` (the blanket `Rc<F>` /
        // `Option<W>` impls), `Clone`, and `Default` (`None` = the empty / frameless witness).
        type Witness = Option<Rc<()>>;
    }

    fn sample_wait(carrier: Option<String>) -> NodeWork<TestWorkload> {
        NodeWork::new(Vec::new(), 0, (), carrier)
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
