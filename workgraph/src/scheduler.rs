//! The workload-independent DAG scheduler — a dynamic graph of dependency-linked nodes
//! with per-node memory frames, parameterized over a [`Workload`] and naming no Koan value,
//! error, scope, memory, or AST type.
//!
//! The run protocol is one door: [`Scheduler::drain`] pops each ready slot (in-flight slots — sub-work
//! and delivery-walk wakeups — ahead of fresh top-level dispatches), reads the slot's dep residents,
//! releases its dep edges, hands the step to the embedder's callback, and applies the
//! [`StepVerdict`] it returns. A spawned dep's edge never cycles — a new node's slot is allocated
//! after every node it owns — and a park edge never cycles either: the embedder's dispatch rule
//! keeps every claim wait lexically backward, and the install door asserts that invariant in debug
//! builds. Slots still parked when the queues drain are therefore an invariant breach; `drain`
//! surfaces them as its deadlock error rather than panicking on a result read.
//!
//! Generic over a single [`Workload`] `W`: the scheduler stores `W`'s value, error, anchor,
//! profile, and continuation types and hands them back, but inspects none of them beyond
//! [`Anchor::owner`] and the two behavioural hooks, [`Workload::deliver`] and
//! [`Workload::retiring`].
//!
//! See [design/dag-scheduler.md](../design/dag-scheduler.md) and
//! [design/reach.md](../design/reach.md).

use std::rc::Rc;

use dep_graph::DepGraph;
use edge_slab::EdgeSlab;
use node_store::NodeStore;
use nodes::{NodeWork, StoredWork, seal_work};
use work_queues::WorkQueues;

mod alloc;
mod dep_graph;
mod deps;
mod drain;
mod edge_slab;
mod lifecycle;
mod node_id;
mod node_store;
pub mod nodes;
mod splice;
mod work_queues;
mod workload;

#[cfg(test)]
mod tests;

// Kept private: `witnessed` is the public path to these, and re-exporting them here would double it.
use crate::witnessed::{DropFree, Reattachable};
// `pub`, because an embedder assembles a dep list before submitting it.
pub use deps::{Dep, Deps};
// The realized dep list is scheduler currency: the install doors write it onto the slot's dep row
// and `drain` consumes it there, so no embedder ever holds one.
pub(crate) use deps::ResolvedDeps;
pub use drain::{DrainDeadlock, Step, StepVerdict};
pub use edge_slab::{EdgeId, InstalledEdge};
pub use node_id::NodeId;
pub use workload::{
    Anchor, DeliveredTerminal, DeliveryDestination, Live, OwnerOf, SealedTerminal, Terminal,
    Workload,
};

/// A dynamic DAG of dispatch and execution work. See the module docs for the queue-priority and
/// cycle contract.
pub struct Scheduler<W: Workload> {
    // Declaration order is drop order and is load-bearing: `deps` holds each slot's anchor row, so
    // it must drop before `store` releases the slots whose sealed continuations run destructor glue
    // against those regions.
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

    /// Pop the next ready slot, in-flight slots ahead of fresh dispatches.
    pub(in crate::scheduler) fn pop_next(&mut self) -> Option<NodeId> {
        self.queues.pop_next()
    }

    /// Take a slot's stored work to run it (`PreRun` → `Running`). The slot sits empty until the
    /// drain applies the step's verdict.
    pub(in crate::scheduler) fn take_for_run(
        &mut self,
        id: NodeId,
    ) -> (StoredWork<W>, Rc<W::Frame>) {
        (self.store.take_for_run(id), self.deps.anchor_clone(id))
    }

