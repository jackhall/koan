//! A resumption of the static pass ([the build module's design](../../build/README.md#type-facts-and-the-solver)):
//! the load's walk from the program root under a binding, each bound name read as the closed type
//! it binds, re-running only the facts whose frontier waits on what it binds.
//!
//! The walk skips a shape whose reach waits on none of it, and in a shape it walks starts each
//! level from the facts it skips — every fact whose frontier the binding misses, as the
//! resumptions it extends left it, else as the load did — so a rule that reads one finds it, and a
//! rule run at a skipped fact's key returns it without descending. A fact it re-runs is written
//! beside the load's, never into it, and each shape it walks is laid down whole, with its residual
//! ([`Settled`]). The load's bookkeeping for the run — caching, type captures, the path entries —
//! is the load's alone: a resumption reads it, and mints no path entry.

use std::cell::Cell;

use crate::memory::{
    BumpAllocator, BumpBackedMap, BumpBackedSet, BumpVec, Writer, bump_set, bump_table, collect,
    resident,
};
use crate::scope::{
    ChannelSettled, FactKey, Frontier, Keyed, Resolution, Resumption, Segment, Settled, ShapeError,
    ShapeFacts, StaticSolution, StaticsSettled, Unknown,
};
use crate::type_lattice::KType;

/// One of a shape's own facts as a whole, which a resumption re-runs at most once.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Whole {
    Summary,
    Signature,
    Ran,
    Born,
}

impl Whole {
    /// Whether `facts` holds this fact yet. A born instance's own cell holds it, so its frontier
    /// reads as unset.
    pub(super) fn is_set(self, facts: &ShapeFacts<'_>) -> bool {
        match self {
            Whole::Summary => facts.summary.is_some(),
            Whole::Signature => facts.signature.is_some(),
            Whole::Ran => facts.ran.is_some(),
            Whole::Born => false,
        }
    }
}

/// A fact by its key, the newest the walk wrote.
pub(super) type Held<'p, K, T> = Cell<BumpBackedMap<'p, K, T>>;

/// A resumption the static pass runs: what it resumes under, the type channel's settlement under
/// it, and each fact it re-runs.
pub(super) struct Resume<'p, 'graph> {
    pub(super) over: Resumption<'p, 'graph>,
    /// What the type channel's resumption settled.
    pub(super) channel: &'graph Settled<'graph>,
    pub(super) statics: Held<'p, Segment, StaticsSettled<'graph>>,
    pub(super) facts: Held<'p, Segment, ShapeFacts<'graph>>,
    /// Each of a shape's own facts the walk re-ran.
    wholes: Cell<BumpBackedSet<'p, (Segment, Whole)>>,
    pub(super) born: Held<'p, Segment, StaticSolution<'graph>>,
    pub(super) refusals: Held<'p, Segment, &'graph ShapeError<'graph>>,
    pub(super) resolutions: Held<'p, (Segment, usize), Resolution<'graph>>,
    /// Each fact the walk re-ran, by its shape's segment and key.
    pub(super) reran: Cell<BumpBackedSet<'p, (Segment, FactKey)>>,
    scratch: BumpAllocator<'p>,
}

impl<'p, 'graph> Resume<'p, 'graph> {
    pub(super) fn new(
        over: Resumption<'p, 'graph>,
        channel: &'graph Settled<'graph>,
        scratch: BumpAllocator<'p>,
    ) -> Self {
        Resume {
            over,
            channel,
            statics: Cell::new(bump_table(scratch)),
            facts: Cell::new(bump_table(scratch)),
            wholes: Cell::new(bump_set(scratch)),
            born: Cell::new(bump_table(scratch)),
            refusals: Cell::new(bump_table(scratch)),
            resolutions: Cell::new(bump_table(scratch)),
            reran: Cell::new(bump_set(scratch)),
            scratch,
        }
    }

    /// The type the resumption binds `unknown` to.
    pub(super) fn bound(&self, unknown: Unknown) -> Option<KType> {
        self.over.bound(unknown)
    }

