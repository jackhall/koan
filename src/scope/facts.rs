//! The vocabulary of the solver'held type facts: what a load-time fact waits on, and what a binding
//! settles ([the build module'held design](../build/README.md#type-facts-and-the-solver)).
//!
//! A **fact** is one write-once load-time cell — a type channel cell or a value channel entry —
//! and the rule that computes it runs at one [`FactKey`] of one shape. Every fact a rule run
//! writes at its key shares one [`Frontier`]: the [`Unknown`]s that could change it, read
//! transitively. A fact with an empty frontier is **fixed**; the others, each beside its frontier,
//! are the shape'held [`Residual`]. A [`Binding`] gives some unknowns closed types, and **wakes**
//! every fact whose frontier meets it.
//!
//! An unknown is named by its **lexical address** — the segment of the body that declares it and
//! the slot or capture its activation holds it at — never by its symbol or its lattice node, since
//! every body numbering its chain alike shares one node per level. `scope` holds the vocabulary,
//! since `elaborate` and `dispatch` both name it and neither names `build`; the passes compute it,
//! and `build` lists the seams it decides.

use crate::memory::{BumpVec, Writer, collect};
use crate::type_lattice::{KType, Parametric};

use super::keyed::Keyed;
use super::shape::{CaptureSlot, Segment, ShapeError, Site, Slot};
use super::typed::{
    Resolution, ShapeFacts, StaticCallable, StaticRegistered, StaticSolution, StaticType, Statics,
};

/// Where a body'held activation holds an unknown: a slot, or — for a name a quote'held code is run
/// with — a capture.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Address {
    Slot(Slot),
    Capture(CaptureSlot),
}

/// A name a binding may remove, by its lexical address: the segment of the body declaring it, and
/// where that body'held activation holds it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Unknown {
    pub segment: Segment,
    pub at: Address,
}

/// Every unknown that could change a fact, transitively: sorted and deduplicated. Empty for a
/// fixed fact.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Frontier<'a>(&'a [Unknown]);

impl<'a> Frontier<'a> {
    /// The frontier of a fixed fact.
    pub const EMPTY: Frontier<'static> = Frontier(&[]);

    /// `unknowns`, already sorted and deduplicated.
    pub fn from_sorted(unknowns: &'a [Unknown]) -> Self {
        debug_assert!(unknowns.is_sorted() && unknowns.windows(2).all(|pair| pair[0] != pair[1]));
        Frontier(unknowns)
    }

    /// `staged`, sorted, deduplicated and laid down through `writer`.
    pub fn laid(writer: Writer<'a, '_>, staged: &mut BumpVec<'_, Unknown>) -> Self {
        staged.sort_unstable();
        staged.dedup();
        Frontier(collect(writer, staged.iter().copied()))
    }

    pub fn unknowns(self) -> &'a [Unknown] {
        self.0
    }

    pub fn is_empty(self) -> bool {
        self.0.is_empty()
    }

    pub fn contains(self, unknown: Unknown) -> bool {
        self.0.binary_search(&unknown).is_ok()
    }

    /// Whether `binding` gives a type to some unknown here: whether it wakes the fact.
    pub fn meets(self, binding: Binding<'_>) -> bool {
        let (mut mine, mut bound) = (self.0.iter().peekable(), binding.0.iter().peekable());
        while let (Some(unknown), Some((other, _))) = (mine.peek(), bound.peek()) {
            match (*unknown).cmp(other) {
                std::cmp::Ordering::Less => _ = mine.next(),
                std::cmp::Ordering::Greater => _ = bound.next(),
                std::cmp::Ordering::Equal => return true,
            }
        }
        false
    }

    /// This frontier with `other`'held unknowns, laid down through `writer` where it grows.
    pub fn union<'w>(self, other: Frontier<'_>, writer: Writer<'w, '_>) -> Frontier<'w>
    where
        'a: 'w,
    {
        if other.0.iter().all(|unknown| self.contains(*unknown)) {
            return self;
        }
        let merged = || Merged {
            mine: self.0,
            theirs: other.0,
        };
        let count = merged().count();
        let mut each = merged();
        Frontier(writer.fill(count, |_| each.next().expect("the merge yields its count")))
    }
}

/// Two sorted, deduplicated runs of unknowns merged, each unknown once.
struct Merged<'a, 'other> {
    mine: &'a [Unknown],
    theirs: &'other [Unknown],
}

impl Iterator for Merged<'_, '_> {
    type Item = Unknown;

