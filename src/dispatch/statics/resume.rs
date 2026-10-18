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

use crate::memory::{BumpAllocator, BumpBackedSet, BumpVec, Writer, bump_set, collect, resident};
use crate::scope::{
    ChannelSettled, FactKey, Frontier, Keyed, Resolution, Resumption, Segment, Settled, ShapeError,
    ShapeFacts, StaticSolution, StaticsSettled, Unknown,
};
use crate::type_lattice::KType;

/// One of a shape's own facts as a whole, which a resumption re-runs at most once.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Whole {
    Summary,
    Signature,
    Ran,
    Born,
}

/// A resumption the static pass runs: what it resumes under, the type channel's settlement under
/// it, and each fact it re-runs.
pub(super) struct Resume<'p, 'graph> {
    pub(super) over: Resumption<'p, 'graph>,
    /// What the type channel's resumption settled.
    pub(super) channel: &'graph Settled<'graph>,
    pub(super) statics: Cell<BumpVec<'p, (Segment, StaticsSettled<'graph>)>>,
    pub(super) facts: Cell<BumpVec<'p, (Segment, ShapeFacts<'graph>)>>,
    /// Each of a shape's own facts the walk re-ran.
    pub(super) wholes: Cell<BumpVec<'p, (Segment, Whole)>>,
    pub(super) born: Cell<BumpVec<'p, (Segment, StaticSolution<'graph>)>>,
    pub(super) refusals: Cell<BumpVec<'p, (Segment, &'graph ShapeError<'graph>)>>,
    pub(super) resolutions: Cell<BumpVec<'p, ((Segment, usize), Resolution<'graph>)>>,
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
            statics: Cell::new(BumpVec::new_in(scratch)),
            facts: Cell::new(BumpVec::new_in(scratch)),
            wholes: Cell::new(BumpVec::new_in(scratch)),
            born: Cell::new(BumpVec::new_in(scratch)),
            refusals: Cell::new(BumpVec::new_in(scratch)),
            resolutions: Cell::new(BumpVec::new_in(scratch)),
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

    /// The newest of `held` the walk wrote for `segment`.
    pub(super) fn own<T: Copy>(
        &self,
        held: &Cell<BumpVec<'p, (Segment, T)>>,
        segment: Segment,
    ) -> Option<T> {
        let run = held.replace(BumpVec::new_in(self.scratch));
        let found = (run.iter().rev())
            .find(|(at, _)| *at == segment)
            .map(|(_, found)| *found);
        held.set(run);
        found
    }

    /// Write `value` for `segment` into `held`.
    pub(super) fn write<K, T>(&self, held: &Cell<BumpVec<'p, (K, T)>>, key: K, value: T) {
        let mut run = held.replace(BumpVec::new_in(self.scratch));
        run.push((key, value));
        held.set(run);
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
        let run = self.wholes.replace(BumpVec::new_in(self.scratch));
        let found = run.contains(&(segment, whole));
        self.wholes.set(run);
        found
    }

    /// What the walk settled, beside what the type channel's resumption did, laid down.
    pub(super) fn settled(self, writer: Writer<'graph, 'graph>) -> &'graph Settled<'graph> {
        /// `held` by key, the newest of each, laid down.
        fn newest<'graph, K: Ord + Copy, T: Copy>(
            writer: Writer<'graph, 'graph>,
            scratch: BumpAllocator<'_>,
            held: &mut BumpVec<'_, (K, T)>,
        ) -> Keyed<'graph, (K, T)> {
            // A stable sort keeps the writes to one key in order, the newest last.
            held.sort_by_key(|(key, _)| *key);
            let mut kept = BumpVec::with_capacity_in(held.len(), scratch);
            for (key, value) in held.iter().copied() {
                match kept.last_mut() {
                    Some((last, newest)) if *last == key => *newest = value,
                    _ => kept.push((key, value)),
                }
            }
            Keyed::from_sorted(collect(writer, kept.iter().copied()))
        }
        let scratch = self.scratch;
        let mut settled = *self.channel;
        settled.statics = newest(writer, scratch, &mut self.statics.into_inner());
        settled.facts = newest(writer, scratch, &mut self.facts.into_inner());
        settled.born = newest(writer, scratch, &mut self.born.into_inner());
        settled.refusals = newest(writer, scratch, &mut self.refusals.into_inner());
        settled.resolutions = newest(writer, scratch, &mut self.resolutions.into_inner());
        settled.reran.1 = self.reran.into_inner().len() as u32;
        resident(writer, settled)
    }
}
