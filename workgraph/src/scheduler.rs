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
use edge_slab::EdgeSlab;
use node_store::NodeStore;
use nodes::{NodeWork, StoredWork, seal_work};
use work_queues::WorkQueues;

mod alloc;
mod dep_graph;
mod deps;
mod edge_slab;
mod lifecycle;
mod node_id;
mod node_store;
pub mod nodes;
mod splice;
mod work_queues;
mod workload;

/// The scheduler's white-box slates: the owned-tier continuation slot under Miri (a parked
/// droppable continuation dropped unopened under the seal's own pin, and the park → wake → open →
/// run round trip) and the edge slab's alloc/release recycling and install branches.
#[cfg(test)]
mod tests;

// The lifetime-erasure carrier substrate lives in the top-level `witnessed` module (below both
// `machine` and `scheduler`); imported here so the scheduler's carriers name it unqualified. A
// private `use`: `witnessed` is the one public path to these types, and a `pub use` here would
// double it for every one of them.
use crate::witnessed::{Carrier, Delivered, DropFree, Reattachable};
pub use deps::{Deps, ResolvedDeps};
// `pub` (not `pub(crate)`) like [`NodeId`]: it appears in the `pub` `AwaitContinue` builtin-finish
// type (via the `pub` `Action::AwaitDeps` field), so a narrower visibility would leak.
pub use deps::DepResults;
pub use edge_slab::{EdgeId, InstalledEdge};
pub use node_id::NodeId;
pub use workload::{Anchor, DeliveredTerminal, Live, OwnerOf, SealedTerminal, Terminal, Workload};

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
    pub(in crate::scheduler) edges: EdgeSlab<OwnerOf<W>>,
}

impl<W: Workload> Scheduler<W> {
    pub fn new() -> Self {
        Self {
            queues: WorkQueues::new(),
            deps: DepGraph::new(),
            store: NodeStore::new(),
            edges: EdgeSlab::new(),
        }
    }

    /// Pop the next ready slot — the run loop's iterator (in-flight slots ahead of fresh
    /// dispatches). `None` when the queue drains.
    pub fn pop_next(&mut self) -> Option<NodeId> {
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
    ) -> (StoredWork<W>, Rc<W::Frame>, Option<Rc<W::Frame>>) {
        (
            self.store.take_for_run(id),
            self.deps.anchor_clone(id),
            self.deps.take_handoff(id),
        )
    }

