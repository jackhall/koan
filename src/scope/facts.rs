//! The vocabulary of the load's facts: what a unit waits on, and the binding that wakes it
//! ([the build module's design](../build/README.md#type-facts-and-the-solver)).
//!
//! The load keys its facts by unit ([`UnitId`]): one record per unit, beside its **frontier**, the
//! [`Unknown`]s that could change it, read transitively, kept in a [`Store`]. A unit with an empty
//! frontier is fixed. A [`Binding`] gives each unknown a [`Bound`] — a closed type, or for a module
//! binder a closed content tree — and **wakes** every unit whose frontier meets it.
//!
//! An unknown is named by its **lexical address** — the segment of the body that declares it and
//! the slot or capture its activation holds it at — never by its symbol or its lattice node, since
//! every body numbering its chain alike shares one node per level. `scope` holds the vocabulary,
//! since `elaborate` and `dispatch` both name it and neither names `build`; the walk computes it,
//! and `build` folds the keys it decides.

use crate::memory::{BumpVec, Lays, collect};
use crate::type_lattice::KType;

use super::records::{Closing, UnitFacts};
use super::shape::{CaptureSlot, Segment, Slot, UnitIndex};
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

/// What a fact waits on: the unknowns its unit gathered itself, beside each ended frontier it
/// read, cut to the reader's chain. Its unknowns — every unknown that could change the fact,
/// transitively — are its own and each read's past its cut; they are never laid flat, so a read
/// costs one entry however many unknowns it holds. Its unknowns, flattened, are declared by its
/// record's body or a body enclosing it: the walk gathers only what its chain declares, and cuts
/// each read to it. Empty for a fixed fact. Only the tests lay its unknowns flat
/// (`Frontier::unknowns`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Frontier<'a> {
    own: &'a [Unknown],
    reads: &'a [Cut<'a>],
}

/// An ended frontier a unit read, cut to the reader's chain: of its unknowns, only those declared
/// in `from` or a body enclosing it count. That set is exactly the unknowns whose segment is at
/// least `from`, since three things hold: a frontier's unknowns lie on its record's chain
/// ([`Frontier`]), `from` lies on that chain too, and segments fall along a chain ([`Segment`]).
/// Segments compare within one shape table alone, and no frontier spans two.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cut<'a> {
    pub frontier: Frontier<'a>,
    pub from: Segment,
}

impl<'a> Frontier<'a> {
    /// The frontier of a fixed fact.
    pub const EMPTY: Frontier<'static> = Frontier {
        own: &[],
        reads: &[],
    };

    /// `unknowns`, already sorted and deduplicated, read from no other frontier.
    pub fn from_sorted(unknowns: &'a [Unknown]) -> Self {
        debug_assert!(unknowns.is_sorted() && unknowns.windows(2).all(|pair| pair[0] != pair[1]));
        Frontier {
            own: unknowns,
            reads: &[],
        }
    }

    /// `own` sorted and deduplicated, beside `reads` each read once at its widest cut, laid down
    /// through `lay`. A read of an empty frontier is dropped.
    pub fn laid(
        lay: impl Lays<'a>,
        own: &mut BumpVec<'_, Unknown>,
        reads: &mut BumpVec<'_, Cut<'a>>,
    ) -> Self {
        own.sort_unstable();
        own.dedup();
        reads.retain(|cut| !cut.frontier.is_empty());
        // Sorted by cut within one frontier, the first of each run is its widest.
        reads.sort_unstable_by_key(|cut| (cut.frontier.identity(), cut.from));
        reads.dedup_by_key(|cut| cut.frontier.identity());
        // An empty slice is never laid, so it shares no address with a frontier's laid one.
        Frontier {
            own: match own.is_empty() {
                true => &[],
                false => collect(lay, own.iter().copied()),
            },
            reads: match reads.is_empty() {
                true => &[],
                false => collect(lay, reads.iter().copied()),
            },
        }
    }

    /// The unknowns its unit gathered itself, sorted and deduplicated.
    pub fn own(self) -> &'a [Unknown] {
        self.own
    }

    /// Each ended frontier its unit read, at the cut it read it.
    pub fn reads(self) -> &'a [Cut<'a>] {
        self.reads
    }

    /// Whether it holds nothing: its unit gathered no unknown and read no frontier.
    pub fn is_empty(self) -> bool {
        self.own.is_empty() && self.reads.is_empty()
    }

    /// Which frontier this is, by where it is laid: a laid frontier lays each slice that holds
    /// something afresh, so two distinct frontiers that hold something never share both.
    pub(crate) fn identity(self) -> (usize, usize) {
        (self.own.as_ptr().addr(), self.reads.as_ptr().addr())
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

/// Where one walk keeps its records, and what it binds: the walk reads and writes
/// every unit's record through it, and never names where they rest
/// ([the store](../build/README.md#the-store)). The load's store keeps the load's
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
}