    /// Reinstall a tail-replaced slot's work and re-enqueue it if its deps are already satisfied.
    /// `anchor` is the reinstalled incarnation's memory anchor at a framed tail replace (`None` for
    /// a frameless replace, which turns over no region).
    ///
    /// Returns the **displaced** anchor a framed replace retires, and the scheduler keeps no hold of
    /// its own past this call: an embedder relocates whatever the next incarnation reads into the
    /// new anchor's region *before* replacing, so ordering the retiring region's free stays a local
    /// of the apply path rather than a row field spanning a step
    /// ([design/reach.md § Retention model](../design/reach.md#retention-model)).
    pub(in crate::scheduler) fn replace(
        &mut self,
        id: NodeId,
        work: NodeWork<'_, W>,
        anchor: Option<Rc<W::Frame>>,
    ) -> Option<Rc<W::Frame>> {
        // Seal against the incarnation's *effective* anchor — the new one at a framed replace, the
        // row's current one otherwise — so the resting continuation is pinned by the anchor whose
        // region it will read.
        let stored = match &anchor {
            Some(new) => seal_work(work, new),
            None => seal_work(work, &self.deps.anchor_clone(id)),
        };
        let displaced = anchor.map(|new| self.deps.set_anchor(id, new));
        self.store.reinstall(id, stored);
        // Replace return sites install their own edges, so the pending count is authoritative here.
        if self.deps.pending_count(id) == 0 {
            self.queues.push_after_replace(id);
        }
        displaced
    }

    /// Slots still `PreRun` after the queue drained — each parked on a dependency that can no longer
    /// fire — as `(count, first stuck slot's anchor)`. The sample is the workload's own anchor type
    /// so an embedder renders the deadlock report off data it wrote itself; the scheduler stores
    /// nothing diagnostic.
    pub(in crate::scheduler) fn unresolved(&self) -> Option<(usize, Rc<W::Frame>)> {
        self.store
            .unresolved()
            .map(|(count, first)| (count, self.deps.anchor_clone(first)))
    }

    /// A clone of the slot's memory anchor, or `None` for a slot with none installed.
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
    /// `producer` would deadlock. Debug-assertion-only: the embedder's dispatch rule keeps every
    /// claim wait lexically backward, so a cycling park is unconstructible upstream and this walk
    /// only guards that invariant.
    ///
    /// The walk skips released notify entries (Inv-C) and consumer-less ones — a root or placeholder
    /// edge continues no chain.
    fn would_create_cycle(&self, producer: NodeId, consumer: NodeId) -> bool {
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

    /// **The one door a consumer's dep list is wired through**: the dep row lives on a
    /// scheduler-private `DepGraph`, so no embedder-reachable path writes one. The embedder hands
    /// the dep *sources* it holds — one edge per dep, in dep order — and the door mints the
    /// consumer's own slab edge off each. Producer `NodeId`s never leave the scheduler: resolving
    /// a source edge to one is this door's job.
    ///
    /// Every dep's edge inherits its source's destination, uniformly: a dep on a placeholder
    /// delivers into the region that placeholder named, a dep on spawned sub-work into the region
    /// that sub-work's source named.
    ///
    /// The returned `Vec<InstalledEdge>` is index-aligned with `sources`. A *filled* verdict is the
    /// caller's to act on — its producer has already delivered, so an errored one propagates at once
    /// rather than waiting for a wake that will not come. A *parked* one is asserted acyclic in
    /// debug builds: the embedder's lexical dispatch rule means a park can never wait forward, so a
    /// cycling edge here is an upstream bug, not a runtime condition.
    pub fn install_deps(&mut self, consumer: NodeId, sources: &[EdgeId]) -> Vec<InstalledEdge> {
        let mut installed = Vec::with_capacity(sources.len());
        for &source in sources {
            let verdict = self.install_edge_from(source);
            self.edges.bind_consumer(verdict.edge_id(), consumer);
            // The mint already listed a parked edge on its producer, so only the pending half of
            // the wire is left. A filled verdict waits on nothing.
            if let InstalledEdge::Parked(edge) = verdict {
                debug_assert!(
                    !self.would_create_cycle(
                        self.edges
                            .producer_of(edge)
                            .expect("a parked edge names the producer it waits on"),
                        consumer,
                    ),
                    "a park must wait lexically backward; a cycling edge can never be installed",
                );
                self.deps.count_pending(consumer);
            }
            self.deps.record_dep(consumer, verdict.edge_id());
            installed.push(verdict);
        }
        installed
    }

    /// Wire one edge from `producer` toward a destination region, named by its owner: holding
    /// `destination` at this call is the wiring-time proof the caller pins that region
    /// ([design/dag-scheduler.md § Edges and the boundary](../design/dag-scheduler.md#edges-and-the-boundary)),
    /// which is why the door takes an owner and performs no coverage check of its own. The standing
    /// half of the lattice — the destination stays covered for the edge's life — rides the releasing
    /// owner's teardown verb.
    ///
    /// The edge always parks, and its producer must be live: a slot reclaims at its own finalize, so
    /// a `NodeId` that names a terminal producer names nothing. Wiring off a *name the embedder
    /// already holds* takes [`install_edge_from`](Self::install_edge_from) instead, which is where
    /// the filled branch lives.
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
    ///
    /// Inheriting the destination is what makes the filled branch a *share*: the resident already
    /// sits in the region the new edge names, so duplicating the cell is the whole of the work.
    /// There is no cross-destination form — the lattice derives the new edge's coverage from
    /// `source`'s owner standing. An embedder that needs a destination of its own names one, through
    /// [`install_edge`](Self::install_edge).
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

    /// Release one edge. Rides its owner's teardown verb: an [`EdgeId`] is a name, not a lifecycle
    /// handle its holder manages. A parked edge's slab index is withheld from circulation until the
    /// walk that still lists it drops the entry (Inv-C). Panics in debug builds on a name whose
    /// index was recycled.
    pub fn release_edge(&mut self, id: EdgeId) {
        self.edges.release(id);
    }
}

/// The edge-keyed reads: an embedder holding an [`EdgeId`] reaches the delivered terminal resting on
/// it. There is no slot behind these — the producer reclaimed at its own finalize, and what the edge
/// holds is an ordinary resident of the destination region the edge names.
impl<W: Workload> Scheduler<W> {
    /// The delivered terminal's error, or `Ok(())` for a value terminal — the success/failure probe
    /// that borrows no value.
    pub fn edge_result_error(&self, id: EdgeId) -> Result<(), &W::Error> {
        self.edges.resident_error(id)
    }