    /// Reinstall a tail-replaced slot's work and re-enqueue it if its deps are already satisfied —
    /// the whole `Replace` apply in one step. `anchor` is the reinstalled incarnation's memory anchor
    /// at a framed tail replace (`None` for a frameless `Inherit` replace, which turns over no
    /// region); swapping it in parks the displaced anchor as the TCO handoff so the retiring region
    /// is released only after the reinstalled incarnation adopts the carried arguments.
    pub fn replace(&mut self, id: NodeId, work: NodeWork<'_, W>, anchor: Option<Rc<W::Frame>>) {
        // Seal the incoming continuation against the incarnation's *effective* anchor — the new one
        // at a framed replace, the row's current one at a frameless `Inherit` replace — so the
        // resting continuation is pinned by the anchor whose region it will read.
        let stored = match &anchor {
            Some(new) => seal_work(work, new),
            None => seal_work(work, &self.deps.anchor_clone(id)),
        };
        // On a framed replace, swap the row's anchor for the new incarnation's and park the displaced
        // one as the reinstalled slot's TCO handoff hold; the run loop holds it across the reinstalled
        // incarnation's first step, so the retiring region is released only after the carried
        // arguments are adopted. On a frameless `Inherit` replace, keep the current anchor and clear
        // any handoff — it turns over no region.
        match anchor {
            Some(new) => {
                let displaced = self.deps.set_anchor(id, new);
                self.deps.set_handoff(id, Some(displaced));
            }
            None => self.deps.set_handoff(id, None),
        }
        self.store.reinstall(id, stored);
        // Replace return sites install their own edges (or clear the slot's dep edges for tail
        // rewrites), so the pending count is authoritative here.
        if self.deps.pending_count(id) == 0 {
            self.queues.push_after_replace(id);
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
        self.deps.anchor_of(id)
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
        let pin = self.deps.retained_owner(target);
        self.store.read_result_with(target, pin.as_ref(), f)
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
    /// ([`dep_carrier`](Self::dep_carrier)) paired with its retained producer-frame owner and the
    /// terminal's owned foreign bundle, both cloned from the retention hold and unioned by the
    /// envelope into one member set (the producer frame becomes an ordinary member there), so a
    /// consumer reads the value under a pin sourced from the retention hold rather than threaded
    /// per call site. Sound because the retention hold is active
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
        let target = self.resolve_alias(id);
        let host = self
            .deps
            .retained_owner(target)
            .expect("a pull-able dep's retention hold is active (seeded at every finalize)");
        // Clone the terminal's owned foreign bundle out of the hold — the reach was captured at
        // finalize and threaded in, never re-derived from the carrier's description here.
        let foreign = self
            .deps
            .retained_foreign(target)
            .expect("a pull-able dep's retention hold carries its foreign bundle");
        Ok(Delivered::hosted(
            cell,
            host,
            crate::witnessed::StepCoverage(foreign),
        ))
    }

    /// Re-home a finalized terminal (relocated into a surviving region, bundled with the witness set
    /// of any per-call source it still reaches), dropping the pinned producer frame. The drain
    /// boundary uses this for consumer-less roots. Resolves a bare-name alias so the real producer's
    /// frame — not the alias slot — is released.
    ///
    /// Takes the same [`DeliveredTerminal`] currency [`finalize`](Self::finalize) does, and for the
    /// same reason: the relocation's product carries its own reach. The envelope's coverage is
    /// *dropped* here rather than re-seeded — the value moved into a region that outlives the
    /// scheduler, so the transit pins have nothing left to keep alive.
    pub fn rehome_terminal(&mut self, id: NodeId, output: Result<DeliveredTerminal<W>, W::Error>) {
        let target = self.resolve_alias(id);
        // The re-homed terminal has no per-call producer frame to retain — its value moved into a
        // surviving region — so any hold seeded at its finalize is released here (its count is zero
        // by construction: a consumer-less root has no parked destination).
        self.deps.drop_retain(target);
        self.store
            .rehome_terminal(target, output.map(Delivered::into_cell));
    }

    /// True iff `producer` is forward-reachable from `consumer`
    /// (`DepGraph::would_create_cycle`).
    pub fn would_create_cycle(&self, producer: NodeId, consumer: NodeId) -> bool {
        self.deps.would_create_cycle(producer, consumer)
    }

    /// Install a resolved dep list's edges against `consumer`: each park a `Notify` edge (the
    /// consumer reads the producer but does not own it), each owned dep an `Owned` edge (cascade-freed
    /// on success). Both kinds resolve a bare-name-forward alias first, and an already-finalized
    /// producer takes no edge at all — its value is read directly, so the consumer never parks on a
    /// slot that will not fire — but its pull on the producer's retained frame is counted, to be
    /// discharged after the read.
    ///
    /// **The one door an embedder wires a consumer slot's dep edges through** (slab edges have
    /// their own door, [`install_edge`](Self::install_edge)). It serves an already-allocated
    /// consumer slot, which is why it takes the dep list separately from the work. The submit-time
    /// path does not route here: [`alloc_node`](Self::alloc_node) initializes a fresh row and its
    /// edges as one atomic step, and takes ownership of the sub-work it spawns — so an
    /// already-finalized *owned* dep still records its backward edge there, because that edge is
    /// the ownership record the error-path cascade walks. The two are deliberately not the same
    /// operation.
    pub fn install_edges(&mut self, deps: &ResolvedDeps, consumer: NodeId) {
        for &producer in deps.parks() {
            self.add_park_edge(producer, consumer);
        }
        for &producer in deps.owned() {
            self.add_owned_edge(producer, consumer);
        }
    }

    /// Wire one edge from `producer` toward a destination region, named by its owner: holding
    /// `destination` at this call is the wiring-time proof the caller pins that region
    /// ([design/dag-scheduler.md § Edges and the boundary](../design/dag-scheduler.md#edges-and-the-boundary)),
    /// which is why the door takes an owner and performs no coverage check of its own. The standing
    /// half of the lattice — the destination stays covered for the edge's life — rides the releasing
    /// owner's teardown verb.
    ///
    /// Returns **filled-or-parked**: filled when the producer (alias resolved) has already
    /// finalized, so the consumer reads its value rather than waiting on a slot that will not fire;
    /// parked otherwise. The edge stores the destination as a raw pointer plus a debug-only weak
    /// shadow of its owner; the deref that reads it is the delivery walk's
    /// ([delivery-at-finalize](../roadmap/delivery-at-finalize.md)).
    pub fn install_edge(
        &mut self,
        producer: NodeId,
        destination: &Rc<OwnerOf<W>>,
    ) -> InstalledEdge {
        let producer = self.resolve_alias(producer);
        let pending = (!self.store.is_result_ready(producer)).then_some(producer);
        self.edges.install(pending, destination)
    }

    /// Release one edge. Rides its owner's teardown verb — a consumer or frame teardown calls this
    /// with the names it still holds; an [`EdgeId`] is a name, not a lifecycle handle its holder
    /// manages. Panics in debug builds on a name whose edge was already released.
    pub fn release_edge(&mut self, id: EdgeId) {
        self.edges.release(id);
    }
}

impl<W: Workload> Default for Scheduler<W> {
    fn default() -> Self {
        Self::new()
    }
}

/// `#[cfg(any(test, feature = "test-hooks"))]` forwarders that let the driver's white-box tests
/// poke slot/edge state without exposing the `store` / `deps` / `queues` / `edges` fields. Each
/// wraps an already-test-only primitive on the inner store, dep graph, or edge slab. The
/// `test-hooks` feature widens this for an embedder compiling as a dependent crate, where
/// `cfg(test)` is off.
#[cfg(any(test, feature = "test-hooks"))]
impl<W: Workload> Scheduler<W> {
    pub fn clear_node(&mut self, id: NodeId) {
        self.store.clear_node(id);
    }
    pub fn set_result(
        &mut self,
        id: NodeId,
        output: Result<Live<'_, W>, W::Error>,
        carrier: crate::witnessed::Carrier<OwnerOf<W>>,
    ) {
        self.store.set_result(id, output, carrier);
    }
    /// Seed a retention hold on a synthetically-finalized slot ([`Self::set_result`] writes the
    /// terminal but runs no finalize, so no hold exists) — [`Self::dep_delivered`] requires one for
    /// every pull-able dep. `foreign` is the hold's owned foreign bundle: pass
    /// [`StepCoverage::empty`](crate::witnessed::StepCoverage::empty) for a slot that reaches nothing, or a
    /// real bundle to exercise the foreign half's pull-count-zero release timeline.
    pub fn seed_retention(
        &mut self,
        id: NodeId,
        owner: Rc<OwnerOf<W>>,
        foreign: crate::witnessed::StepCoverage<OwnerOf<W>>,
        pulls: usize,
    ) {
        self.deps.seed_retain(id, owner, foreign.0, pulls);
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
    pub fn notify_list_iter(&self) -> impl Iterator<Item = (NodeId, &Vec<NodeId>)> {
        self.deps.notify_list_iter()
    }
    pub fn free_list_snapshot(&self) -> Vec<NodeId> {
        self.store.free_list_snapshot()
    }
    pub fn free_list_len(&self) -> usize {
        self.store.free_list_len()
    }
    /// The producer a parked edge waits on, `None` for a filled one.
    pub fn edge_producer(&self, id: EdgeId) -> Option<NodeId> {
        self.edges.producer_of(id)
    }
    /// Re-point a parked edge at another producer — the alias splice's half of edge wiring.
    pub fn rewrite_edge_producer(&mut self, id: EdgeId, producer: NodeId) {
        self.edges.rewrite_producer(id, producer);
    }
    /// Whether the edge's recorded destination is `owner`'s region — pointer identity, no deref.
    pub fn edge_destination_is(&self, id: EdgeId, owner: &Rc<OwnerOf<W>>) -> bool {
        std::ptr::eq(
            self.edges.destination_region(id),
            crate::witnessed::RegionOwner::region(&**owner) as *const _,
        )
    }
    /// The destination owner behind the edge's debug shadow, upgraded — `None` once it has died.
    #[cfg(debug_assertions)]
    pub fn edge_destination_owner(&self, id: EdgeId) -> Option<Rc<OwnerOf<W>>> {
        self.edges.destination_owner(id)
    }
    pub fn edge_free_list_len(&self) -> usize {
        self.edges.free_list_len()
    }
    pub fn edge_slab_len(&self) -> usize {
        self.edges.len()
    }
    pub fn set_dep_edges(&mut self, id: NodeId, edges: Vec<DepEdge>) {
        self.deps.set_dep_edges(id, edges);
    }
    pub fn dep_edges_at(&self, id: NodeId) -> &[DepEdge] {
        self.deps.dep_edges_at(id)
    }
}
