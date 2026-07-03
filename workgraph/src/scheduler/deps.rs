//! The two scheduler-side dep-list currencies and the producer-disposition classifier.
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
//!   must thread; [`ResolvedDeps`] is the realized form ([`NodeWork`](super::nodes::NodeWork) stores
//!   it) and [`Deps<R>`] the pre-realization form (`R = DepRequest` before the harness turns each
//!   owned request into its producer id).
//! - [`DepResults`] is the read side — a `[park..., owned...]` result slice plus its park-prefix
//!   length, addressed through [`park`](DepResults::park) / [`owned`](DepResults::owned) accessors so
//!   a finish never re-derives the prefix arithmetic.
//! - [`ProducerDisposition`] classifies "can I depend on this producer?" — the park-ladder check
//!   order every consumer site shares, leaving each its own ready-Ok policy.
//!
//! Everything here is generic over the [`Workload`](super::Workload)'s error (`E`) or plain
//! (`NodeId`, `usize`, a type parameter) — it names no Koan value, error, or AST type.

use super::NodeId;

/// Classification of "can I depend on this producer?" — the shared park-ladder check order. The
/// caller keeps its own per-site policy for every arm (a ready-Ok producer means different things in
/// different lanes); this owns only the order in which the checks run.
pub enum ProducerDisposition<'a, E> {
    /// Ready, and its terminal is an error — the caller propagates a clone.
    Errored(&'a E),
    /// Ready, and its terminal is a value (`Ok`).
    Ready,
    /// Still finalizing, and parking on it would close a wake cycle.
    Cycle,
    /// Still finalizing — park on it.
    Park,
}

/// The dep-list builder: the one way production code assembles a node's dep list. Parks and owned
/// entries live in separate vecs, so `[park..., owned...]` is structural — there is no `park_count`
/// for a caller to thread or get wrong. Generic in the owned-entry type `R`: a `DepRequest` before
/// the apply harness realizes each owned request to its producer id, `NodeId` after ([`ResolvedDeps`]).
pub struct Deps<R> {
    /// Park producers, deduped, in first-occurrence order.
    parks: Vec<NodeId>,
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

    /// Add a dedup'ing park edge on `id`. Returns `id`'s park index — the existing position when `id`
    /// is already parked, else the newly-pushed one. Positional reads (a literal cell keyed on its
    /// park slot) stay correct when two consumers share one producer because the index is stable.
    pub fn park_on(&mut self, id: NodeId) -> usize {
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

    /// Build a park-only dep list from an id sequence (re-dedup'ing harmlessly). The park-and-replay
    /// shapes that own no sub-work (`park_resume`) start here.
    pub fn from_parks(ids: impl IntoIterator<Item = NodeId>) -> Self {
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

    pub fn parks(&self) -> &[NodeId] {
        &self.parks
    }

    pub fn owned(&self) -> &[R] {
        &self.owned
    }

    /// The park-prefix length — the split point of the `[park..., owned...]` delivery order.
    pub fn park_count(&self) -> usize {
        self.parks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.parks.is_empty() && self.owned.is_empty()
    }

    /// Decompose into `(parks, owned)` for the realization loop, which turns each owned `DepRequest`
    /// into a producer id and rebuilds a [`ResolvedDeps`] from the same parks.
    pub fn into_parts(self) -> (Vec<NodeId>, Vec<R>) {
        (self.parks, self.owned)
    }
}

/// A realized dep list — parks and owned deps are all producer ids. This is what
/// [`NodeWork`](super::nodes::NodeWork) stores and the apply harness installs edges from.
pub type ResolvedDeps = Deps<NodeId>;

impl ResolvedDeps {
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
    pub fn new(items: &'a [T], park_count: usize) -> Self {
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
