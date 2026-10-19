//! A resumption of the static pass ([the build module's design](../../build/README.md#type-facts-and-the-solver)):
//! the load's walk from the program root under a binding, each bound name read as the closed type
//! it binds, re-running only the facts whose frontier waits on what it binds.
//!
//! The walk skips a shape whose reach waits on none of it, and in a shape it walks starts each
//! level from the facts it skips — every fact whose frontier the binding misses, as the
//! resumptions it extends left it, else as the load did — so a rule that reads one finds it, and a
//! rule run at a skipped fact's key returns it without descending. A fact it re-runs is kept
//! beside the load's, never in it, and each shape it walks is kept whole, with its residual: in
//! [`Resume`], the write half of the store a resumption runs over, which relays what it keeps into
//! the [`Settled`] it lays once the walk ends. The load's bookkeeping for the run — caching, type
//! captures, the path entries — is the load's alone: a resumption reads it, and mints no path entry
//! and no type capture.

use std::cell::Cell;

use crate::memory::{
    BumpAllocator, BumpBackedMap, BumpBackedSet, BumpVec, bump_set, bump_table, resident,
};
use crate::scope::{
    BodyShape, Caching, Coordinate, FactKey, Keyed, PathEntry, Relay, Resolution, Segment, Settled,
    ShapeError, ShapeFacts, StaticSolution, StaticsSettled,
};
use crate::type_lattice::Habitat;

use super::store::Write;

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
type Held<'p, K, T> = Cell<BumpBackedMap<'p, K, T>>;

/// A resumption's write half: each fact the walk re-runs, kept in the walk's scratch.
pub(super) struct Resume<'p> {
    /// Whether the resumption re-runs every fact, not only those its binding wakes.
    everything: bool,
    statics: Held<'p, Segment, StaticsSettled<'p>>,
    facts: Held<'p, Segment, ShapeFacts<'p>>,
    /// Each of a shape's own facts the walk re-ran.
    wholes: Cell<BumpBackedSet<'p, (Segment, Whole)>>,
    born: Held<'p, Segment, StaticSolution<'p>>,
    refusals: Held<'p, Segment, &'p ShapeError<'p>>,
    resolutions: Held<'p, (Segment, usize), Resolution<'p>>,
    /// Each fact the walk re-ran, by its shape's segment and key.
    reran: Cell<BumpBackedSet<'p, (Segment, FactKey)>>,
    scratch: BumpAllocator<'p>,
}

impl<'p> Resume<'p> {
    pub(super) fn new(everything: bool, scratch: BumpAllocator<'p>) -> Self {
        Resume {
            everything,
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

    /// What the walk wrote in `held` for `segment`.
    fn own<T: Copy>(&self, held: &Held<'p, Segment, T>, segment: Segment) -> Option<T> {
        let table = held.replace(bump_table(self.scratch));
        let found = table.get(&segment).copied();
        held.set(table);
        found
    }

    /// Write `value` for `key` in `held`, in place of what the walk wrote there.
    fn write<K: Eq + std::hash::Hash, T>(&self, held: &Held<'p, K, T>, key: K, value: T) {
        let mut table = held.replace(bump_table(self.scratch));
        table.insert(key, value);
        held.set(table);
    }

    /// What the walk settled, beside `channel`, what the type channel's resumption settled: the
    /// facts it kept relayed out through `relay`, and one `Settled` laid holding both.
    pub(super) fn settled<'out, 'w, 'from, H: Habitat<'from>>(
        &self,
        channel: Settled<'out>,
        relay: &Relay<'out, 'w, 'from, H>,
    ) -> &'out Settled<'out> {
        let scratch = self.scratch;
        /// `held` by key, sorted.
        fn sorted<'a, K: Ord + Copy, T: Copy>(
            scratch: BumpAllocator<'a>,
            held: BumpBackedMap<'_, K, T>,
        ) -> Keyed<'a, (K, T)> {
            let mut kept = BumpVec::with_capacity_in(held.len(), scratch);
            kept.extend(held);
            Keyed::sorted(scratch, &mut kept)
        }
        let statics = sorted(scratch, self.statics.replace(bump_table(scratch)));
        let facts = sorted(scratch, self.facts.replace(bump_table(scratch)));
        let born = sorted(scratch, self.born.replace(bump_table(scratch)));
        let refusals = sorted(scratch, self.refusals.replace(bump_table(scratch)));
        let resolutions = sorted(scratch, self.resolutions.replace(bump_table(scratch)));
        let mut settled = channel;
        settled.statics = relay.keyed(statics, |held| relay.statics_settled(*held));
        settled.facts = relay.keyed(facts, |held| relay.shape_facts(*held));
        settled.born = relay.keyed(born, |held| relay.static_solution(*held));
        settled.refusals = relay.keyed(refusals, |refusal| relay.shape_error_ref(refusal));
        settled.resolutions = relay.keyed(resolutions, |held| relay.resolution(*held));
        settled.reran.1 = self.reran.replace(bump_set(scratch)).len() as u32;
        resident(relay.writer(), settled)
    }
}

