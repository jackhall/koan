//! The workload-independent DAG scheduler — a dynamic graph of dependency-linked nodes
//! with per-node memory frames, parameterized over a [`Workload`] and naming no Koan value,
//! error, scope, memory, or AST type.
//!
//! The execute loop drains via [`WorkQueues::pop_next`], which prioritizes in-flight slots
//! (sub-work and notify-walk wakeups) ahead of fresh top-level dispatches. Owned edges never
//! cycle — a new node's `NodeId` is strictly greater than every node it owns. Park (`Notify`)
//! edges can point at an earlier producer, so a self-referential binding (`LET x = x`) forms
//! a cycle that drains with both slots still `PreRun`; the driver detects the leftover parked
//! slots (via [`Scheduler::unresolved`]) and surfaces a deadlock.
//!
//! Generic over a single [`Workload`] `W`: an inter-node value `W::Value` passed along dep edges, a
//! terminal error `W::Error`, a per-slot memory anchor `W::Frame` managed by `Rc` (whose projected
//! region owner the scheduler retains for delivery), and a one-shot `W::Continuation`. The scheduler
//! stores all of these and hands them back but inspects none beyond [`Anchor::owner`]. An embedder's
//! interpreter instantiates the scheduler and drives it through the inherent-method contract; Koan's
//! `machine` module is the first such instantiation.
//!
//! See design/execution/README.md and design/memory-model.md.

use std::rc::Rc;

use dep_graph::DepGraph;
use node_store::NodeStore;
use nodes::NodeWork;
use work_queues::WorkQueues;

mod alloc;
mod dep_graph;
mod deps;
mod lifecycle;
mod node_id;
mod node_store;
pub mod nodes;
mod splice;
mod work_queues;
mod workload;

// The lifetime-erasure carrier substrate lives in the top-level `witnessed` module (below both
// `machine` and `scheduler`); re-exported here so the scheduler's carriers name it unqualified.
pub use crate::witnessed::{
    Carrier, ComposeWitness, Delivered, Erased, Reattachable, Sealed, Witnessed,
};
pub use deps::{Deps, ProducerDisposition, ResolvedDeps};
// `pub` (not `pub(crate)`) like [`NodeId`]: it appears in the `pub` `AwaitContinue` builtin-finish
// type (via the `pub` `Action::AwaitDeps` field), so a narrower visibility would leak.
pub use deps::DepResults;
pub use node_id::NodeId;
pub use workload::{Anchor, Live, OwnerOf, SealedTerminal, Terminal, Workload};

/// Re-exported for the driver's white-box reclaim tests (the only cross-module user of the edge
/// kind); production driver code never names it. Widened to `test-hooks` so the embedder's own
/// white-box tests (compiled as a dependent crate, where `cfg(test)` is off) can reach it too.
#[cfg(any(test, feature = "test-hooks"))]
pub use dep_graph::DepEdge;

/// A dynamic DAG of dispatch and execution work. See the module docs for the queue-priority and
/// cycle-detection contract.
pub struct Scheduler<W: Workload> {
    pub(in crate::scheduler) queues: WorkQueues,
    pub(in crate::scheduler) deps: DepGraph<W>,
    pub(in crate::scheduler) store: NodeStore<W>,
}

impl<W: Workload> Scheduler<W> {
    pub fn new() -> Self {
        Self {
            queues: WorkQueues::new(),
            deps: DepGraph::new(),
            store: NodeStore::new(),
        }
    }

    /// Pop the next ready slot index — the run loop's iterator (in-flight slots ahead of fresh
    /// dispatches). `None` when the queue drains.
    pub fn pop_next(&mut self) -> Option<usize> {
        self.queues.pop_next()
    }

