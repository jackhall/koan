//! The **edge slab** — first-class edges, addressed by [`EdgeId`], mirroring the node store's
//! shape: a state vector plus a free list of recyclable indices.
//!
//! An [`EdgeId`] is a *name*, not the edge: holding one confers no ownership and no lifecycle
//! duty. Edges are released by their owner — a teardown-bearing structure (the consumer node, or
//! the frame whose teardown verb carries the release) — so a live edge implies a live owner by
//! construction.
//!
//! An edge is also **where a delivered terminal lives**. The producer's finalize walk adopts its
//! terminal into each distinct destination region and rests the product on the edges waiting there,
//! so a filled edge holds the resident itself rather than a route back to a slot that no longer
//! exists. That is what lets every slot reclaim at finalize.
//!
//! Each edge names its destination region by raw pointer plus, in debug builds, a weak shadow of
//! the region's owner. Validity is the containment lattice — destination outlives owner outlives
//! edge — rather than a refcount, so nothing here owns the destination. The one deref
//! ([`Destination::region_ref`]) is the scheduler's only `unsafe`.
//!
//! Recycling is deferred for a **listed** edge (Inv-C): a parked edge sits on its producer's notify
//! list, so releasing it sets `Free` but leaves the index out of the free list until the walk (or
//! the splice) drops the entry naming it. Generation stamps are debug-only, so a release-build walk
//! that met a recycled index would deliver into a stranger's edge.
//!
//! See [design/dag-scheduler.md § Edges and the boundary](../../design/dag-scheduler.md#edges-and-the-boundary),
//! [§ Late wiring and install](../../design/dag-scheduler.md#late-wiring-and-install) and
//! [§ Delivery at finalize](../../design/dag-scheduler.md#delivery-at-finalize).

use std::rc::Rc;
#[cfg(debug_assertions)]
use std::rc::Weak;

use crate::witnessed::{Region, RegionHandle, RegionOwner};

use super::workload::{OwnerOf, SealedTerminal};
use super::{NodeId, Workload};

/// Stable name for an edge in the slab: a name, not the edge, conferring no ownership. The index is
/// private and unreadable outside the crate — like [`NodeId`], the whole surface speaks this
/// currency and never a raw slab index.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct EdgeId {
    index: usize,
    /// Matched against the slab's per-index generation, so a name outliving its edge is loud rather
    /// than silently renaming a recycled index. Bumped when the index is **recycled**, not when the
    /// edge is released: a released-but-listed index is still addressed by the walk entry that will
    /// drop it, and only re-minting can rename it.
    #[cfg(debug_assertions)]
    generation: u32,
}

impl EdgeId {
    /// Fabricate a name for a white-box test that drives no slab. Gated so it cannot reach
    /// production, and widened past `cfg(test)` for an embedder's own tests, which compile against
    /// this crate as a dependency. The generation is the fresh-index stamp, so a fabricated name
    /// never collides with a released one.
    #[cfg(any(test, feature = "test-hooks"))]
    pub const fn for_test(index: usize) -> Self {
        EdgeId {
            index,
            #[cfg(debug_assertions)]
            generation: 0,
        }
    }
}

/// The destination half of an edge: the region delivery lands in, named raw — validity is the
/// containment lattice (destination outlives owner outlives edge), not a refcount.
struct Destination<W: Workload> {
    region: *const Region<W::Profile>,
    /// Debug-only liveness shadow of the destination's owner, asserted live at the deref.
    /// `Weak`, deliberately: a strong `Rc` would pin the destination and mask a lattice violation
    /// instead of exposing it.
    #[cfg(debug_assertions)]
    shadow: Weak<OwnerOf<W>>,
}

impl<W: Workload> Destination<W> {
    /// Name a destination by its region's owner. Holding that `Rc` at the call is the wiring-time
    /// proof the caller pins the region, which is why the install door takes one.
    fn of_owner(owner: &Rc<OwnerOf<W>>) -> Self {
        let region: &Region<W::Profile> = RegionOwner::region(&**owner);
        Destination {
            region,
            #[cfg(debug_assertions)]
            shadow: Rc::downgrade(owner),
        }
    }

    /// Copy a live edge's destination onto a second edge — the wire-from-a-source path, which names
    /// no owner of its own. Sound on the containment lattice: the source edge stands, so its owner
    /// stands, so the region it names is covered; the new edge's own owner sits below that owner on
    /// the same lattice. Not a `Clone` impl — copying a raw destination is a wiring act, not a
    /// general duplication.
    fn inherit(&self) -> Self {
        Destination {
            region: self.region,
            #[cfg(debug_assertions)]
            shadow: self.shadow.clone(),
        }
    }

