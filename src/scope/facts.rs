//! The vocabulary of the load's facts: what a unit waits on, and what a frame's binding gives it
//! ([the build module's design](../build/README.md#type-facts-and-the-solver)).
//!
//! The load keys its facts by unit ([`UnitId`]): one record per unit, beside its **frontier** — the
//! [`Unknown`]s it read itself and the frontiers it read, together every unknown that could change
//! it, transitively — kept in a [`Store`]. A unit with an empty frontier is fixed. A [`Binding`]
//! gives each unknown a [`Bound`] — a closed type, or for a module binder a closed content tree:
//! what a frame reads a unit's facts under ([frontiers and seams](../build/README.md#frontiers-and-seams)).
//!
//! An unknown is named by its **lexical address** — the segment of the body that declares it and
//! the slot or capture its activation holds it at — never by its symbol or its lattice node, since
//! every body numbering its chain alike shares one node per level. `scope` holds the vocabulary,
//! since `elaborate` and `dispatch` both name it and neither names `build`; the walk computes it,
//! and `build` folds what a frame's entry and seams bind.

use crate::memory::{BumpAllocator, BumpBackedMap, BumpVec, Lays, bump_table, collect};
use crate::type_lattice::{DeclaredType, KType, Parametric, TypeDigest};
use crate::values::Chain;

use super::derived::{DerivedName, DerivedTerm};
use super::records::{Closing, UnitFacts, UnitRecord};
use super::shape::{CaptureSlot, Segment, Site, Slot, UnitIndex};
use super::tree::Tree;

/// Where a body holds an unknown: an activation's slot, or — for a name a quote's code is run
/// with — a capture, or, for a value whose type the text does not fix written where no `LET`
/// holds it, the site of the expression holding it, for its unit.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Address {
    Slot(Slot),
    Capture(CaptureSlot),
    Held(Site),
}

/// A name a binding may remove, by its lexical address: the segment of the body declaring it, and
/// where that body's activation holds it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Unknown {
    pub segment: Segment,
    pub at: Address,
}

/// The judgement a door left undecided over a variable, which the question laid beside the unit
/// record decides under a binding ([stored questions](../build/README.md#stored-questions)). One
/// per judgement door.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum OpenKind {
    /// A verdict *maybe*.
    Verdict,
    NeverSatisfies,
    Fits,
    LowerEndOutside,
    /// A selection's question: what a candidate or an arm does at its call, or one bit of a use's
    /// ranking.
    Settles,
    /// A solve the collector reports not reproducible at the run.
    Solve,
    Instance,
    /// Whether a partial rule over a run-bound name has an answer: a spelling over a head — a
    /// member read, an application, a pin, a code kind — a variant read, or a type naming fields.
    Spells,
    /// A retype not the point at its declared type.
    Retype,
    Unequal,
    /// Whether one type lies under another in the order.
    Under,
    Overlap,
    /// The kind of the type above a variable, or of a type name's own seed.
    Kind,
    /// Whether a binder declared at a variable is a module binder.
    ModuleBinder,
}

impl OpenKind {
    /// How many kinds there are: the length of [`ALL`](Self::ALL).
    #[cfg(test)]
    pub const COUNT: usize = OpenKind::ModuleBinder as usize + 1;

    /// Every kind, in discriminant order: what the tests tally marks by.
    #[cfg(test)]
    pub const ALL: [OpenKind; OpenKind::COUNT] = [
        OpenKind::Verdict,
        OpenKind::NeverSatisfies,
        OpenKind::Fits,
        OpenKind::LowerEndOutside,
        OpenKind::Settles,
        OpenKind::Solve,
        OpenKind::Instance,
        OpenKind::Spells,
        OpenKind::Retype,
        OpenKind::Unequal,
        OpenKind::Under,
        OpenKind::Overlap,
        OpenKind::Kind,
        OpenKind::ModuleBinder,
    ];
}

/// An open mark: the judgement left undecided, the slot type it was judged against (`None` where
/// there is none) and the type it judged.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct OpenMark {
    pub kind: OpenKind,
    pub slot: Option<DeclaredType<Parametric>>,
    pub judged: DeclaredType<Parametric>,
}

