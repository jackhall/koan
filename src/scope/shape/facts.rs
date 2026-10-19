//! The vocabulary of the solver's type facts: what a load-time fact waits on, and the binding
//! that wakes it ([the build module's design](../../build/README.md#type-facts-and-the-solver)).
//!
//! A **fact** is one write-once load-time cell — a type channel cell or a value channel entry —
//! and the rule that computes it runs at one [`FactKey`] of one shape. Every fact a rule run
//! writes at its key shares one [`Frontier`]: the [`Unknown`]s that could change it, read
//! transitively. A fact with an empty frontier is **fixed**; the others, each beside its frontier,
//! are the shape's [`Residual`]. A [`Binding`] gives some unknowns closed types, and **wakes**
//! every fact whose frontier meets it.
//!
//! An unknown is named by its **lexical address** — the segment of the body that declares it and
//! the slot or capture its activation holds it at — never by its symbol or its lattice node, since
//! every body numbering its chain alike shares one node per level. `scope` holds the vocabulary,
//! since `elaborate` and `dispatch` both name it and neither names `build`; the passes compute it,
//! and `build` lists the seams it decides.

use crate::memory::{BumpVec, Lays, collect};
use crate::type_lattice::KType;

use super::super::keyed::Keyed;
use super::{CaptureSlot, Segment, Site, Slot};

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

    /// `staged`, sorted, deduplicated and laid down through `writer`.
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

    /// This frontier with `other`'s unknowns, laid down through `writer` where it grows.
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
pub struct Binding<'a>(&'a [(Unknown, KType)]);

impl<'a> Binding<'a> {
    /// The binding that binds nothing.
    pub const EMPTY: Binding<'static> = Binding(&[]);

    /// `staged`, sorted and laid down through `writer`. Each unknown is bound once.
    pub fn laid(lay: impl Lays<'a>, staged: &mut BumpVec<'_, (Unknown, KType)>) -> Self {
        staged.sort_unstable_by_key(|(unknown, _)| *unknown);
        debug_assert!(
            staged.windows(2).all(|pair| pair[0].0 != pair[1].0),
            "a binding binds each unknown once"
        );
        Binding(collect(lay, staged.iter().copied()))
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