    /// Take a slot's stored work to run it (`PreRun` → `Running`), together with a clone of the
    /// slot's memory anchor (kept on the row) and its pending TCO handoff — the displaced
    /// incarnation's anchor a framed tail [`replace`](Self::replace) parked on it. The slot sits
    /// empty until the driver finalizes or `replace`s it. The caller holds the returned handoff `Rc`
    /// across the step: drop order frees the retiring region only after the reinstalled incarnation
    /// adopts the carried arguments out of it (`None` for any slot with no pending handoff — a first
    /// run, or a frameless replace).
    // The (work, anchor, handoff) triple reads clearer inline than split into a named alias.
    #[allow(clippy::type_complexity)]
    pub fn take_for_run(
        &mut self,
        id: NodeId,
    ) -> (NodeWork<W>, Rc<W::Frame>, Option<Rc<W::Frame>>) {
        (
            self.store.take_for_run(id),
            self.deps.anchor_clone(id.index()),
            self.deps.take_handoff(id.index()),
        )
    }

    /// Reinstall a tail-replaced slot's work and re-enqueue it if its deps are already satisfied —
    /// the whole `Replace` apply in one step. `anchor` is the reinstalled incarnation's memory anchor
    /// at a framed tail replace (`None` for a frameless `Inherit` replace, which turns over no
    /// region); swapping it in parks the displaced anchor as the TCO handoff so the retiring region
    /// is released only after the reinstalled incarnation adopts the carried arguments.
    pub fn replace(&mut self, id: NodeId, work: NodeWork<W>, anchor: Option<Rc<W::Frame>>) {
        // On a framed replace, swap the row's anchor for the new incarnation's and park the displaced
        // one as the reinstalled slot's TCO handoff hold; the run loop holds it across the reinstalled
        // incarnation's first step, so the retiring region is released only after the carried
        // arguments are adopted. On a frameless `Inherit` replace, keep the current anchor and clear
        // any handoff — it turns over no region.
        match anchor {
            Some(new) => {
                let displaced = self.deps.set_anchor(id.index(), new);
                self.deps.set_handoff(id.index(), Some(displaced));
            }
            None => self.deps.set_handoff(id.index(), None),
        }
        self.store.reinstall(id, work);
        // Replace return sites install their own edges (or clear the slot's dep edges for tail
        // rewrites), so the pending count is authoritative here.
        if self.deps.pending_count(id.index()) == 0 {
            self.queues.push_after_replace(id.index());
        }
    }

    /// Slots still `PreRun` after the queue drained — each is parked on a dependency that can no
    /// longer fire (a dependency cycle). `(count, sample)` for the deadlock error, or `None` when
    /// every slot is terminal.
    pub fn unresolved(&self) -> Option<(usize, String)> {
        self.store.unresolved()
    }