impl OpenMark {
    /// The mark of `kind` over `judged` against `slot`, undecided.
    pub fn undecided(
        kind: OpenKind,
        slot: Option<DeclaredType<Parametric>>,
        judged: DeclaredType<Parametric>,
    ) -> Self {
        OpenMark { kind, slot, judged }
    }
}

/// Why a unit waits on an unknown: its unit record only names it, or a rule judged it and reached no
/// answer that holds at every binding within its ends.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mark {
    Named,
    Open(OpenMark),
}

/// The order a frontier sorts the marks on one unknown by.
type MarkKey = (u8, OpenKind, Option<TypeDigest>, Option<TypeDigest>);

impl Mark {
    /// The order a frontier sorts the marks on one unknown by: `Named` lowest.
    fn key(self) -> MarkKey {
        match self {
            Mark::Named => (0, OpenKind::Verdict, None, None),
            Mark::Open(open) => (
                1,
                open.kind,
                open.slot.map(DeclaredType::digest),
                Some(open.judged.digest()),
            ),
        }
    }
}

/// One unknown a unit gathered, beside why it waits on it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Noted {
    pub unknown: Unknown,
    pub mark: Mark,
}

impl Noted {
    /// `unknown`, only named.
    pub fn named(unknown: Unknown) -> Self {
        Noted {
            unknown,
            mark: Mark::Named,
        }
    }

    pub fn is_open(self) -> bool {
        matches!(self.mark, Mark::Open(_))
    }

    /// The order a frontier sorts its marks by: by unknown, then mark.
    fn order(&self, other: &Self) -> std::cmp::Ordering {
        let key = |noted: &Self| (noted.unknown, noted.mark.key());
        key(self).cmp(&key(other))
    }
}

/// `noted` sorted by unknown, then mark, each held once, and a `Named` dropped where the same
/// unknown holds an `Open`: what a frontier keeps of its own.
pub fn settle_marks(noted: &mut BumpVec<'_, Noted>) {
    noted.sort_unstable_by(Noted::order);
    noted.dedup();
    let mut kept = 0;
    for at in 0..noted.len() {
        let shadowed = !noted[at].is_open()
            && (noted.get(at + 1)).is_some_and(|next| next.unknown == noted[at].unknown);
        if !shadowed {
            noted[kept] = noted[at];
            kept += 1;
        }
    }
    noted.truncate(kept);
}

/// What a fact waits on: the unknowns its unit gathered itself, beside each ended frontier it
/// read, cut to the reader's chain. Its unknowns — every unknown that could change the fact,
/// transitively — are its own and each read's past its cut; they are never laid flat, so a read
/// costs one entry however many unknowns it holds. Its unknowns, flattened, are declared by its
/// record's body or a body enclosing it: the walk gathers only what its chain declares, and cuts
/// each read to it. Empty for a fixed fact. Only the tests lay its unknowns flat
/// (`Frontier::unknowns`).
///
/// It has no equality: a shared frontier is the same one only by `identity`, and comparing
/// content would recurse through its reads. It prints each read by its cut and identity alone,
/// since a chain of k reads is k deep.
#[derive(Clone, Copy)]
pub struct Frontier<'a> {
    own: &'a [Noted],
    reads: &'a [Cut<'a>],
    /// The latest segment among its unknowns, where it holds some.
    top: Segment,
}

/// An ended frontier a unit read, cut to the reader's chain: of its unknowns, only those declared
/// in `from` or a body enclosing it count. That set is exactly the unknowns whose segment is at
/// least `from`, since three things hold: a frontier's unknowns lie on its record's chain
/// ([`Frontier`]), `from` lies on that chain too, and segments fall along a chain ([`Segment`]).
/// Segments compare within one shape table alone, and no frontier spans two.
#[derive(Clone, Copy)]
pub struct Cut<'a> {
    pub frontier: Frontier<'a>,
    pub from: Segment,
}