    /// Whether a fact waiting on `frontier` re-runs.
    pub(super) fn wakes(&self, frontier: Frontier<'_>) -> bool {
        self.over.wakes(frontier)
    }

    /// What the walk wrote in `held` for `segment`.
    pub(super) fn own<T: Copy>(&self, held: &Held<'p, Segment, T>, segment: Segment) -> Option<T> {
        let table = held.replace(bump_table(self.scratch));
        let found = table.get(&segment).copied();
        held.set(table);
        found
    }

    /// Write `value` for `key` in `held`, in place of what the walk wrote there.
    pub(super) fn write<K: Eq + std::hash::Hash, T>(
        &self,
        held: &Held<'p, K, T>,
        key: K,
        value: T,
    ) {
        let mut table = held.replace(bump_table(self.scratch));
        table.insert(key, value);
        held.set(table);
    }

    /// Record that the walk re-ran `whole` of `segment`.
    pub(super) fn rerun_whole(&self, segment: Segment, whole: Whole) {
        let mut wholes = self.wholes.replace(bump_set(self.scratch));
        wholes.insert((segment, whole));
        self.wholes.set(wholes);
    }

    /// The type channel's cell of `segment`, read through `of`: as this resumption settled it,
    /// else as the newest one it extends did.
    pub(super) fn channel_cell<T>(
        &self,
        segment: Segment,
        of: impl Fn(&ChannelSettled<'graph>) -> Option<T>,
    ) -> Option<T> {
        let settled = self.channel.channel.get(segment).map(|(_, held)| held);
        settled
            .and_then(&of)
            .or_else(|| self.over.channel(segment).find_map(&of))
    }

    /// The type channel's residual of `segment`, where a resumption re-walked it.
    pub(super) fn channel_residual(
        &self,
        segment: Segment,
    ) -> Option<crate::scope::Residual<'graph>> {
        (self
            .channel
            .channel
            .get(segment)
            .map(|(_, held)| held.residual))
        .or_else(|| self.over.channel(segment).next().map(|held| held.residual))
    }

    /// Count a fact the walk re-runs. A fact re-runs at most once in a resumption that skips.
    pub(super) fn rerun(&self, segment: Segment, key: FactKey) {
        let mut reran = self.reran.replace(bump_set(self.scratch));
        let new = reran.insert((segment, key));
        self.reran.set(reran);
        debug_assert!(
            new || self.over.everything,
            "a fact re-runs at most once in a resumption"
        );
    }

    /// Whether the walk re-ran `whole` of `segment`.
    pub(super) fn reran_whole(&self, segment: Segment, whole: Whole) -> bool {
        let wholes = self.wholes.replace(bump_set(self.scratch));
        let found = wholes.contains(&(segment, whole));
        self.wholes.set(wholes);
        found
    }

    /// What the walk settled, beside what the type channel's resumption did, laid down.
    pub(super) fn settled(self, writer: Writer<'graph, 'graph>) -> &'graph Settled<'graph> {
        /// `held` by key, laid down.
        fn laid<'graph, K: Ord + Copy, T: Copy>(
            writer: Writer<'graph, 'graph>,
            scratch: BumpAllocator<'_>,
            held: BumpBackedMap<'_, K, T>,
        ) -> Keyed<'graph, (K, T)> {
            let mut kept = BumpVec::with_capacity_in(held.len(), scratch);
            kept.extend(held);
            kept.sort_unstable_by_key(|(key, _)| *key);
            Keyed::from_sorted(collect(writer, kept.iter().copied()))
        }
        let scratch = self.scratch;
        let mut settled = *self.channel;
        settled.statics = laid(writer, scratch, self.statics.into_inner());
        settled.facts = laid(writer, scratch, self.facts.into_inner());
        settled.born = laid(writer, scratch, self.born.into_inner());
        settled.refusals = laid(writer, scratch, self.refusals.into_inner());
        settled.resolutions = laid(writer, scratch, self.resolutions.into_inner());
        settled.reran.1 = self.reran.into_inner().len() as u32;
        resident(writer, settled)
    }
}
