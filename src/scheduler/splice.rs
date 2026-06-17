//! Bare-name forward splice — all the graph logic for eliminating a forwarding node.
//!
//! When a slot resolves to a downstream producer, its result *is* that producer's result. Rather
//! than keep a forwarding node, the slot is **spliced out**: it becomes an alias of the producer,
//! which stays the single producer of that result. This module owns every graph operation the
//! splice needs, so the alias contract lives in one place:
//!
//! - [`Scheduler::splice_forward`] performs the splice — move the consumers already parked on the
//!   slot onto the producer's notify list, and mark the slot an alias (in the node store) so reads
//!   follow through.
//! - [`Scheduler::resolve_alias`] walks an alias chain to the real producer. Reads
//!   ([`Scheduler::read_result`] etc.) and edge-installs both resolve through it, so neither the
//!   store nor the dep graph has to be alias-aware on its own.
//! - [`Scheduler::add_owned_edge`] / [`Scheduler::add_park_edge`] install a dep edge against the
//!   *resolved* producer. A consumer that wires to the slot *after* the splice therefore waits on —
//!   and is woken by — the real producer, not the dead alias. A resolved producer that has already
//!   finalized adds no edge at all: its value is read directly when the consumer runs.

use super::{NodeId, Scheduler, Workload};

impl<W: Workload> Scheduler<W> {
    /// Follow a chain of bare-name-forward aliases to the slot that actually holds the result.
    /// Aliases always point downstream to a real producer, so the walk terminates.
    pub(crate) fn resolve_alias(&self, mut id: NodeId) -> NodeId {
        let mut guard = 0;
        while let Some(to) = self.store.alias_target(id) {
            id = to;
            guard += 1;
            assert!(
                guard <= self.len(),
                "alias cycle while resolving forward at {id:?}"
            );
        }
        id
    }

    /// Splice `slot` out as an alias of `producer`: move the consumers already parked on `slot`
    /// onto `producer`'s notify list and mark `slot` an alias. `producer` is resolved first so
    /// aliases never chain. Late parkers are handled by [`Self::add_owned_edge`] /
    /// [`Self::add_park_edge`] resolving the alias when they wire in.
    pub(crate) fn splice_forward(&mut self, slot: NodeId, producer: NodeId) {
        let producer = self.resolve_alias(producer);
        self.deps.splice_notify(slot.index(), producer.index());
        self.store.alias(slot, producer);
    }

    /// Install an `Owned` read-edge from `producer` to `consumer`, following any alias on
    /// `producer`. An already-finalized resolved producer adds no edge — the consumer reads its
    /// value directly, so it never parks on a slot that will not fire.
    pub(crate) fn add_owned_edge(&mut self, producer: NodeId, consumer: NodeId) {
        let producer = self.resolve_alias(producer);
        if !self.store.is_result_ready(producer) {
            self.deps.add_owned_edge(producer, consumer);
        }
    }

    /// Park (`Notify`) sibling of [`Self::add_owned_edge`]: the consumer reads `producer` but does
    /// not own it. Same alias-resolve and already-finalized short-circuit.
    pub(crate) fn add_park_edge(&mut self, producer: NodeId, consumer: NodeId) {
        let producer = self.resolve_alias(producer);
        if !self.store.is_result_ready(producer) {
            self.deps.add_park_edge(producer, consumer);
        }
    }
}