impl Cut<'_> {
    /// Whether the cut keeps some unknown of its frontier.
    pub fn keeps_any(self) -> bool {
        self.frontier.top().is_some_and(|top| top >= self.from)
    }
}

impl std::fmt::Debug for Frontier<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (f.debug_struct("Frontier"))
            .field("own", &self.own)
            .field("reads", &self.reads)
            .finish()
    }
}

impl std::fmt::Debug for Cut<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (f.debug_struct("Cut"))
            .field("from", &self.from)
            .field("laid at", &self.frontier.identity())
            .finish()
    }
}

impl Default for Frontier<'_> {
    fn default() -> Self {
        Frontier::EMPTY
    }
}

impl<'a> Frontier<'a> {
    /// The frontier of a fixed fact.
    pub const EMPTY: Frontier<'static> = Frontier {
        own: &[],
        reads: &[],
        top: Segment(0),
    };

    /// `noted`, already settled ([`settle_marks`]), read from no other frontier.
    pub fn from_sorted(noted: &'a [Noted]) -> Self {
        debug_assert!(
            noted.is_sorted_by(|a, b| a.order(b).is_le())
                && noted.windows(2).all(|pair| pair[0] != pair[1])
        );
        Frontier {
            own: noted,
            reads: &[],
            top: noted
                .last()
                .map_or(Segment(0), |noted| noted.unknown.segment),
        }
    }

    /// `own` sorted and deduplicated, beside each frontier of `reads` read once at its widest cut,
    /// in the order first read, laid down through `lay`. A read whose cut keeps none of its
    /// unknowns is dropped, so a frontier is empty exactly where it holds no unknown.
    pub fn laid(lay: impl Lays<'a>, own: &mut BumpVec<'_, Noted>, reads: &[Cut<'a>]) -> Self {
        settle_marks(own);
        let mut ordered = BumpVec::with_capacity_in(reads.len(), *own.allocator());
        ordered.extend(
            reads
                .iter()
                .copied()
                .enumerate()
                .filter(|(_, cut)| cut.keeps_any()),
        );
        // Sorted by cut within one frontier, the first of each run is its widest; it keeps the
        // run's earliest read for its order.
        ordered.sort_unstable_by_key(|(_, cut)| (cut.frontier.identity(), cut.from));
        ordered.dedup_by(|(later, cut), (kept, widest)| {
            let same = cut.frontier.identity() == widest.frontier.identity();
            if same {
                *kept = (*kept).min(*later);
            }
            same
        });
        ordered.sort_unstable_by_key(|(read, _)| *read);
        let top = (own.last().map(|noted| noted.unknown.segment).into_iter())
            .chain(ordered.iter().map(|(_, cut)| cut.frontier.top))
            .max()
            .unwrap_or(Segment(0));
        // An empty slice is never laid, so it shares no address with a frontier's laid one.
        Frontier {
            own: match own.is_empty() {
                true => &[],
                false => collect(lay, own.iter().copied()),
            },
            reads: match ordered.is_empty() {
                true => &[],
                false => collect(lay, ordered.iter().map(|(_, cut)| *cut)),
            },
            top,
        }
    }

    /// The unknowns its unit gathered itself, each beside why, sorted by unknown then mark.
    pub fn own(self) -> &'a [Noted] {
        self.own
    }

    /// The unknowns its unit gathered itself, sorted and each once.
    pub fn own_unknowns(self) -> impl Iterator<Item = Unknown> + 'a {
        let own = self.own;
        (own.iter().enumerate())
            .filter(move |(at, noted)| *at == 0 || own[at - 1].unknown != noted.unknown)
            .map(|(_, noted)| noted.unknown)
    }

    /// Hand `each` every type its own open marks name: a read frontier is visited where it rests.
    pub fn each_type(self, each: &mut dyn FnMut(DeclaredType<Parametric>)) {
        for noted in self.own {
            if let Mark::Open(open) = noted.mark {
                open.slot.into_iter().for_each(&mut *each);
                each(open.judged);
            }
        }
    }

    /// Each ended frontier its unit read, at the cut it read it.
    pub fn reads(self) -> &'a [Cut<'a>] {
        self.reads
    }

    /// Whether it holds no unknown: its unit gathered none, and read no frontier its cut keeps
    /// one of.
    pub fn is_empty(self) -> bool {
        self.own.is_empty() && self.reads.is_empty()
    }

    /// The latest segment among its unknowns: `None` where it holds none.
    pub fn top(self) -> Option<Segment> {
        (!self.is_empty()).then_some(self.top)
    }

    /// Which frontier this is, by where it is laid: a laid frontier lays each slice that holds
    /// something afresh, so two distinct frontiers that hold something never share both.
    pub(crate) fn identity(self) -> (usize, usize) {
        (self.own.as_ptr().addr(), self.reads.as_ptr().addr())
    }
}

