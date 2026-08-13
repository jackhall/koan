//! The **edge slab** — first-class edges, addressed by [`EdgeId`], mirroring the node store's
//! shape: a state vector plus a free list of recyclable indices.
//!
//! An [`EdgeId`] is a *name*, not the edge: holding one grants the scheduler's wiring verbs and
//! confers no ownership and no lifecycle duty. Edges are released by their owner — a
//! teardown-bearing structure (the consumer node, or the frame whose teardown verb carries the
//! release) — so a live edge implies a live owner by construction. Debug-only generation stamps
//! make a stale name loud rather than silently renaming a recycled index.
//!
//! Each edge names its destination region by raw pointer plus, in debug builds, a weak shadow of
//! the region's owner. Validity is the containment lattice — destination outlives owner outlives
//! edge — rather than a refcount, so nothing here owns the destination.
//!
//! See [design/dag-scheduler.md § Edges and the boundary](../../design/dag-scheduler.md#edges-and-the-boundary)
//! and [§ Late wiring and install](../../design/dag-scheduler.md#late-wiring-and-install).

use std::rc::Rc;
#[cfg(debug_assertions)]
use std::rc::Weak;

use crate::witnessed::RegionOwner;

use super::NodeId;

/// Stable name for an edge in the slab. A name, not the edge: holding one grants the scheduler's
/// wiring verbs and confers no ownership.
///
/// The index is private and unreadable outside the crate — like [`NodeId`], the whole surface
/// speaks this currency and never a raw slab index.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct EdgeId {
    index: usize,
    /// Debug-only stamp matched against the slab's per-index generation, so a name outliving its
    /// edge is loud rather than silently renaming a recycled index.
    #[cfg(debug_assertions)]
    generation: u32,
}

/// The destination half of an edge: the region delivery lands in, named raw — validity is the
/// containment lattice (destination outlives owner outlives edge), not a refcount.
///
/// `#[allow(dead_code)]`: the install door records the destination ahead of any deref, and the one
/// deref site is the delivery walk's
/// ([delivery-at-finalize](../../roadmap/delivery-at-finalize.md)). Until then only the white-box
/// probes below read either field, so a plain `--lib` build (no `cfg(test)`) sees none.
#[allow(dead_code)]
struct Destination<F: RegionOwner> {
    region: *const F::Region,
    /// Debug-only liveness shadow of the destination's owner, asserted live at that same deref.
    /// `Weak`, deliberately: a strong `Rc` would pin the destination and mask a lattice violation
    /// instead of exposing it.
    #[cfg(debug_assertions)]
    shadow: Weak<F>,
}

impl<F: RegionOwner> Destination<F> {
    /// Name a destination by its region's owner. Holding that `Rc` at the call is the wiring-time
    /// proof the caller pins the region, which is why the install door takes one.
    fn of_owner(owner: &Rc<F>) -> Self {
        Destination {
            region: RegionOwner::region(&**owner) as *const F::Region,
            #[cfg(debug_assertions)]
            shadow: Rc::downgrade(owner),
        }
    }
}

/// `#[allow(dead_code)]` for the same reason [`Destination`] carries one: the slab's own verbs read
/// the discriminant, and the payload's readers — the delivery deref and the splice's producer
/// rewrite — arrive with the items that add them.
#[allow(dead_code)]
enum EdgeState<F: RegionOwner> {
    /// Wired before its producer finalized. The producer is the scheduler's to rewrite while the
    /// edge is parked (the alias splice); post-fill an edge has no producer to name.
    Parked {
        producer: NodeId,
        destination: Destination<F>,
    },
    /// The producer finalized. The resident is read through the slot machinery, so a filled edge
    /// records only that it is past parking.
    Filled { destination: Destination<F> },
    /// Reclaimed; the index sits on the free list.
    Free,
}

/// What [`Scheduler::install_edge`](super::Scheduler::install_edge) wired: the edge's name, tagged
/// by whether the producer had already finalized at the call.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum InstalledEdge {
    /// The producer was already terminal — the consumer reads its value rather than parking.
    Filled(EdgeId),
    /// The producer is pre-terminal; the edge waits on it.
    Parked(EdgeId),
}

impl InstalledEdge {
    /// The edge's name, whichever branch install took.
    pub fn edge_id(self) -> EdgeId {
        match self {
            InstalledEdge::Filled(id) | InstalledEdge::Parked(id) => id,
        }
    }
}

pub(in crate::scheduler) struct EdgeSlab<F: RegionOwner> {
    entries: Vec<EdgeState<F>>,
    /// Reclaimed indices. [`install`](Self::install) pulls from here before extending `entries`,
    /// giving constant slab memory across a run's churn.
    free_list: Vec<usize>,
    /// Per-index generation, bumped at release. Parallel to `entries` so a `Free` entry keeps its
    /// stamp and every outstanding copy of its name goes stale at once.
    #[cfg(debug_assertions)]
    generations: Vec<u32>,
}

