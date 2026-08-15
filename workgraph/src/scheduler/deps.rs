//! The two scheduler-side dep-list currencies.
//!
//! A node's dep list is one vector, and **dep order is read order**: results reach a finish in the
//! order the builder appended them. Every dep is named by a **source edge** and inherits that
//! source's destination region — one rule, whether the source is a binding the embedder already
//! holds or one the apply harness minted over sub-work it spawned.
//!
//! - [`Deps`] is the write side — what builder production code assembles a dep list with. Its
//!   entries are [`Dep`]s, which distinguish only *realization phase*: a dep the caller can already
//!   name versus a request the harness must spawn work for. That distinction dies at the apply
//!   boundary. Generic in the request type `R` (`DepRequest` before the harness realizes each one).
//! - [`ResolvedDeps`] is the realized list [`NodeWork`](super::nodes::NodeWork) stores: every entry
//!   is the consumer's **own** edge, minted by the install door. It is the consumer's ownership
//!   record as well as its read list — the run loop reads each dep's resident through these names
//!   and releases them when the step is done with them.
//!
//! Everything here is plain or type-parameter-generic (`EdgeId`, `usize`, `R`) — it names no Koan
//! value, error, or AST type.

use super::EdgeId;

/// One dep, in the phase the caller can name it in. Both arms are the same dep; the distinction is
/// whether it is realized yet, and it does not outlive the apply harness.
pub enum Dep<R> {
    /// A dep the caller can already name, by the source edge it holds.
    Producer(EdgeId),
    /// Sub-work the harness realizes into a producer, then a source edge.
    Request(R),
}

/// The dep-list builder: the one way production code assembles a node's dep list. One vector, in
/// append order, which is the order results come back in. Generic in the request type `R`: a
/// `DepRequest` before the apply harness realizes each request, nothing after — past realization
/// every dep is one [`EdgeId`].
///
/// A dep is named by the **source edge** it is wired off, never by a producer: the install door
/// ([`Scheduler::install_deps`](super::Scheduler::install_deps)) mints the consumer's own edge off
/// each source, so the producer currency never leaves the scheduler.
pub struct Deps<R> {
    entries: Vec<Dep<R>>,
}

impl<R> Deps<R> {
    pub fn new() -> Self {
        Deps {
            entries: Vec::new(),
        }
    }

    /// Append a dep on the source edge `id`. No dedup: an expression naming one source twice gets
    /// two deps, hence two edges off that one producer. Both inherit the same destination, so the
    /// delivery walk's per-destination dedup collapses them to a single adopt — the cost is one
    /// recycled slab entry, and the gain is that dep order stays read order.
    pub fn on(&mut self, id: EdgeId) {
        self.entries.push(Dep::Producer(id));
    }

    /// Append a request for sub-work. Returns its **entry index** — the position in the one dep
    /// list, which is the position its result comes back at. Callers that read their results in
    /// order ignore it.
    pub fn request(&mut self, entry: R) -> usize {
        let pos = self.entries.len();
        self.entries.push(Dep::Request(entry));
        pos
    }

    /// Build a dep list naming source edges only — the park-and-replay shapes that spawn no sub-work
    /// start here.
    pub fn from_producers(ids: impl IntoIterator<Item = EdgeId>) -> Self {
        let mut deps = Deps::new();
        for id in ids {
            deps.on(id);
        }
        deps
    }

    /// Build a dep list whose every entry is a request — the shape a dispatch decide takes when it
    /// has no already-named producers to wait on.
    pub fn from_requests(entries: impl IntoIterator<Item = R>) -> Self {
        let mut deps = Deps::new();
        for entry in entries {
            deps.request(entry);
        }
        deps
    }

    pub fn entries(&self) -> &[Dep<R>] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Hand the entries to the realization loop, which walks them in order into source edges.
    pub fn into_entries(self) -> Vec<Dep<R>> {
        self.entries
    }
}

/// A realized dep list: every entry is the consumer's **own** edge. This is what
/// [`NodeWork`](super::nodes::NodeWork) stores, what the run loop reads each dep's delivered
/// resident through, and the record that says which edges this slot releases when it is done.
///
/// Deliberately *not* `Deps<EdgeId>`: [`Deps`] is the embedder's write side, whose entries name
/// *source* edges; a realized list names the consumer's own edges, minted off those sources by the
/// install door — which is why the only way to put one here is the door's own crate-private append.
pub struct ResolvedDeps {
    /// The consumer's edges, one per dep the embedder handed the door, in its order.
    ids: Vec<EdgeId>,
}

impl ResolvedDeps {
    pub fn new() -> Self {
        ResolvedDeps { ids: Vec::new() }
    }

    /// Append one dep's edge — the install door's push, one entry per source it was handed. Never
    /// deduping: alignment is the point, since a caller's dep index addresses its own source list.
    pub(crate) fn push(&mut self, id: EdgeId) {
        self.ids.push(id);
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    /// The edges in dep order. The run loop reads each dep's resident in this order, so a finish's
    /// result slice lines up with the list the builder wrote, and releases them in it too.
    pub fn all_ids(&self) -> impl Iterator<Item = EdgeId> + '_ {
        self.ids.iter().copied()
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
