//! The workload-independent DAG scheduler — a dynamic graph of dependency-linked nodes
//! with per-node memory frames, parameterized over a [`Workload`] and naming no Koan value,
//! error, scope, memory, or AST type.
//!
//! The execute loop drains via [`WorkQueues::pop_next`], which prioritizes in-flight slots
//! (sub-work and delivery-walk wakeups) ahead of fresh top-level dispatches. An owned dep's edge
//! never cycles — a new node's slot is allocated after every node it owns. A park edge can point at
//! an earlier producer, so a self-referential binding (`LET x = x`) forms a cycle that drains with
//! both slots still `PreRun`; the driver detects the leftover parked slots (via
//! [`Scheduler::unresolved`]) and surfaces a deadlock.
//!
//! Generic over a single [`Workload`] `W`: an inter-node value `W::Value` passed along dep edges, a
//! terminal error `W::Error`, a per-slot memory anchor `W::Frame` managed by `Rc`, a storage profile
//! `W::Profile` its destination regions are built over, and a one-shot `W::Continuation`. The
//! scheduler stores all of these and hands them back but inspects none beyond [`Anchor::owner`] and
//! the one behavioural hook, [`Workload::deliver`]. An embedder's
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
use crate::witnessed::{DropFree, Reattachable};
pub use deps::{Deps, ResolvedDeps};
// `pub` (not `pub(crate)`) like [`NodeId`]: it appears in the `pub` `AwaitContinue` builtin-finish
// type (via the `pub` `Action::AwaitDeps` field), so a narrower visibility would leak.
pub use deps::DepResults;
pub use edge_slab::{EdgeId, InstalledEdge};
pub use node_id::NodeId;
pub use workload::{
    Anchor, DeliveredTerminal, DeliveryDestination, Live, OwnerOf, SealedTerminal, Terminal,
    Workload,
};