impl<F: RegionOwner> EdgeSlab<F> {
    pub(super) fn new() -> Self {
        Self {
            entries: Vec::new(),
            free_list: Vec::new(),
            #[cfg(debug_assertions)]
            generations: Vec::new(),
        }
    }

    /// Wire one edge toward `destination`, named by its region's owner. `producer_if_pending` is
    /// the producer to park on, or `None` when it has already finalized — the readiness probe is
    /// the caller's, so this file owns only the slab's own arithmetic.
    pub(in crate::scheduler) fn install(
        &mut self,
        producer_if_pending: Option<NodeId>,
        destination: &Rc<F>,
    ) -> InstalledEdge {
        let destination = Destination::of_owner(destination);
        match producer_if_pending {
            Some(producer) => InstalledEdge::Parked(self.alloc(EdgeState::Parked {
                producer,
                destination,
            })),
            None => InstalledEdge::Filled(self.alloc(EdgeState::Filled { destination })),
        }
    }

    /// Release one edge: the index returns to the free list and, in debug builds, its generation
    /// bumps so every outstanding copy of the name is stale. Panics on a name already released.
    pub(in crate::scheduler) fn release(&mut self, id: EdgeId) {
        let index = self.slot_index(id);
        self.entries[index] = EdgeState::Free;
        self.free_list.push(index);
        #[cfg(debug_assertions)]
        {
            self.generations[index] += 1;
        }
    }

    /// The only path that picks an index, and the only mint of an [`EdgeId`] — recycle from
    /// `free_list` or extend, mirroring `NodeStore::alloc_slot`.
    fn alloc(&mut self, state: EdgeState<F>) -> EdgeId {
        let index = match self.free_list.pop() {
            Some(index) => {
                self.entries[index] = state;
                index
            }
            None => {
                self.entries.push(state);
                #[cfg(debug_assertions)]
                self.generations.push(0);
                self.entries.len() - 1
            }
        };
        EdgeId {
            index,
            #[cfg(debug_assertions)]
            generation: self.generations[index],
        }
    }

    /// Resolve a name to its index, checking the debug generation stamp — the single id→index step
    /// every verb here goes through, mirroring `DepGraph::row` / `row_mut`.
    fn slot_index(&self, id: EdgeId) -> usize {
        #[cfg(debug_assertions)]
        debug_assert_eq!(
            self.generations[id.index], id.generation,
            "stale EdgeId: the edge at this index was released",
        );
        id.index
    }

    /// Re-point a parked edge at another producer — the alias splice's half of the wiring. Only a
    /// pre-fill edge has a producer to rewrite.
    ///
    /// Exercised by the white-box tests below; the splice path is the production caller once parked
    /// edges ride the slab ([edge-wiring-migration](../../../roadmap/refactor/edge-wiring-migration.md)).
    #[cfg(any(test, feature = "test-hooks"))]
    pub(in crate::scheduler) fn rewrite_producer(&mut self, id: EdgeId, producer: NodeId) {
        let index = self.slot_index(id);
        match &mut self.entries[index] {
            EdgeState::Parked { producer: slot, .. } => *slot = producer,
            _ => panic!("only a pre-fill edge has a producer to rewrite"),
        }
    }

    // --- Test-only probes over slab state the wiring verbs otherwise keep to themselves. ---

    /// The producer a parked edge waits on, or `None` for a filled one.
    #[cfg(any(test, feature = "test-hooks"))]
    pub(in crate::scheduler) fn producer_of(&self, id: EdgeId) -> Option<NodeId> {
        match &self.entries[self.slot_index(id)] {
            EdgeState::Parked { producer, .. } => Some(*producer),
            _ => None,
        }
    }

    /// The destination region this edge was wired toward — a bare pointer for identity comparison
    /// against the owner install was handed; never dereferenced.
    #[cfg(any(test, feature = "test-hooks"))]
    pub(in crate::scheduler) fn destination_region(&self, id: EdgeId) -> *const F::Region {
        match &self.entries[self.slot_index(id)] {
            EdgeState::Parked { destination, .. } | EdgeState::Filled { destination } => {
                destination.region
            }
            EdgeState::Free => panic!("a released edge names no destination"),
        }
    }

    /// The destination owner behind this edge's debug shadow, upgraded — `None` once it has died.
    #[cfg(all(debug_assertions, any(test, feature = "test-hooks")))]
    pub(in crate::scheduler) fn destination_owner(&self, id: EdgeId) -> Option<Rc<F>> {
        match &self.entries[self.slot_index(id)] {
            EdgeState::Parked { destination, .. } | EdgeState::Filled { destination } => {
                destination.shadow.upgrade()
            }
            EdgeState::Free => panic!("a released edge names no destination"),
        }
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(in crate::scheduler) fn free_list_len(&self) -> usize {
        self.free_list.len()
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(in crate::scheduler) fn len(&self) -> usize {
        self.entries.len()
    }
}