    /// **The scheduler's one `unsafe`**: borrow the destination region a live edge names.
    ///
    /// The witness is the containment lattice — *destination outlives owner outlives edge*
    /// ([design/dag-scheduler.md § Edges and the boundary](../../design/dag-scheduler.md#edges-and-the-boundary)).
    /// Wiring establishes the upper half: the install door is handed an `Rc` on the destination's
    /// owner (or inherits a standing edge's destination, whose own owner is holding it), so the
    /// region is covered at the moment the pointer is recorded, and the destination sits at or above
    /// the new edge's owner on the lattice. The teardown side establishes the lower half: an edge is
    /// released by the owner that holds it, from that owner's own teardown, so a live edge implies a
    /// live owner implies a live destination. The pointee is a `Region` inside its owner's storage,
    /// which is `StableDeref` behind the owner's `Rc`, so the address does not move under it.
    ///
    /// The debug shadow asserts exactly that argument at the deref: a destination owner that died
    /// under a live edge is a lattice violation, and this is where it is loud.
    fn region_ref(&self) -> &Region<W::Profile> {
        #[cfg(debug_assertions)]
        debug_assert!(
            self.shadow.strong_count() > 0,
            "an edge's destination owner died under a live edge (containment lattice violated)",
        );
        // SAFETY: see the doc comment above — the containment lattice, asserted in debug by the
        // weak shadow immediately above.
        unsafe { &*self.region }
    }

    /// The destination's owner as an owned pin, upgraded off the debug shadow's strong sibling — the
    /// region's own back-link. Infallible on the lattice: a live edge implies a live owner.
    fn host(&self) -> Rc<OwnerOf<W>> {
        self.region_ref()
            .host()
            .upgrade()
            .expect("a live edge's destination owner is live (containment lattice)")
    }
}

enum EdgeState<W: Workload> {
    /// Wired before its producer finalized, and **listed on that producer's notify list**. The
    /// producer is the scheduler's to rewrite while the edge is parked (the alias splice).
    Parked {
        producer: NodeId,
        /// The consumer whose `pending` this edge counts against; `None` for a root or placeholder
        /// edge, which receives delivery but wakes nobody.
        consumer: Option<NodeId>,
        destination: Destination<W>,
    },
    /// **Delivered**: the producer's walk adopted its terminal into this edge's destination. The
    /// consumer is kept for symmetry with the parked state; its `pending` decrement already fired
    /// at the fill.
    Filled {
        resident: Result<SealedTerminal<W>, W::Error>,
        consumer: Option<NodeId>,
        destination: Destination<W>,
    },
    /// Released. The index sits on the free list unless a notify entry still names it (Inv-C).
    Free,
}

/// What an install door wired: the edge's name, tagged by whether its producer had already finalized
/// at the call — in which case the terminal is already resident on the new edge.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum InstalledEdge {
    /// The producer was already terminal — the edge carries its delivered resident.
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

pub(in crate::scheduler) struct EdgeSlab<W: Workload> {
    entries: Vec<EdgeState<W>>,
    /// Recyclable indices, preferred over extending `entries`, so slab memory stays roughly
    /// constant across a run's churn.
    free_list: Vec<usize>,
    /// Per-index generation, bumped at recycle. Parallel to `entries` so a recycled index's
    /// outstanding names all go stale at once.
    #[cfg(debug_assertions)]
    generations: Vec<u32>,
}

impl<W: Workload> EdgeSlab<W> {
    pub(super) fn new() -> Self {
        Self {
            entries: Vec::new(),
            free_list: Vec::new(),
            #[cfg(debug_assertions)]
            generations: Vec::new(),
        }
    }

    /// Wire one parked edge toward the region `destination` owns. Every parked edge is registered
    /// on its producer's notify list, which is what makes Inv-C's deferred recycle exact.
    pub(in crate::scheduler) fn install_parked(
        &mut self,
        producer: NodeId,
        consumer: Option<NodeId>,
        destination: &Rc<OwnerOf<W>>,
    ) -> EdgeId {
        self.alloc(EdgeState::Parked {
            producer,
            consumer,
            destination: Destination::of_owner(destination),
        })
    }