/// Each frontier's marks laid flat — its own and each read's past its cut, transitively, settled
/// by [`settle_marks`] — memoized by identity in `scratch`: what the load's folds read, each
/// frontier flattened once.
pub(crate) struct Flattened<'x> {
    scratch: BumpAllocator<'x>,
    flat: BumpBackedMap<'x, (usize, usize), &'x [Noted]>,
}

impl<'x> Flattened<'x> {
    /// A flattener memoizing in `scratch`.
    pub(crate) fn new(scratch: BumpAllocator<'x>) -> Self {
        Flattened {
            scratch,
            flat: bump_table(scratch),
        }
    }

    /// `frontier`'s marks laid flat: a frontier met before, as itself or as a read, is not walked
    /// again.
    pub(crate) fn of(&mut self, frontier: Frontier<'_>) -> &'x [Noted] {
        // A post-order over the reads: each frontier laid once its reads are.
        let mut stack = BumpVec::new_in(self.scratch);
        stack.push((frontier, false));
        while let Some((at, expanded)) = stack.pop() {
            if self.flat.contains_key(&at.identity()) {
                continue;
            }
            if !expanded {
                stack.push((at, true));
                stack.extend((at.reads().iter()).map(|cut| (cut.frontier, false)));
                continue;
            }
            let mut noted = BumpVec::new_in(self.scratch);
            noted.extend(at.own().iter().copied());
            for cut in at.reads() {
                let read = self.flat[&cut.frontier.identity()];
                noted.extend((read.iter()).filter(|noted| noted.unknown.segment >= cut.from));
            }
            settle_marks(&mut noted);
            let laid = collect(self.scratch, noted.iter().copied());
            self.flat.insert(at.identity(), laid);
        }
        self.flat[&frontier.identity()]
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
    /// A value binder declared `Any`: the exact type its value carries, which has no content.
    Carried(KType),
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
            (Bound::Carried(mine), Bound::Carried(theirs)) => mine == theirs,
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
/// records, and a composition's.
pub trait Store<'o> {
    /// Where every record a unit keeps is laid.
    type Lay: Lays<'o> + Copy;

    fn lay(&self) -> Self::Lay;

    /// The record of `unit` beside its frontier, where this walk wrote it.
    fn fact(&self, unit: UnitId) -> Option<(&'o UnitFacts<'o>, Frontier<'o>)>;

    /// The newest closing record of the owning body `body`, beside its frontier.
    fn close(&self, body: Segment) -> Option<(&'o Closing<'o>, Frontier<'o>)>;

    /// The derived name for `term`: the one this store's walk laid, else laid now in this store's
    /// own table, named through its
    /// [`Spellings`](super::Spellings). Every walk names a term alike.
    fn derived(&self, types: &Chain<'_, '_, '_, '_>, term: DerivedTerm<'_>) -> DerivedName<'_>;

    /// The derived name `id`, where this store's table holds it.
    fn derived_name(&self, id: crate::type_lattice::DerivedId) -> Option<DerivedName<'_>>;

    /// Write this walk's record of `unit`: its facts, or the mark that the walk did not reach it.
    fn write(&self, unit: UnitId, record: UnitRecord<'o>);

    fn write_close(&self, body: Segment, closing: &'o Closing<'o>, frontier: Frontier<'o>);
}