    /// A clone of the slot's memory anchor, or `None` for a slot with none installed. Test-only.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn anchor_of(&self, id: NodeId) -> Option<Rc<W::Frame>> {
        self.deps.anchor_of(id.index())
    }

    pub fn len(&self) -> usize {
        self.store.len()
    }
    pub fn is_empty(&self) -> bool {
        self.store.is_empty()
    }

    /// An errored sub counts as ready — parents short-circuit on it. Follows a bare-name-forward
    /// alias to the real producer (see [`splice`](self::splice)).
    pub fn is_result_ready(&self, id: NodeId) -> bool {
        self.store.is_result_ready(self.resolve_alias(id))
    }

    /// Open a finalized terminal at a rank-2 brand and hand it to `f` as
    /// `Result<Live<'b>, &W::Error>` — the destination-verb read, so the value nests inside the
    /// access rather than riding the `&self` borrow up-stack. Follows a bare-name-forward alias.
    pub fn read_result_with<R>(
        &self,
        id: NodeId,
        f: impl for<'b> FnOnce(Live<'b, W>) -> R,
    ) -> Result<R, &W::Error> {
        let target = self.resolve_alias(id);
        // The retained producer frame owner pins the value across the open (`None` for a frameless /
        // run-region producer); held in `pin` for the duration of the read.
        let pin = self.deps.retained_owner(target.index());
        self.store.read_result_with(target, pin.as_ref(), f)
    }

    /// The retained producer-frame owner of a finalized dep, or `None` for a frameless / run-region
    /// producer. Private: a bare frame pin never escapes the scheduler — consumers receive it
    /// paired with the sealed carrier as a [`dep_delivered`](Self::dep_delivered) envelope.
    fn dep_host(&self, id: NodeId) -> Option<Rc<OwnerOf<W>>> {
        self.deps.retained_owner(self.resolve_alias(id).index())
    }

    /// The terminal's error, or `Ok(())` for a value terminal — the borrow-free success/failure
    /// probe that reads no value. Follows a bare-name-forward alias to the real producer.
    pub fn result_error(&self, id: NodeId) -> Result<(), &W::Error> {
        self.store.result_error(self.resolve_alias(id))
    }

    /// Duplicate a finalized terminal's sealed carrier (value + witness set), leaving the producer's
    /// own seal intact — the consumer-pull lift hands each dep this so a construction finish folds it
    /// witnessed, naming the reach on the carrier rather than reconstructing it. Follows a
    /// bare-name-forward alias to the real producer (which holds the sole copy).
    pub fn dep_carrier(&self, id: NodeId) -> Result<SealedTerminal<W>, &W::Error> {
        self.store.dep_carrier(self.resolve_alias(id))
    }

    /// A finalized dep as a **delivery envelope**: its duplicated sealed carrier
    /// ([`dep_carrier`](Self::dep_carrier)) paired with its retained producer-frame owner
    /// ([`dep_host`](Self::dep_host)), so a consumer reads the value under a pin sourced from the
    /// retention hold rather than threaded per call site. Sound because the retention hold is active
    /// while any consumer edge is undischarged (the pinning invariant) — and total for the same
    /// reason: every finalize seeds a hold (the run frame's storage owns the run region), so a
    /// pull-able dep always has a retained owner. Follows a bare-name-forward alias to the real
    /// producer. Relocations ride the envelope too
    /// ([`Delivered::transfer_into`](crate::witnessed::Delivered)); the scheduler exposes no
    /// separate transfer verb.
    // The three-parameter envelope over a witnessed `Result` reads clearer inline than split apart.
    #[allow(clippy::type_complexity)]
    pub fn dep_delivered(
        &self,
        id: NodeId,
    ) -> Result<Delivered<W::Value, Carrier<OwnerOf<W>>, OwnerOf<W>>, &W::Error> {
        let cell = self.dep_carrier(id)?;
        let host = self
            .dep_host(id)
            .expect("a pull-able dep's retention hold is active (seeded at every finalize)");
        Ok(Delivered::hosted(cell, host))
    }

    /// Re-home a finalized terminal (relocated into a surviving region, bundled with the witness set
    /// of any per-call source it still reaches), dropping the pinned producer frame. The drain
    /// boundary uses this for consumer-less roots. Resolves a bare-name alias so the real producer's
    /// frame — not the alias slot — is released.
    pub fn rehome_terminal(&mut self, id: NodeId, output: Result<Terminal<W>, W::Error>) {
        let target = self.resolve_alias(id);
        // The re-homed terminal has no per-call producer frame to retain — its value moved into a
        // surviving region — so any hold seeded at its finalize is released here (its count is zero
        // by construction: a consumer-less root has no parked destination).
        self.deps.drop_retain(target.index());
        self.store.rehome_terminal(target, output);
    }

    /// True iff `producer` is forward-reachable from `consumer`
    /// (`DepGraph::would_create_cycle`).
    pub fn would_create_cycle(&self, producer: NodeId, consumer: NodeId) -> bool {
        self.deps.would_create_cycle(producer, consumer)
    }

    /// Classify "can this consumer depend on `producer`?" — the shared park-ladder check order
    /// (ready → errored → would-cycle → park), leaving the caller its own policy per arm. `consumer`
    /// is `None` at a site with no consumer id in scope (a leaf park), where a cycle can never be
    /// classified. Follows a bare-name-forward alias through the `is_result_ready` / `result_error`
    /// facades. See [`ProducerDisposition`](self::deps::ProducerDisposition).
    pub fn producer_disposition(
        &self,
        producer: NodeId,
        consumer: Option<NodeId>,
    ) -> ProducerDisposition<'_, W::Error> {
        if self.is_result_ready(producer) {
            match self.result_error(producer) {
                Err(e) => ProducerDisposition::Errored(e),
                Ok(()) => ProducerDisposition::Ready,
            }
        } else if consumer.is_some_and(|c| self.would_create_cycle(producer, c)) {
            ProducerDisposition::Cycle
        } else {
            ProducerDisposition::Park
        }
    }

    /// Install a resolved dep list's edges against `consumer`: each park a `Notify` edge (the
    /// consumer reads the producer but does not own it), each owned dep an `Owned` edge (cascade-freed
    /// on success). Both route the alias-resolving [`splice`](self::splice) facades, which drop the
    /// edge for an already-finalized producer. The apply harness uses this for an
    /// already-allocated consumer slot; the submit-time path installs its own edges in
    /// [`alloc`](self::alloc).
    pub fn install_edges(&mut self, deps: &ResolvedDeps, consumer: NodeId) {
        for &producer in deps.parks() {
            self.add_park_edge(producer, consumer);
        }
        for &producer in deps.owned() {
            self.add_owned_edge(producer, consumer);
        }
    }
}