/// A dynamic DAG of dispatch and execution work. See the module docs for the queue-priority and
/// cycle-detection contract.
pub struct Scheduler<W: Workload> {
    pub(in crate::scheduler) queues: WorkQueues,
    pub(in crate::scheduler) deps: DepGraph<W>,
    pub(in crate::scheduler) store: NodeStore<W>,
    pub(in crate::scheduler) edges: EdgeSlab<W>,
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
        // Replace return sites install their own edges, so the pending count is authoritative here.
        if self.deps.pending_count(id) == 0 {
            self.queues.push_after_replace(id);
        }
    }

    /// Slots still `PreRun` after the queue drained — each is parked on a dependency that can no
    /// longer fire (a dependency cycle). `(count, sample)` for the deadlock error, or `None` when
    /// every slot has reclaimed.
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

    /// True iff `producer` is forward-reachable from `consumer` — i.e. parking `consumer` on
    /// `producer` would deadlock (e.g. `LET Ty = Ty`, where the sub-Dispatch would park on its own
    /// ancestor). Caller surfaces a structured error instead of installing the park edge.
    ///
    /// The walk follows each slot's notify list to the consumers its edges name, skipping released
    /// entries (Inv-C) and consumer-less ones (a root or placeholder edge continues no chain).
    pub fn would_create_cycle(&self, producer: NodeId, consumer: NodeId) -> bool {
        if producer == consumer {
            return true;
        }
        let mut stack: Vec<NodeId> = vec![consumer];
        let mut visited: std::collections::HashSet<NodeId> = std::collections::HashSet::new();
        while let Some(node) = stack.pop() {
            if !visited.insert(node) {
                continue;
            }
            for &edge in self.deps.notify_of(node) {
                if self.edges.is_free(edge) {
                    continue;
                }
                let Some(next) = self.edges.consumer_of(edge) else {
                    continue;
                };
                if next == producer {
                    return true;
                }
                stack.push(next);
            }
        }
        false
    }

    /// **The one door a consumer's dep list is wired through.** The embedder hands the park *sources*
    /// it holds — edges its own bindings named — plus the producers it owns; the door mints the
    /// consumer's own slab edge per dep and hands back the realized list for
    /// [`NodeWork`](nodes::NodeWork) plus each park's **filled-or-parked** verdict. Producer
    /// `NodeId`s never leave the scheduler: resolving a source edge to one is this door's job.
    ///
    /// A park's edge inherits its source's destination, so a park on a placeholder delivers into the
    /// region that placeholder named; an owned dep's edge is destined at the consumer's own anchor
    /// region, which is where its sub-work's result belongs.
    ///
    /// The returned `Vec<InstalledEdge>` is index-aligned with `parks`. A *filled* verdict is the
    /// caller's to act on — its producer has already delivered, so an errored one propagates at once
    /// rather than waiting for a wake that will not come.
    ///
    /// It serves an already-allocated consumer slot, which is why it takes the dep list separately
    /// from the work; the submit-time sibling is
    /// [`alloc_node_with_parks`](Self::alloc_node_with_parks), which initializes a fresh row and its
    /// wires as one atomic step. Two doors, one primitive — so no wiring path can skew a row's
    /// invariants.
    pub fn install_deps(
        &mut self,
        consumer: NodeId,
        parks: &[EdgeId],
        owned: &[NodeId],
    ) -> (ResolvedDeps, Vec<InstalledEdge>) {
        let mut resolved = ResolvedDeps::new();
        let mut installed = Vec::with_capacity(parks.len());
        for &source in parks {
            let verdict = self.install_edge_from(source);
            self.edges.bind_consumer(verdict.edge_id(), consumer);
            // The mint already listed a parked edge on its producer, so only the pending half of
            // the wire is left: this consumer waits on one more unfilled edge. A filled verdict
            // waits on nothing — its producer has already delivered.
            if matches!(verdict, InstalledEdge::Parked(_)) {
                self.deps.count_pending(consumer);
            }
            resolved.push_park(verdict.edge_id());
            installed.push(verdict);
        }
        for &producer in owned {
            resolved.push_owned(self.wire_owned(consumer, producer));
        }
        (resolved, installed)
    }

    /// Mint one owned dep's edge: `producer` is freshly allocated sub-work of `consumer`, so it is
    /// pre-terminal by construction and the edge always parks. Its destination is the consumer's own
    /// anchor region — the sub-result lands where the consumer will read it.
    fn wire_owned(&mut self, consumer: NodeId, producer: NodeId) -> EdgeId {
        debug_assert!(
            self.store.is_live(producer),
            "an owned dep is freshly allocated sub-work, so it cannot have run yet",
        );
        let destination = Rc::clone(self.deps.anchor_clone(consumer).owner());
        let edge = self
            .edges
            .install_parked(producer, Some(consumer), &destination);
        self.deps.wire_parked(producer, edge, Some(consumer));
        edge
    }

    /// Wire one edge from `producer` toward a destination region, named by its owner: holding
    /// `destination` at this call is the wiring-time proof the caller pins that region
    /// ([design/dag-scheduler.md § Edges and the boundary](../design/dag-scheduler.md#edges-and-the-boundary)),
    /// which is why the door takes an owner and performs no coverage check of its own. The standing
    /// half of the lattice — the destination stays covered for the edge's life — rides the releasing
    /// owner's teardown verb.
    ///
    /// The edge always parks, and its producer must be live: a slot reclaims at its own finalize, so
    /// a `NodeId` that names a terminal producer names nothing. Both production callers — the run's
    /// roots and a binder's placeholder claim — wire immediately after allocating the slot. An edge
    /// wired from a *name the embedder already holds* takes
    /// [`install_edge_from`](Self::install_edge_from), which is where the filled branch lives.
    ///
    /// The edge carries no consumer: it wakes nobody and counts against no pending. It receives
    /// delivery like every other listed edge, which is what makes a root or a placeholder readable.
    pub fn install_edge(&mut self, producer: NodeId, destination: &Rc<OwnerOf<W>>) -> EdgeId {
        debug_assert!(
            self.store.is_live(producer),
            "install_edge names a live producer; a terminal one has already reclaimed its slot",
        );
        let edge = self.edges.install_parked(producer, None, destination);
        self.deps.wire_parked(producer, edge, None);
        edge
    }

    /// Wire a second edge to the producer behind `source`, **inheriting `source`'s destination
    /// region**: the consumer parking on an embedder's placeholder edge lands its delivery in the
    /// region that placeholder already named, not in the consumer's own
    /// ([design/dag-scheduler.md § Edges and the boundary](../design/dag-scheduler.md#edges-and-the-boundary)).
    /// Sound on the containment lattice without naming an owner here: `source` stands, so its owner
    /// stands, so the region it names is covered — and the new edge's own owner sits below that
    /// owner on the same lattice.
    ///
    /// Returns **filled-or-parked**, which is the whole of the readiness question
    /// ([§ Late wiring and install](../design/dag-scheduler.md#late-wiring-and-install)):
    ///
    /// - `source` **filled** — its producer already delivered into the destination both edges name,
    ///   so the new edge shares that resident. The per-destination dedup the walk applies, applied
    ///   again here: the second write into one region is free, and the shared cell's reach
    ///   description is retained by the destination for the region's life.
    /// - `source` **parked** — its producer is necessarily pre-terminal (unfilled ⇒ undelivered ⇒
    ///   slot alive), so the new edge parks on it too and registers on its notify list.
    pub fn install_edge_from(&mut self, source: EdgeId) -> InstalledEdge {
        match self.edges.producer_of(source) {
            Some(producer) => {
                let edge = self.edges.install_parked_inheriting(source, producer, None);
                self.deps.wire_parked(producer, edge, None);
                InstalledEdge::Parked(edge)
            }
            None => {
                let resident = self.edges.resident_duplicate(source);
                InstalledEdge::Filled(self.edges.install_filled_inheriting(source, resident, None))
            }
        }
    }

    /// Release one edge. Rides its owner's teardown verb — a consumer or frame teardown calls this
    /// with the names it still holds; an [`EdgeId`] is a name, not a lifecycle handle its holder
    /// manages. A parked edge's slab index is withheld from circulation until the walk that still
    /// lists it drops the entry (Inv-C). Panics in debug builds on a name whose index was recycled.
    pub fn release_edge(&mut self, id: EdgeId) {
        self.edges.release(id);
    }
}

