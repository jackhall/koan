//! The vocabulary of the solver's type facts: what a load-time fact waits on, and the binding
//! that wakes it ([the build module's design](../build/README.md#type-facts-and-the-solver)).
//!
//! A **fact** is one write-once load-time cell — a type channel cell or a value channel entry —
//! and the rule that computes it runs at one [`FactKey`] of one shape. Every fact a rule run
//! writes at its key shares one [`Frontier`]: the [`Unknown`]s that could change it, read
//! transitively. A fact with an empty frontier is **fixed**; the others, each beside its frontier,
//! are the shape's [`Residual`]. A [`OldBinding`] gives some unknowns closed types, and **wakes**
//! every fact whose frontier meets it.
//!
//! The one-walk build keys its facts by unit instead ([`UnitId`]): one record per unit, beside
//! the frontier the unit gathered, kept in a [`Store`]. Its [`Binding`] gives each unknown a
//! [`Bound`]: a closed type, or for a module binder a closed content tree. [`OldBinding`] and the
//! per-fact vocabulary go with the passes that use them
//! ([the one-walk build](../../roadmap/gradual-build/one-walk-build.md)).
//!
//! An unknown is named by its **lexical address** — the segment of the body that declares it and
//! the slot or capture its activation holds it at — never by its symbol or its lattice node, since
//! every body numbering its chain alike shares one node per level. `scope` holds the vocabulary,
//! since `elaborate` and `dispatch` both name it and neither names `build`; the passes compute it,
//! and `build` lists the seams it decides.

use crate::memory::{BumpVec, Lays, collect};
use crate::type_lattice::KType;

use super::keyed::Keyed;
use super::records::{Closing, UnitFacts};
use super::shape::{CaptureSlot, Segment, ShapeError, Site, Slot, UnitIndex};
use super::tree::Tree;

/// Where a body's activation holds an unknown: a slot, or — for a name a quote's code is run
/// with — a capture.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Address {
    Slot(Slot),
    Capture(CaptureSlot),
}

/// A name a binding may remove, by its lexical address: the segment of the body declaring it, and
/// where that body's activation holds it.
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

    /// `staged`, sorted, deduplicated and laid down through `lay`.
    pub fn laid(lay: impl Lays<'a>, staged: &mut BumpVec<'_, Unknown>) -> Self {
        staged.sort_unstable();
        staged.dedup();
        Frontier(collect(lay, staged.iter().copied()))
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
    pub fn meets(self, binding: OldBinding<'_>) -> bool {
        self.meets_sorted(binding.0.iter().map(|(unknown, _)| *unknown))
    }

    /// Whether `binding` binds some unknown here: whether it wakes the unit's record.
    pub fn meets_binding(self, binding: Binding<'_>) -> bool {
        self.meets_sorted(binding.0.iter().map(|(unknown, _)| *unknown))
    }

    /// Whether some unknown of the sorted run `bound` is here.
    fn meets_sorted(self, bound: impl Iterator<Item = Unknown>) -> bool {
        let (mut mine, mut bound) = (self.0.iter().peekable(), bound.peekable());
        while let (Some(unknown), Some(other)) = (mine.peek(), bound.peek()) {
            match (*unknown).cmp(other) {
                std::cmp::Ordering::Less => _ = mine.next(),
                std::cmp::Ordering::Greater => _ = bound.next(),
                std::cmp::Ordering::Equal => return true,
            }
        }
        false
    }

    /// This frontier with `other`'s unknowns, laid down through `lay` where it grows.
    pub fn union<'w>(self, other: Frontier<'_>, lay: impl Lays<'w>) -> Frontier<'w>
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
        Frontier(lay.fill(count, |_| each.next().expect("the merge yields its count")))
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

/// Where a fact's rule runs in its shape: a part, a node, a use or a type expression by its site;
/// a statement by its index; a binder, a type binder or a registration by its slot; and the
/// shape's own facts — a callable's type, its group levels, its summary — at the shape. A
/// fact a rule records beside its own — a settled check, an instance read, a call by name's
/// contributions, a view's tree — is a side fact, by its site, waiting on what its rule had read.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum FactKey {
    Site(Site),
    Statement(u32),
    Slot(Slot),
    Shape,
    Side(Site),
}

/// A shape's facts that wait on an unknown, each by its key beside its frontier, sorted by key:
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
pub struct OldBinding<'a>(&'a [(Unknown, KType)]);

impl<'a> OldBinding<'a> {
    /// The binding that binds nothing.
    pub const EMPTY: OldBinding<'static> = OldBinding(&[]);