    /// Open the delivered terminal at a rank-2 brand, so the value nests inside the access rather
    /// than riding the `&self` borrow up-stack. The pin is the destination region's own owner,
    /// upgraded off its back-link: the value lives in that region, so the region's own liveness is
    /// what covers the read.
    pub fn read_edge_result_with<R>(
        &self,
        id: EdgeId,
        f: impl for<'b> FnOnce(Live<'b, W>) -> R,
    ) -> Result<R, &W::Error> {
        let host = self.edges.destination_host(id);
        Ok(self.edges.resident_ref(id)?.open_with(&host, f))
    }

    /// Duplicate the delivered terminal's sealed cell — value + reach description — leaving the
    /// edge's own copy intact. The cell is `Copy`-shaped data whose pointee lives in the destination
    /// region, so a consumer that holds that region re-brands it
    /// ([`Retained::brand_with`](crate::witnessed::Retained::brand_with)) rather than carrying pins
    /// of its own.
    pub(in crate::scheduler) fn edge_resident(
        &self,
        id: EdgeId,
    ) -> Result<SealedTerminal<W>, W::Error> {
        self.edges.resident_duplicate(id)
    }

    /// [`edge_resident`](Self::edge_resident), widened for a driving crate's own white-box tests: a
    /// test can re-brand the duplicate into an owned envelope at a lifetime of its own choosing —
    /// the capability [`read_edge_result_with`](Self::read_edge_result_with) withholds, since its
    /// callback's value is scoped to the read rather than escaping it.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn edge_resident_duplicate(&self, id: EdgeId) -> Result<SealedTerminal<W>, W::Error> {
        self.edge_resident(id)
    }

    /// [`splice_forward`](Self::splice_forward) onto the producer behind `source`. A filled source
    /// never reaches here — the slot's step took the install verb's filled branch and forwarded the
    /// resident instead of emitting an alias.
    pub(in crate::scheduler) fn splice_forward_from(&mut self, slot: NodeId, source: EdgeId) {
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

/// Forwarders that let white-box tests poke slot/edge state without exposing the `store` / `deps` /
/// `queues` / `edges` fields. The `test-hooks` feature widens them for an embedder compiling as a
/// dependent crate, where `cfg(test)` is off.
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
    /// The realized dep list the install door wrote onto a slot's dep row.
    pub fn stored_deps(&self, id: NodeId) -> Vec<EdgeId> {
        self.deps.stored_deps(id)
    }
}