/// The edge-keyed reads: an embedder holding an [`EdgeId`] reaches the delivered terminal resting on
/// it. There is no slot behind these — the producer reclaimed at its own finalize, and what the edge
/// holds is an ordinary resident of the destination region the edge names.
impl<W: Workload> Scheduler<W> {
    /// The delivered terminal's error, or `Ok(())` for a value terminal — the borrow-free
    /// success/failure probe that reads no value.
    pub fn edge_result_error(&self, id: EdgeId) -> Result<(), &W::Error> {
        self.edges.resident_error(id)
    }

    /// Open the delivered terminal at a rank-2 brand and hand it to `f` as
    /// `Result<Live<'b>, &W::Error>` — the destination-verb read, so the value nests inside the
    /// access rather than riding the `&self` borrow up-stack. The pin is the destination region's
    /// own owner, upgraded off its back-link: the value lives in that region, so the region's own
    /// liveness is what covers the read.
    pub fn read_edge_result_with<R>(
        &self,
        id: EdgeId,
        f: impl for<'b> FnOnce(Live<'b, W>) -> R,
    ) -> Result<R, &W::Error> {
        let host = self.edges.destination_host(id);
        Ok(self.edges.resident_ref(id)?.open_with(&host, f))
    }

    /// Duplicate the delivered terminal's sealed cell — value + reach description — leaving the
    /// edge's own copy intact. The consumer's step-start read: the cell is `Copy`-shaped data whose
    /// pointee lives in the destination region, so a consumer that holds that region takes this and
    /// re-brands it ([`Retained::brand_with`](crate::witnessed::Retained::brand_with)) rather than
    /// carrying pins of its own.
    pub fn edge_resident(&self, id: EdgeId) -> Result<SealedTerminal<W>, W::Error> {
        self.edges.resident_duplicate(id)
    }

    /// [`would_create_cycle`](Self::would_create_cycle) against the producer behind `source` — the
    /// one pre-wiring query a read-only decide still asks, since parking on an ancestor deadlocks
    /// rather than errors. A filled source can start no cycle: its producer is gone.
    pub fn would_create_cycle_from(&self, source: EdgeId, consumer: NodeId) -> bool {
        match self.edges.producer_of(source) {
            Some(producer) => self.would_create_cycle(producer, consumer),
            None => false,
        }
    }

    /// [`splice_forward`](Self::splice_forward) onto the producer behind `source`. A filled source
    /// never reaches here — the slot's step took the install verb's filled branch and forwarded the
    /// resident instead of emitting an alias.
    pub fn splice_forward_from(&mut self, slot: NodeId, source: EdgeId) {
        let producer = self
            .edges
            .producer_of(source)
            .expect("a spliced-out slot forwards a parked source; a filled one forwards its value");
        self.splice_forward(slot, producer);
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
    pub fn is_live(&self, id: NodeId) -> bool {
        self.store.is_live(id)
    }
    pub fn notify_list_iter(&self) -> impl Iterator<Item = (NodeId, &Vec<EdgeId>)> {
        self.deps.notify_list_iter()
    }
    pub fn free_list_snapshot(&self) -> Vec<NodeId> {
        self.store.free_list_snapshot()
    }
    pub fn free_list_len(&self) -> usize {
        self.store.free_list_len()
    }
    /// The producer a parked edge waits on, `None` for a delivered one.
    pub fn edge_producer(&self, id: EdgeId) -> Option<NodeId> {
        self.edges.producer_of(id)
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
    /// Whether the edge's slab entry is released — the stale-name probe a driver's reclamation
    /// canary reads a surviving notify list against.
    pub fn edge_is_free(&self, id: EdgeId) -> bool {
        self.edges.is_free(id)
    }
    pub fn edge_free_list_len(&self) -> usize {
        self.edges.free_list_len()
    }
    pub fn edge_slab_len(&self) -> usize {
        self.edges.len()
    }
    pub fn pending_count(&self, id: NodeId) -> usize {
        self.deps.pending_count(id)
    }
    /// The realized dep list the install door wrote onto a pre-run slot.
    pub fn stored_deps(&self, id: NodeId) -> &ResolvedDeps {
        self.store.stored_deps(id)
    }
}