    /// [`install_parked`](Self::install_parked) toward the destination `source` already names —
    /// inherited rather than supplied (see [`Destination::inherit`] for why that is sound).
    pub(in crate::scheduler) fn install_parked_inheriting(
        &mut self,
        source: EdgeId,
        producer: NodeId,
        consumer: Option<NodeId>,
    ) -> EdgeId {
        let destination = self.destination(source).inherit();
        self.alloc(EdgeState::Parked {
            producer,
            consumer,
            destination,
        })
    }

    /// Mint an already-**delivered** edge toward `source`'s destination: the late-wire filled
    /// branch, whose resident is shared with the source when the destination matches and adopted
    /// afresh when it does not.
    pub(in crate::scheduler) fn install_filled_inheriting(
        &mut self,
        source: EdgeId,
        resident: Result<SealedTerminal<W>, W::Error>,
        consumer: Option<NodeId>,
    ) -> EdgeId {
        let destination = self.destination(source).inherit();
        self.alloc(EdgeState::Filled {
            resident,
            consumer,
            destination,
        })
    }

    /// **Deliver**: rest `resident` on a parked edge, preserving its consumer and destination. Runs
    /// once per live edge on the producer's notify list.
    pub(in crate::scheduler) fn fill(
        &mut self,
        id: EdgeId,
        resident: Result<SealedTerminal<W>, W::Error>,
    ) {
        let index = self.slot_index(id);
        let (consumer, destination) = match &mut self.entries[index] {
            EdgeState::Parked {
                consumer,
                destination,
                ..
            } => (*consumer, destination.inherit()),
            _ => panic!("only a parked edge receives delivery"),
        };
        self.entries[index] = EdgeState::Filled {
            resident,
            consumer,
            destination,
        };
    }

    /// The producer a **parked** edge waits on, `None` once it is delivered — the parked/filled
    /// split itself.
    pub(in crate::scheduler) fn producer_of(&self, id: EdgeId) -> Option<NodeId> {
        match &self.entries[self.slot_index(id)] {
            EdgeState::Parked { producer, .. } => Some(*producer),
            EdgeState::Filled { .. } => None,
            EdgeState::Free => panic!("a released edge names no producer"),
        }
    }

    /// The consumer whose `pending` this edge counts against, `None` for a root or placeholder edge.
    pub(in crate::scheduler) fn consumer_of(&self, id: EdgeId) -> Option<NodeId> {
        match &self.entries[self.slot_index(id)] {
            EdgeState::Parked { consumer, .. } | EdgeState::Filled { consumer, .. } => *consumer,
            EdgeState::Free => panic!("a released edge names no consumer"),
        }
    }

    /// Bind a freshly minted edge to the consumer that owns it, once that consumer's slot exists —
    /// an edge can be minted before its consumer is allocated.
    pub(in crate::scheduler) fn bind_consumer(&mut self, id: EdgeId, consumer: NodeId) {
        let index = self.slot_index(id);
        match &mut self.entries[index] {
            EdgeState::Parked { consumer: slot, .. } | EdgeState::Filled { consumer: slot, .. } => {
                *slot = Some(consumer)
            }
            EdgeState::Free => panic!("a released edge takes no consumer"),
        }
    }

    /// Duplicate a delivered edge's resident — the sealed cell bit-copied and its reference-only
    /// witness cloned, or the error cloned.
    pub(in crate::scheduler) fn resident_duplicate(
        &self,
        id: EdgeId,
    ) -> Result<SealedTerminal<W>, W::Error> {
        match &self.entries[self.slot_index(id)] {
            EdgeState::Filled { resident, .. } => match resident {
                Ok(cell) => Ok(cell.duplicate()),
                Err(error) => Err(error.clone()),
            },
            _ => panic!("only a delivered edge has a resident to read"),
        }
    }

    /// The delivered resident's error, or `Ok(())` for a value terminal — the success/failure probe
    /// that duplicates nothing.
    pub(in crate::scheduler) fn resident_error(&self, id: EdgeId) -> Result<(), &W::Error> {
        match &self.entries[self.slot_index(id)] {
            EdgeState::Filled {
                resident: Ok(_), ..
            } => Ok(()),
            EdgeState::Filled {
                resident: Err(error),
                ..
            } => Err(error),
            _ => panic!("only a delivered edge has a resident to read"),
        }
    }

    /// The delivered resident, borrowed — the read that duplicates nothing.
    pub(in crate::scheduler) fn resident_ref(
        &self,
        id: EdgeId,
    ) -> Result<&SealedTerminal<W>, &W::Error> {
        match &self.entries[self.slot_index(id)] {
            EdgeState::Filled { resident, .. } => resident.as_ref(),
            _ => panic!("only a delivered edge has a resident to read"),
        }
    }