impl<'p, 'graph> Write<'p, 'graph> for Resume<'p> {
    fn fix_statics(&self, shape: &'graph BodyShape<'graph>, settled: StaticsSettled<'p>) {
        self.write(&self.statics, shape.segment(), settled);
    }

    fn fix_shape_facts(
        &self,
        shape: &'graph BodyShape<'graph>,
        whole: Whole,
        facts: ShapeFacts<'p>,
    ) {
        let segment = shape.segment();
        debug_assert!(
            self.everything || !Write::<'p, 'graph>::reran_whole(self, segment, whole),
            "a shape's fact as a whole re-runs at most once in a resumption"
        );
        self.write(&self.facts, segment, facts);
        let mut wholes = self.wholes.replace(bump_set(self.scratch));
        wholes.insert((segment, whole));
        self.wholes.set(wholes);
    }

    fn fix_born_instance(&self, body: &'graph BodyShape<'graph>, solution: StaticSolution<'p>) {
        self.write(&self.born, body.segment(), solution);
    }

    fn refuse_code(&self, code: &'graph BodyShape<'graph>, refusal: ShapeError<'p>) {
        let refusal = resident(self.scratch, refusal);
        self.write(&self.refusals, code.segment(), refusal);
    }

    fn fix_resolutions(
        &self,
        root: &'graph BodyShape<'graph>,
        resolutions: &mut [(usize, Resolution<'p>)],
    ) {
        for (level, resolution) in resolutions.iter() {
            self.write(&self.resolutions, (root.segment(), *level), *resolution);
        }
    }

    fn fix_paths(&self, root: &'graph BodyShape<'graph>, paths: &'p [PathEntry<'p>]) {
        assert_eq!(
            paths.len(),
            root.paths().len(),
            "a resumption mints no path entry"
        );
    }

    fn fix_type_captures(&self, _: &'graph BodyShape<'graph>, captures: &'p [Coordinate]) {
        assert!(captures.is_empty(), "a resumption lays no type capture");
    }

    fn fix_caching(&self, _: &'graph BodyShape<'graph>, _: Caching<'p>) {
        unreachable!("only the load's entry fixes where a body's types intern")
    }

    fn own_statics(&self, segment: Segment) -> Option<StaticsSettled<'p>> {
        self.own(&self.statics, segment)
    }

    fn own_facts(&self, segment: Segment) -> Option<ShapeFacts<'p>> {
        self.own(&self.facts, segment)
    }

    fn own_born(&self, segment: Segment) -> Option<StaticSolution<'p>> {
        self.own(&self.born, segment)
    }

    fn own_refusal(&self, segment: Segment) -> bool {
        self.own(&self.refusals, segment).is_some()
    }

    fn reran_whole(&self, segment: Segment, whole: Whole) -> bool {
        let wholes = self.wholes.replace(bump_set(self.scratch));
        let found = wholes.contains(&(segment, whole));
        self.wholes.set(wholes);
        found
    }

    /// A fact re-runs at most once in a resumption that skips.
    fn count(&self, segment: Segment, key: FactKey) {
        let mut reran = self.reran.replace(bump_set(self.scratch));
        let new = reran.insert((segment, key));
        self.reran.set(reran);
        debug_assert!(
            new || self.everything,
            "a fact re-runs at most once in a resumption"
        );
    }

    fn bookkeeps(&self) -> bool {
        false
    }
}