    fn next(&mut self) -> Option<Unknown> {
        let next = match (self.mine.first(), self.theirs.first()) {
            (None, None) => return None,
            (Some(mine), None) => *mine,
            (None, Some(theirs)) => *theirs,
            (Some(mine), Some(theirs)) => *mine.min(theirs),
        };
        if self.mine.first() == Some(&next) {
            self.mine = &self.mine[1..];
        }
        if self.theirs.first() == Some(&next) {
            self.theirs = &self.theirs[1..];
        }
        Some(next)
    }
}

/// Add `unknowns` to the sorted, deduplicated run `into`.
pub fn merge_into(into: &mut BumpVec<'_, Unknown>, unknowns: &[Unknown]) {
    for unknown in unknowns {
        if let Err(at) = into.binary_search(unknown) {
            into.insert(at, *unknown);
        }
    }
}

/// Where a fact'held rule runs in its shape: a part, a node, a use or a type expression by its site;
/// a statement by its index; a binder, a type binder or a registration by its slot; and the
/// shape'held own facts — a callable'held type, its group levels, its summary — at the shape.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum FactKey {
    Site(Site),
    Statement(u32),
    Slot(Slot),
    Shape,
}

/// A shape'held facts that wait on an unknown, each by its key beside its frontier, sorted by key:
/// every other fact of the shape is fixed.
pub type Residual<'a> = Keyed<'a, (FactKey, Frontier<'a>)>;

/// The frontier `residual` records for `key`: empty where the fact is fixed.
pub fn frontier_at<'a>(residual: Residual<'a>, key: FactKey) -> Frontier<'a> {
    residual
        .get(key)
        .map_or(Frontier(&[]), |(_, frontier)| *frontier)
}

/// Some unknowns, each beside the closed type a binding gives it, sorted by unknown: a type for a
/// type name, and the exact type a module binder carries for a module unknown.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Binding<'a>(&'a [(Unknown, KType)]);

impl<'a> Binding<'a> {
    /// The binding that binds nothing.
    pub const EMPTY: Binding<'static> = Binding(&[]);

    /// `staged`, sorted and laid down through `writer`. Each unknown is bound once.
    pub fn laid(writer: Writer<'a, '_>, staged: &mut BumpVec<'_, (Unknown, KType)>) -> Self {
        staged.sort_unstable_by_key(|(unknown, _)| *unknown);
        debug_assert!(
            staged.windows(2).all(|pair| pair[0].0 != pair[1].0),
            "a binding binds each unknown once"
        );
        Binding(collect(writer, staged.iter().copied()))
    }

    /// The type `unknown` is bound to, where it is bound.
    pub fn bound(self, unknown: Unknown) -> Option<KType> {
        let at = self
            .0
            .binary_search_by_key(&unknown, |(held, _)| *held)
            .ok()?;
        Some(self.0[at].1)
    }

    pub fn entries(self) -> &'a [(Unknown, KType)] {
        self.0
    }

    pub fn is_empty(self) -> bool {
        self.0.is_empty()
    }

    /// Whether this binding binds every unknown `other` binds, each to the same type.
    pub fn extends(self, other: Binding<'_>) -> bool {
        (other.0.iter()).all(|(unknown, bound)| self.bound(*unknown) == Some(*bound))
    }
}

/// Where in a body a seam lies: at its entry, or at a unit, by index into the body'held units — the
/// order a body binds in, entry first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum SeamAt {
    Entry,
    Unit(u32),
}

/// A binding site at which some frontier'held last member binds, beside every unknown the frontiers
/// completed there wait on: where a resumption over the binding the seam takes lands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Seam<'a> {
    pub at: SeamAt,
    pub waits_on: Frontier<'a>,
}

/// What a resumption settled about one body'held type channel cells: each cell it re-ran, by where
/// it lies, beside the body'held residual as the resumption leaves it.
#[derive(Clone, Copy)]
pub struct ChannelSettled<'a> {
    pub declared: Keyed<'a, (Slot, StaticType<'a>)>,
    pub registered: Keyed<'a, (Slot, StaticRegistered<'a>)>,
    pub expressions: Keyed<'a, (Site, StaticType<'a>)>,
    /// A callable body'held type, and its own group as its body reads it, where it re-ran.
    pub callable: Option<StaticCallable<'a>>,
    pub levels: Option<&'a [Parametric]>,
    pub residual: Residual<'a>,
    /// A quote'held code'held refusal, where its code no longer types.
    pub refusal: Option<&'a ShapeError<'a>>,
}