    /// `staged`, sorted and laid down through `lay`. Each unknown is bound once.
    pub fn laid(lay: impl Lays<'a>, staged: &mut BumpVec<'_, (Unknown, KType)>) -> Self {
        staged.sort_unstable_by_key(|(unknown, _)| *unknown);
        debug_assert!(
            staged.windows(2).all(|pair| pair[0].0 != pair[1].0),
            "a binding binds each unknown once"
        );
        OldBinding(collect(lay, staged.iter().copied()))
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
    pub fn extends(self, other: OldBinding<'_>) -> bool {
        (other.0.iter()).all(|(unknown, bound)| self.bound(*unknown) == Some(*bound))
    }
}

/// One unit of one owning body: what the one-walk build keeps one record for.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct UnitId {
    pub segment: Segment,
    pub unit: UnitIndex,
}

/// What a [`Binding`] gives an unknown.
#[derive(Clone, Copy, Debug)]
pub enum Bound<'a> {
    /// A type-ish name: a `FOR ALL` name, a `:Type` parameter, a surfaced name, a type binder.
    Type(KType),
    /// A module binder: the closed content tree the load computes, and the exact type it carries.
    Module { carried: KType, tree: &'a Tree<'a> },
}

impl<'a> Bound<'a> {
    /// Whether the two give one meaning: one type, or one carried type over one content.
    pub fn same_as(&self, other: &Bound<'a>) -> bool {
        match (self, other) {
            (Bound::Type(mine), Bound::Type(theirs)) => mine == theirs,
            (
                Bound::Module { carried, tree },
                Bound::Module {
                    carried: other,
                    tree: theirs,
                },
            ) => carried == other && tree.same_as(theirs),
            _ => false,
        }
    }
}

/// Some unknowns, each beside what a binding gives it, sorted by unknown and each bound once.
#[derive(Clone, Copy, Debug, Default)]
pub struct Binding<'a>(&'a [(Unknown, Bound<'a>)]);

impl<'a> Binding<'a> {
    /// The binding that binds nothing.
    pub const EMPTY: Binding<'static> = Binding(&[]);

    /// `staged`, sorted and laid down through `lay`. Each unknown is bound once.
    pub fn laid(lay: impl Lays<'a>, staged: &mut BumpVec<'_, (Unknown, Bound<'a>)>) -> Self {
        staged.sort_unstable_by_key(|(unknown, _)| *unknown);
        debug_assert!(
            staged.windows(2).all(|pair| pair[0].0 != pair[1].0),
            "a binding binds each unknown once"
        );
        Binding(collect(lay, staged.iter().copied()))
    }

    /// What `unknown` is bound to, where it is bound.
    pub fn bound(self, unknown: Unknown) -> Option<Bound<'a>> {
        let at = self
            .0
            .binary_search_by_key(&unknown, |(held, _)| *held)
            .ok()?;
        Some(self.0[at].1)
    }

    pub fn entries(self) -> &'a [(Unknown, Bound<'a>)] {
        self.0
    }

    pub fn is_empty(self) -> bool {
        self.0.is_empty()
    }

    /// Whether this binding binds every unknown `other` binds, each to the same meaning.
    pub fn extends(self, other: Binding<'_>) -> bool {
        (other.0.iter()).all(|(unknown, theirs)| {
            self.bound(*unknown)
                .is_some_and(|mine| mine.same_as(theirs))
        })
    }
}

/// Where the one-walk build keeps one walk's records, and what it binds: the walk reads and writes
/// every unit's record through it, and never names where they rest
/// ([the store](../../roadmap/gradual-build/one-walk-build.md)). The load's store keeps the load's
/// records; a test's store layers a resumption's over them.
pub trait Store<'o> {
    /// Where every record a unit keeps is laid.
    type Lay: Lays<'o> + Copy;

    fn lay(&self) -> Self::Lay;

    /// What the binding gives `unknown`, where it binds it.
    fn binding(&self, unknown: Unknown) -> Option<Bound<'o>>;

    /// The newest record of `unit` beside its frontier: this walk's write, else a base layer's.
    fn fact(&self, unit: UnitId) -> Option<(&'o UnitFacts<'o>, Frontier<'o>)>;

    /// The newest closing record of the owning body `body`, beside its frontier.
    fn close(&self, body: Segment) -> Option<(&'o Closing<'o>, Frontier<'o>)>;

    /// Whether a record whose frontier is `frontier` re-runs under this store's binding.
    fn wakes(&self, frontier: Frontier<'_>) -> bool;

    fn write(&self, unit: UnitId, facts: &'o UnitFacts<'o>, frontier: Frontier<'o>);

    fn write_close(&self, body: Segment, closing: &'o Closing<'o>, frontier: Frontier<'o>);

    /// A refusal the walk reached: the store's chance to word it, before the walk returns it.
    fn refuse(&self, error: ShapeError<'o>) -> ShapeError<'o>;
}

/// Where in a body a seam lies: at its entry, or at a unit, by index into the body's units — the
/// order a body binds in, entry first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum SeamAt {
    Entry,
    Unit(u32),
}

/// A binding site at which some frontier's last member binds, beside every unknown the frontiers
/// completed there wait on: where a resumption over the binding the seam takes lands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Seam<'a> {
    pub at: SeamAt,
    pub waits_on: Frontier<'a>,
}