impl<W: Workload> Default for Scheduler<W> {
    fn default() -> Self {
        Self::new()
    }
}

/// `#[cfg(any(test, feature = "test-hooks"))]` forwarders that let the driver's white-box tests
/// poke slot/edge state without exposing the `store` / `deps` / `queues` fields. Each wraps an
/// already-test-only primitive on the inner store or dep graph. The `test-hooks` feature widens
/// this for an embedder compiling as a dependent crate, where `cfg(test)` is off.
#[cfg(any(test, feature = "test-hooks"))]
impl<W: Workload> Scheduler<W> {
    pub fn clear_node(&mut self, id: NodeId) {
        self.store.clear_node(id);
    }
    pub fn set_result(&mut self, id: NodeId, output: Result<Live<'_, W>, W::Error>) {
        self.store.set_result(id, output);
    }
    /// Seed a retention hold on a synthetically-finalized slot ([`Self::set_result`] writes the
    /// terminal but runs no finalize, so no hold exists) — [`Self::dep_delivered`] requires one for
    /// every pull-able dep.
    pub fn seed_retention(&mut self, id: NodeId, owner: Rc<OwnerOf<W>>, pulls: usize) {
        self.deps.seed_retain(id.index(), owner, pulls);
    }
    pub fn result_is_none(&self, id: NodeId) -> bool {
        self.store.result_is_none(id)
    }
    pub fn result_is_some(&self, id: NodeId) -> bool {
        self.store.result_is_some(id)
    }
    pub fn is_live(&self, id: NodeId) -> bool {
        self.store.is_live(id)
    }
    pub fn notify_list_iter(&self) -> impl Iterator<Item = (usize, &Vec<usize>)> {
        self.deps.notify_list_iter()
    }
    pub fn free_list_snapshot(&self) -> Vec<NodeId> {
        self.store.free_list_snapshot()
    }
    pub fn free_list_len(&self) -> usize {
        self.store.free_list_len()
    }
    pub fn set_dep_edges(&mut self, idx: usize, edges: Vec<DepEdge>) {
        self.deps.set_dep_edges(idx, edges);
    }
    pub fn dep_edges_at(&self, idx: usize) -> &[DepEdge] {
        self.deps.dep_edges_at(idx)
    }
}
