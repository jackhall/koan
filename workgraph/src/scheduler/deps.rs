//! The two scheduler-side dep-list currencies.
//!
//! A node's dep list is one logical vector laid out `[park_producers..., owned_subs...]`. *Park*
//! deps are notify-only edges — the consumer reads the producer's value but does not own it, so a
//! park producer is never cascade-freed with the consumer. *Owned* deps are sub-work the consumer
//! spawned; they cascade-free when it succeeds. Dep results are delivered to a finish in that same
//! `[park..., owned...]` order.
//!
//! This module is the *only* owner of that layout arithmetic:
//!
//! - [`Deps`] is the write side — the builder production code assembles a dep list with. It keeps
//!   parks and owned entries in two vecs so the split is structural, never a `park_count` a caller
//!   must thread. Its parks are [`EdgeId`]s — the *edges the embedder holds*, which the install door
//!   resolves to producers. It is generic in the owned-entry type `R` (`DepRequest` before the
//!   harness realizes each request to a producer id).
//! - [`ResolvedDeps`] is the realized list [`NodeWork`](super::nodes::NodeWork) stores: parks and
//!   owned entries alike are producer ids. Its own struct rather than `Deps<NodeId>`, because the
//!   two sides' park currencies differ.
//! - [`DepResults`] is the read side — a `[park..., owned...]` result slice plus its park-prefix
//!   length, addressed through [`park`](DepResults::park) / [`owned`](DepResults::owned) accessors so
//!   a finish never re-derives the prefix arithmetic.
//!
//! Everything here is plain or type-parameter-generic (`NodeId`, `usize`, `R`, `T`) — it names no
//! Koan value, error, or AST type.

use super::{EdgeId, NodeId};

/// The dep-list builder: the one way production code assembles a node's dep list. Parks and owned
/// entries live in separate vecs, so `[park..., owned...]` is structural — there is no `park_count`
/// for a caller to thread or get wrong. Generic in the owned-entry type `R`: a `DepRequest` before
/// the apply harness realizes each owned request to its producer id, `NodeId` after ([`ResolvedDeps`]).
///
/// A park is named by the **source edge** the embedder holds, not by a producer: the install door
/// ([`Scheduler::install_deps`](super::Scheduler::install_deps)) mints the consumer's own edge off
/// each source and resolves it, so the producer currency never leaves the scheduler.
pub struct Deps<R> {
    /// Park sources, deduped, in first-occurrence order.
    parks: Vec<EdgeId>,
    /// Owned entries, in insertion order.
    owned: Vec<R>,
}

impl<R> Deps<R> {
    pub fn new() -> Self {
        Deps {
            parks: Vec::new(),
            owned: Vec::new(),
        }
    }

    /// Add a dedup'ing park on the source edge `id`. Returns `id`'s park index — the existing
    /// position when `id` is already parked, else the newly-pushed one. Positional reads (a literal
    /// cell keyed on its park slot) stay correct when one expression names the same edge twice
    /// because the index is stable. Dedup is by [`EdgeId`] equality: two *distinct* edges naming one
    /// producer stay two parks, so every index the caller was handed keeps addressing its own slot.
    pub fn park_on(&mut self, id: EdgeId) -> usize {
        if let Some(pos) = self.parks.iter().position(|p| *p == id) {
            return pos;
        }
        let pos = self.parks.len();
        self.parks.push(id);
        pos
    }

    /// Add an owned dep. Returns its owned index — the position *within* the owned vec, NOT within
    /// the concatenated `[park..., owned...]` delivery order (the read side adds the park prefix).
    pub fn own(&mut self, entry: R) -> usize {
        let pos = self.owned.len();
        self.owned.push(entry);
        pos
    }

    /// Build a park-only dep list from a source-edge sequence (re-dedup'ing harmlessly). The
    /// park-and-replay shapes that own no sub-work (`park_resume`) start here.
    pub fn from_parks(ids: impl IntoIterator<Item = EdgeId>) -> Self {
        let mut deps = Deps::new();
        for id in ids {
            deps.park_on(id);
        }
        deps
    }

    /// Build a dep list whose every entry is owned — the all-owned shape a dispatch decide parks on
    /// when it has no notify-only producers to wait on.
    pub fn from_owned(entries: impl IntoIterator<Item = R>) -> Self {
        let mut deps = Deps::new();
        for entry in entries {
            deps.own(entry);
        }
        deps
    }

    pub fn parks(&self) -> &[EdgeId] {
        &self.parks
    }

    pub fn owned(&self) -> &[R] {
        &self.owned
    }

    pub fn is_empty(&self) -> bool {
        self.parks.is_empty() && self.owned.is_empty()
    }

    /// Decompose into `(park sources, owned)` for the realization loop, which turns each owned
    /// `DepRequest` into a producer id and hands the park sources to the install door.
    pub fn into_parts(self) -> (Vec<EdgeId>, Vec<R>) {
        (self.parks, self.owned)
    }
}