    /// **The adopt capability** on the edge's destination region, minted off the one deref
    /// ([`Destination::region_ref`]).
    pub(in crate::scheduler) fn destination_handle(
        &self,
        id: EdgeId,
    ) -> RegionHandle<'_, W::Profile> {
        RegionHandle::new(self.destination(id).region_ref())
    }

    /// The destination's owner as an owned pin — the region's own back-link, upgraded.
    pub(in crate::scheduler) fn destination_host(&self, id: EdgeId) -> Rc<OwnerOf<W>> {
        self.destination(id).host()
    }

    /// The destination region this edge was wired toward, as a bare pointer for identity
    /// comparison. Never dereferenced.
    pub(in crate::scheduler) fn destination_region(&self, id: EdgeId) -> *const Region<W::Profile> {
        self.destination(id).region
    }

    /// The state match behind this module's destination readers.
    fn destination(&self, id: EdgeId) -> &Destination<W> {
        match &self.entries[self.slot_index(id)] {
            EdgeState::Parked { destination, .. } | EdgeState::Filled { destination, .. } => {
                destination
            }
            EdgeState::Free => panic!("a released edge names no destination"),
        }
    }

    /// Whether this index has been released — a consumer that dies before its producer fires
    /// releases its edges while notify entries still name them.
    pub(in crate::scheduler) fn is_free(&self, id: EdgeId) -> bool {
        matches!(self.entries[self.slot_index(id)], EdgeState::Free)
    }

    /// Release one edge, riding its owner's teardown. A **parked** edge is listed on its producer's
    /// notify list, so its index is withheld from the free list until that entry is dropped
    /// ([`recycle_released`](Self::recycle_released)) — Inv-C, and correctness rather than debug
    /// hygiene: generation stamps are debug-only, so a release-build walk meeting a recycled index
    /// would deliver into a stranger's edge. A delivered edge is on no list and recycles at once.
    pub(in crate::scheduler) fn release(&mut self, id: EdgeId) {
        let index = self.slot_index(id);
        let listed = matches!(self.entries[index], EdgeState::Parked { .. });
        self.entries[index] = EdgeState::Free;
        if !listed {
            self.recycle(index);
        }
    }

    /// Recycle the index behind a released listed edge, once the notify entry naming it is dropped
    /// — the second and last half of Inv-C. A no-op on an entry that is not `Free`.
    pub(in crate::scheduler) fn recycle_released(&mut self, id: EdgeId) {
        let index = self.slot_index(id);
        if matches!(self.entries[index], EdgeState::Free) {
            self.recycle(index);
        }
    }

    /// Re-point a parked edge at another producer — the alias splice's half of the wiring. Only a
    /// pre-delivery edge has a producer to rewrite.
    pub(in crate::scheduler) fn rewrite_producer(&mut self, id: EdgeId, producer: NodeId) {
        let index = self.slot_index(id);
        match &mut self.entries[index] {
            EdgeState::Parked { producer: slot, .. } => *slot = producer,
            _ => panic!("only a pre-delivery edge has a producer to rewrite"),
        }
    }

    /// Pick an index for `state`, recycling from `free_list` ahead of extending, mirroring
    /// `NodeStore::alloc_slot`.
    fn alloc(&mut self, state: EdgeState<W>) -> EdgeId {
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

    /// Return an index to circulation and stale every outstanding name for it: the generation bump
    /// rides the free-list push, so a recycled index cannot be re-minted under an old name.
    fn recycle(&mut self, index: usize) {
        self.free_list.push(index);
        #[cfg(debug_assertions)]
        {
            self.generations[index] += 1;
        }
    }

    /// Resolve a name to its index, checking the debug generation stamp — the single id→index step
    /// every verb here goes through, mirroring `DepGraph::row` / `row_mut`.
    fn slot_index(&self, id: EdgeId) -> usize {
        #[cfg(debug_assertions)]
        debug_assert_eq!(
            self.generations[id.index], id.generation,
            "stale EdgeId: the index this name addresses was recycled",
        );
        id.index
    }

    // --- Test-only probes over slab state the wiring verbs otherwise keep to themselves. ---

    /// The destination owner behind this edge's debug shadow, upgraded — `None` once it has died.
    #[cfg(all(debug_assertions, any(test, feature = "test-hooks")))]
    pub(in crate::scheduler) fn destination_owner(&self, id: EdgeId) -> Option<Rc<OwnerOf<W>>> {
        self.destination(id).shadow.upgrade()
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
