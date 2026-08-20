//! The two scheduler-side dep-list currencies.
//!
//! A node's dep list is one vector, and **dep order is read order**: results reach a finish in the
//! order the builder appended them. Every dep is named by a **source edge** and inherits that
//! source's destination region — one rule, whether the source is a binding the embedder already
//! holds or one the apply harness minted over sub-work it spawned.
//!
//! - [`Deps`] is the embedder's write side. Its entries are [`Dep`]s, which distinguish only
//!   *realization phase*: a dep the caller can already name versus a request the harness must spawn
//!   work for. That distinction dies at the apply boundary, so the request type stays generic.
//! - [`ResolvedDeps`] is the realized list the slot's dep row stores: every entry is the consumer's
//!   **own** edge, minted by the install door. It is the consumer's ownership record as well as its
//!   read list — the drain reads each dep's resident through these names at step start and releases
//!   them there.
//!
//! See [design/dag-scheduler.md § The dep row and its
//! invariants](../../design/dag-scheduler.md#the-dep-row-and-its-invariants).

use allocator_api2::alloc::{Allocator, Global};
use allocator_api2::vec::Vec as AllocVec;

use super::EdgeId;

/// One dep, in the phase the caller can name it in: [`Producer`](Dep::Producer) carries the
/// **source** edge the caller already holds, [`Request`](Dep::Request) the sub-work the harness
/// realizes into a producer and then a source edge. Both arms are the same dep, and the distinction
/// does not outlive the apply harness.
pub enum Dep<R> {
    Producer(EdgeId),
    Request(R),
}

/// A dep-list builder: one vector, in append order, which is the order results come back in.
/// Generic in the embedder's request type `R` — past realization every dep is one [`EdgeId`] — and
/// in the allocator `A` its entries are hosted on.
///
/// A dep is named by the **source edge** it is wired off, never by a producer: the install door
/// ([`Scheduler::install_deps`](super::Scheduler::install_deps)) mints the consumer's own edge off
/// each source, so the producer currency never leaves the scheduler.
///
/// `A` defaults to [`Global`], which is what a submission door outside any step builds on. A park
/// wired *inside* a step passes the drain's scratch handle instead
/// ([`Step::scratch`](super::Step::scratch)), so the list dies with the pop that built it. Each
/// constructor comes in a pair: the bare name fixes `Global`, the `_in` suffix takes the allocator.
/// Prefer [`with_capacity_in`](Self::with_capacity_in) on a bump — a bump-hosted vector that grows
/// abandons its old buffer.
pub struct Deps<R, A: Allocator = Global> {
    entries: AllocVec<Dep<R>, A>,
}

impl<R> Deps<R> {
    pub fn new() -> Self {
        Deps::new_in(Global)
    }

    pub fn from_producers(ids: impl IntoIterator<Item = EdgeId>) -> Self {
        Deps::from_producers_in(ids, Global)
    }

    pub fn from_requests(entries: impl IntoIterator<Item = R>) -> Self {
        Deps::from_requests_in(entries, Global)
    }
}

impl<R, A: Allocator> Deps<R, A> {
    pub fn new_in(alloc: A) -> Self {
        Deps {
            entries: AllocVec::new_in(alloc),
        }
    }

    pub fn with_capacity_in(capacity: usize, alloc: A) -> Self {
        Deps {
            entries: AllocVec::with_capacity_in(capacity, alloc),
        }
    }

    /// No dedup: an expression naming one source twice gets two deps, hence two edges off that one
    /// producer. Both inherit the same destination, so the delivery walk's per-destination dedup
    /// collapses them to a single adopt — the cost is one recycled slab entry, and the gain is that
    /// dep order stays read order.
    pub fn on(&mut self, id: EdgeId) {
        self.entries.push(Dep::Producer(id));
    }

    /// Append a request for sub-work. The returned **entry index** is its position in the dep list,
    /// which is the position its result comes back at.
    pub fn request(&mut self, entry: R) -> usize {
        let pos = self.entries.len();
        self.entries.push(Dep::Request(entry));
        pos
    }

    pub fn from_producers_in(ids: impl IntoIterator<Item = EdgeId>, alloc: A) -> Self {
        let ids = ids.into_iter();
        let mut deps = Deps::with_capacity_in(ids.size_hint().0, alloc);
        for id in ids {
            deps.on(id);
        }
        deps
    }

    pub fn from_requests_in(entries: impl IntoIterator<Item = R>, alloc: A) -> Self {
        let entries = entries.into_iter();
        let mut deps = Deps::with_capacity_in(entries.size_hint().0, alloc);
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

    pub fn into_entries(self) -> AllocVec<Dep<R>, A> {
        self.entries
    }
}

/// A realized dep list: every entry is the consumer's **own** edge. It lives on the slot's dep row
/// — written there by the install door, read at step start by the drain, which pulls each dep's
/// delivered resident through it and releases the edges. Scheduler-internal currency: an embedder
/// never holds one.
///
/// Deliberately *not* `Deps<EdgeId>`: [`Deps`] entries name *source* edges, while a realized list
/// names the consumer's own edges, minted off those sources by the install door — which is why
/// `push` is `pub(crate)`, so no embedder appends.
pub struct ResolvedDeps {
    ids: Vec<EdgeId>,
}

impl ResolvedDeps {
    pub fn new() -> Self {
        ResolvedDeps { ids: Vec::new() }
    }

    /// Never deduping: alignment is the point, since a caller's dep index addresses its own source
    /// list.
    pub(crate) fn push(&mut self, id: EdgeId) {
        self.ids.push(id);
    }

    /// The edges in dep order, so a finish's result slice lines up with the list the builder wrote.
    pub fn all_ids(&self) -> impl Iterator<Item = EdgeId> + '_ {
        self.ids.iter().copied()
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// Empty the list while keeping the buffer, so a row handed its list back keeps the capacity it
    /// grew. The recycling half of the take-and-restore pair the drain and the delivery walk run.
    pub(crate) fn clear(&mut self) {
        self.ids.clear();
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