/// A realized dep list — parks and owned deps are all producer ids. This is what
/// [`NodeWork`](super::nodes::NodeWork) stores, and what the run loop pulls each dep's terminal
/// through at step start.
///
/// Deliberately *not* `Deps<NodeId>`: the two currencies have parted. [`Deps`] is the embedder's
/// write side, whose parks are edges the embedder holds; a realized list's parks are the producer
/// ids the door resolved them to, scheduler-internal from that point on — which is why the only
/// way to put one here is [`push_park`](Self::push_park), the door's own crate-private append.
pub struct ResolvedDeps {
    /// Park producers, deduped, in first-occurrence order.
    parks: Vec<NodeId>,
    /// Owned producers, in insertion order.
    owned: Vec<NodeId>,
}

impl ResolvedDeps {
    pub fn new() -> Self {
        ResolvedDeps {
            parks: Vec::new(),
            owned: Vec::new(),
        }
    }

    /// Append a park producer **without deduping** — the install door's push, one entry per park the
    /// embedder handed it. Alignment is the point: the embedder's own park indices address its edge
    /// list, and two distinct edges can resolve to one producer (a splice), so collapsing here would
    /// slide every later [`DepResults::park`] read by one.
    pub(crate) fn push_park(&mut self, id: NodeId) {
        self.parks.push(id);
    }

    /// Add an owned dep, returning its index *within* the owned vec (not within the concatenated
    /// `[park..., owned...]` delivery order).
    pub fn own(&mut self, id: NodeId) -> usize {
        let pos = self.owned.len();
        self.owned.push(id);
        pos
    }

    pub fn parks(&self) -> &[NodeId] {
        &self.parks
    }

    pub fn owned(&self) -> &[NodeId] {
        &self.owned
    }

    /// The park-prefix length — the split point of the `[park..., owned...]` delivery order.
    fn park_count(&self) -> usize {
        self.parks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.parks.is_empty() && self.owned.is_empty()
    }

    /// The producer ids in delivery order: parks first, then owned. The run loop reads each dep's
    /// terminal in this order so a finish's [`DepResults`] lines up.
    pub fn all_ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.parks.iter().chain(self.owned.iter()).copied()
    }

    /// Wrap a delivered `[park..., owned...]` result slice as a [`DepResults`] view carrying this
    /// list's park prefix — the run loop's single crossing from the write side to the read side, so
    /// the prefix length never leaves the scheduler.
    pub fn results<'a, T>(&self, items: &'a [T]) -> DepResults<'a, T> {
        DepResults::new(items, self.park_count())
    }
}

impl Default for ResolvedDeps {
    fn default() -> Self {
        Self::new()
    }
}

impl<R> Default for Deps<R> {
    fn default() -> Self {
        Self::new()
    }
}

/// The read side of a resolved dep list: the delivered `[park..., owned...]` result slice plus its
/// park-prefix length. The only owner of the prefix arithmetic on the read path — a finish addresses
/// its deps through [`park`](Self::park) / [`owned`](Self::owned) and never re-derives it. `Copy`, so
/// it threads by value through finish signatures.
///
/// `pub` (like [`NodeId`](super::NodeId)) rather than `pub(crate)`: it rides the `pub`
/// `AwaitContinue` builtin-finish signature, so a narrower visibility would leak.
#[derive(Clone, Copy)]
pub struct DepResults<'a, T> {
    /// Delivered as `[parks..., owned...]`.
    items: &'a [T],
    park_count: usize,
}

impl<'a, T> DepResults<'a, T> {
    /// Crate-private: [`ResolvedDeps::results`] is the single crossing from the write side to the
    /// read side, so the prefix length is never paired with a slice by hand outside the scheduler.
    pub(crate) fn new(items: &'a [T], park_count: usize) -> Self {
        DepResults { items, park_count }
    }

    /// The `i`-th park result (`items[i]`).
    pub fn park(&self, i: usize) -> &'a T {
        &self.items[i]
    }

    /// The `j`-th owned result (`items[park_count + j]`).
    pub fn owned(&self, j: usize) -> &'a T {
        &self.items[self.park_count + j]
    }

    /// The owned suffix (`items[park_count..]`) — a re-walk that consumes only its owned sub-results
    /// in order feeds off this.
    pub fn owned_slice(&self) -> &'a [T] {
        &self.items[self.park_count..]
    }

    /// The whole `[park..., owned...]` slice, for a finish that consumes every result in order.
    pub fn all(&self) -> &'a [T] {
        self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Re-wrap a different slice under this view's park prefix — the mediating continuation
    /// combinators map the resolved terminals to values/carriers, then re-wrap so the finish's
    /// `DepResults` keeps the same `[park..., owned...]` split without ever naming the prefix length.
    pub fn rewrap<'b, U>(&self, items: &'b [U]) -> DepResults<'b, U> {
        DepResults::new(items, self.park_count)
    }
}