/// What a resumption settled about one body'held value channel: the body'held cell as the resumption
/// leaves it, beside its residual.
#[derive(Clone, Copy)]
pub struct StaticsSettled<'a> {
    pub statics: Statics<'a>,
    pub residual: Residual<'a>,
}

/// What one resumption settles: its binding, and each body it re-walked, by segment — the
/// facts it re-ran beside the residual it leaves. A fact it did not re-run reads as the
/// resumption it extends left it, and as the load left it below them all.
#[derive(Clone, Copy)]
pub struct Settled<'a> {
    pub binding: Binding<'a>,
    pub channel: Keyed<'a, (Segment, ChannelSettled<'a>)>,
    pub statics: Keyed<'a, (Segment, StaticsSettled<'a>)>,
    /// Each body'held own facts as a whole, where one of them re-ran.
    pub facts: Keyed<'a, (Segment, ShapeFacts<'a>)>,
    /// Each callable body'held born instance, where it re-ran.
    pub born: Keyed<'a, (Segment, StaticSolution<'a>)>,
    /// Each quote'held code the value channel no longer types, beside its refusal.
    pub refusals: Keyed<'a, (Segment, &'a ShapeError<'a>)>,
    /// Each path re-resolved, by its chain root'held segment and its level.
    pub resolutions: Keyed<'a, ((Segment, usize), Resolution<'a>)>,
    /// How many facts each pass re-ran: the type channel'held, then the value channel'held.
    pub reran: (u32, u32),
}

impl<'a> Settled<'a> {
    /// A resumption under `binding` that settled nothing yet.
    pub fn empty(binding: Binding<'a>) -> Self {
        Settled {
            binding,
            channel: Keyed::from_sorted(&[]),
            statics: Keyed::from_sorted(&[]),
            facts: Keyed::from_sorted(&[]),
            born: Keyed::from_sorted(&[]),
            refusals: Keyed::from_sorted(&[]),
            resolutions: Keyed::from_sorted(&[]),
            reran: (0, 0),
        }
    }
}

/// A resumption a pass runs: the binding it resumes under, the resumptions it extends, newest
/// first, and whether it re-runs every fact rather than the ones the binding wakes — what the
/// law that quiescence is exact compares it with.
#[derive(Clone, Copy)]
pub struct Resumption<'held, 'a> {
    pub binding: Binding<'a>,
    pub base: &'held [&'a Settled<'a>],
    pub everything: bool,
}

impl<'held, 'a> Resumption<'held, 'a> {
    /// The type `unknown` is bound to.
    pub fn bound(self, unknown: Unknown) -> Option<KType> {
        self.binding.bound(unknown)
    }

    /// Whether `frontier` waits on an unknown this resumption binds and the ones it extends do
    /// not: whether the fact re-runs.
    pub fn wakes(self, frontier: Frontier<'_>) -> bool {
        let prior = self
            .base
            .first()
            .map_or(Binding::EMPTY, |base| base.binding);
        self.everything
            || (frontier.unknowns().iter())
                .any(|unknown| self.bound(*unknown).is_some() && prior.bound(*unknown).is_none())
    }

    /// The newest of `found` over the resumptions this one extends.
    fn newest<T>(self, found: impl Fn(&Settled<'a>) -> Option<T>) -> Option<T> {
        self.base.iter().find_map(|settled| found(settled))
    }

    pub fn channel(self, segment: Segment) -> impl Iterator<Item = &'a ChannelSettled<'a>> + 'held {
        (self.base.iter()).filter_map(move |settled| {
            let Settled { channel, .. } = **settled;
            channel.get(segment).map(|(_, held)| held)
        })
    }

    pub fn statics(self, segment: Segment) -> Option<StaticsSettled<'a>> {
        self.newest(|settled| settled.statics.get(segment).map(|(_, held)| *held))
    }

    pub fn facts(self, segment: Segment) -> Option<ShapeFacts<'a>> {
        self.newest(|settled| settled.facts.get(segment).map(|(_, held)| *held))
    }

    pub fn born(self, segment: Segment) -> Option<StaticSolution<'a>> {
        self.newest(|settled| settled.born.get(segment).map(|(_, held)| *held))
    }

    pub fn refusal(self, segment: Segment) -> Option<&'a ShapeError<'a>> {
        self.newest(|settled| settled.refusals.get(segment).map(|(_, held)| *held))
    }

    pub fn resolution(self, root: Segment, level: usize) -> Option<Resolution<'a>> {
        self.newest(|settled| {
            settled
                .resolutions
                .get((root, level))
                .map(|(_, held)| *held)
        })
    }
}
