//! The **load's facts**, read through one set of readers over a source of records: [`LoadFacts`]
//! over one load's own, and [`Facts`], what a step reads through its door, over a cell's layers and
//! the load they rest on. Every reader takes the body's shape, so no reader names where a fact is
//! kept. A shape names its body by its segment alone, so a reader may hand in a shape
//! borrowed for any lifetime. A reader whose fact names a type takes the reading frame's chain
//! before the shape and hands back the fact with every type it names resolving there, minted from
//! its composition's term where the chain lacks it, so no read can skip the mint.
//!
//! It reads the one walk's records: one [`UnitFacts`] per unit the walk ran, each beside the
//! frontier it gathered, one [`Closing`] per owning body, and the [`Folds`] the load lays per body
//! after the walk. Every record is `Copy`, names bodies by segment and holds no cell, so each is
//! covariant. A load may sit over a base: every read tries its own records, then its base's, and a
//! unit it re-ran replaces the base's record of it whole. The records index every closed tree their
//! units computed by its identity, read through [`LoadFacts::tree_of`].
//!
//! A `LoadFacts` is laid in program storage before the program is. A step reads it at its own
//! lifetime through the [`Facts`] its door builds, so nothing a cell parks points into it; a value
//! born of written code keeps what it holds of its body through [`Written`], since the load is
//! never evicted.

use crate::memory::{BumpAllocator, BumpVec, Lays, Writer, collect, resident};
use crate::parse::{ExpressionPart, KExpression, copied};
use crate::source::SourceRef;
use crate::symbols::{BinderSymbol, Symbol};
use crate::type_lattice::{DeclaredType, Interval, KType, Parametric, PathClass, Term, TypeDigest};
use crate::values::{Chain, ContentDigest, LayerKey, Restamping, Retype, Weight};

use super::build_layer::BuildLayer;
use super::composition::Composition;
use super::derived::{Derived, DerivedName, DerivedTerm};
use super::facts::{Address, Binding, Bound, Frontier, UnitId, Unknown};
use super::keyed::Keyed;
use super::record_types::{Close, CloseLaying, Closed, Closer, Laying, Named};
use super::shape::{
    BodyShape, CaptureSlot, Coordinate, Lands, Located, Position, Segment, ShapeError, Site, Slot,
    Target,
};
use super::shapes::Shapes;
use super::stamp::SiteRequest;
use super::tree::{Identity, Leaf, NodeCapture, Tree};
use super::typed::{
    Arms, BodyClass, Caching, Elaboration, Handoff, InstanceRead, Mint, PathEntry, PathName, Pick,
    Points, Resolution, Resolved, Route, Static, StaticCallable, StaticInstance, StaticRegistered,
    StaticType, UseStatics, Variable, ViewSlots,
};

/// What a cell reads of a typed fact where it runs ([`Facts::at_run`]).
#[derive(Clone, Copy, Debug)]
pub enum AtRun<T> {
    /// The same at every run.
    Closed(T),
    /// Left to the run's own door: a contribution read off the argument's carried type, or a
    /// view's type over a tree the binding leaves open.
    Open,
    /// Refused at every run, as the load words it.
    Refused(Elaboration),
}

/// A walk's record of one unit: its facts beside the frontier it gathered, or the mark that the
/// walk never reached it, as a quote's code refused before it. A mark hides every older record of
/// the unit, so a reader finds none, as one does where no walk reached the unit.
#[derive(Clone, Copy)]
pub enum UnitRecord<'a> {
    Typed(&'a UnitFacts<'a>, Frontier<'a>),
    Unreached,
}

impl<'a> UnitRecord<'a> {
    /// Its facts beside its frontier, where the walk reached the unit.
    pub fn typed(self) -> Option<(&'a UnitFacts<'a>, Frontier<'a>)> {
        match self {
            UnitRecord::Typed(facts, frontier) => Some((facts, frontier)),
            UnitRecord::Unreached => None,
        }
    }
}

/// One unit's facts: everything the walk computed running it.
#[derive(Clone, Copy)]
pub struct UnitFacts<'a> {
    /// The unit's own body's, and the signature of each module body it ran, by segment.
    pub typed: Keyed<'a, (Segment, Typed<'a>)>,
    /// Each callable or quote's code born here, by its segment.
    pub born: Keyed<'a, (Segment, Born<'a>)>,
    /// Each path entry this unit resolved first on its chain, by its chain root and name, beside
    /// its resolution.
    pub paths: Keyed<'a, ((Segment, PathName), (PathEntry<'a>, Resolution<'a>))>,
    /// What the caching fold reads of each minting and composite site the unit typed.
    pub sites: &'a [SiteDeps<'a>],
    /// Each lexical variable a contribution or a solution names, beside the body reading it: what
    /// the contract fold reads.
    pub named_variables: &'a [NamedVariable],
    /// An entry unit's frontier of each binder it seeds, by slot: a read of a binder's static type
    /// gathers its seed alone, and the unit's own frontier reads every seed.
    pub seeds: Keyed<'a, (Slot, Frontier<'a>)>,
}

/// Whether `key` reads a path class or the type a `:Module` binder carries, itself or through a
/// derived name's term (`names` finds each): a substitution by levels closes neither, so a
/// binding that keeps every question walks a layer whose keys read one.
pub fn reads_module_binders<'n>(
    types: &crate::values::Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    key: DeclaredType<Parametric>,
    names: &dyn Fn(crate::type_lattice::DerivedId) -> Option<DerivedName<'n>>,
) -> bool {
    use crate::type_lattice::Variable;
    (super::free_variables(types, scratch, key).iter()).any(|variable| match variable {
        Variable::Path { .. } => true,
        Variable::Derived { term, .. } => names(*term).is_some_and(|name| match name.term {
            DerivedTerm::Carried { .. } => true,
            term => {
                let mut reads = false;
                term.each_type(&mut |operand| {
                    reads = reads || reads_module_binders(types, scratch, operand, names);
                });
                reads
            }
        }),
        _ => false,
    })
}

/// One body's facts from one unit, each table sorted by its key: the type channel's, then the
/// value channel's, then what an inline body folds into its owner's closing.
#[derive(Clone, Copy, Debug)]
pub struct Typed<'a> {
    /// Each type binder's declaration, by slot.
    pub declared: Keyed<'a, (Slot, StaticType<'a>)>,
    /// Each registration's entry, by slot.
    pub registered: Keyed<'a, (Slot, StaticRegistered<'a>)>,
    /// Each recorded type expression's type, by site.
    pub expressions: Keyed<'a, (Site, StaticType<'a>)>,
    /// Each part's static type, by site.
    pub parts: Keyed<'a, (Site, Interval<Parametric>)>,
    /// Each statement's static type, by index into the body.
    pub statements: Keyed<'a, (u32, Interval<Parametric>)>,
    /// Each binder's static type, by slot.
    pub binders: Keyed<'a, (Slot, DeclaredType<Interval<Parametric>>)>,
    /// Each ascription the walk settled.
    pub settled: Keyed<'a, Site>,
    pub instances: Keyed<'a, (Site, InstanceRead<'a>)>,
    pub uses: Keyed<'a, (Site, UseStatics<'a>)>,
    /// Each selected use's closed points, by group index.
    pub selections: Keyed<'a, (Site, Points<'a>)>,
    /// Each selected use of a callable body, beside what the site fixes of its callee's names.
    pub traced: Keyed<'a, (Site, Traced<'a>)>,
    /// Each call by name's contributions, by its argument part's site.
    pub by_name: Keyed<'a, (Site, &'a [StaticType<'a>])>,
    /// Each tail call's route, by its node's site.
    pub routes: Keyed<'a, (Site, Route<'a>)>,
    /// Each `MATCH`'s arms, by its node's site.
    pub arms: Keyed<'a, (Site, Arms<'a>)>,
    /// Each retype to a closed declared type, by the site its check is settled at, and each module
    /// member read's, by its node's site.
    pub retypes: Keyed<'a, (Site, Restamp)>,
    /// Each `:|` at a closed signature, beside its view's content tree.
    pub trees: Keyed<'a, (Site, &'a Tree<'a>)>,
    /// Each `:|` over a body-born module whose content is closed, beside its keyworded members'
    /// picks.
    pub picks: Keyed<'a, (Site, &'a [Pick<'a>])>,
    /// Each module binder's content tree, by slot, where the load read one.
    pub contents: Keyed<'a, (Slot, Tree<'a>)>,
    /// The identity of each module each of those binders holds, its own and each one a member of
    /// a module body holds, by slot: what a class's variable is read through a binder by.
    pub holding: Keyed<'a, (Slot, &'a [Identity])>,
    /// The body's own minting sites.
    pub minting: Keyed<'a, Site>,
    /// A `MODULE` or `GROUP` body's exact signature, written over names, where the walk typed one.
    pub signature: Option<Parametric>,
    /// What the unit's facts give the body's class, folded back in where a resumption absorbs
    /// the unit: a porous exit, how many of its minting sites are views over closed trees, and
    /// the run mints of the blocks and module bodies it ran.
    pub exits: bool,
    pub closed: u32,
    pub folded: u32,
}

/// A call the walk resolved to one callable body — a keyworded use it selected the body at, or a
/// call by name whose head's tree is a closure of it — beside what the site passes at its callee's
/// slots: what the trace folds, and what the load reads to lay a layer for the call where that
/// fixes its key.
#[derive(Clone, Copy, Debug)]
pub struct Traced<'a> {
    /// The callee's body.
    pub callee: Segment,
    /// Where the call is written.
    pub call: SourceRef,
    /// Whether the call is by name, kept at its head's site.
    pub by_name: bool,
    /// What the site passes at each callee slot it passes anything the trace follows at, sorted
    /// by slot.
    pub fixed: &'a [(Slot, Fixed)],
}

/// Load content at one of a callee's slots: what a layer binds it to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Content {
    /// A `FOR ALL` name's closed point, or a closed type value: its handle in the root.
    Type(KType),
    /// A module whose content tree is closed. `content` is its knot member's digest, which
    /// indexes its tree: a retype's bare member, a view itself. `at` is the type the argument
    /// carries into the call, `own` the member's type and `declared` the parameter's closed
    /// declared type.
    Module {
        content: ContentDigest,
        at: KType,
        own: KType,
        declared: KType,
    },
}

impl Content {
    /// Hand `each` every type this names.
    pub fn each_type(&self, each: &mut dyn FnMut(KType)) {
        match *self {
            Content::Type(kt) => each(kt),
            Content::Module {
                at, own, declared, ..
            } => {
                each(at);
                each(own);
                each(declared);
            }
        }
    }
}

/// What a call site passes at one of its callee's slots.
#[derive(Clone, Copy, Debug)]
pub enum Fixed {
    /// Load content.
    Content(Content),
    /// The caller's own slot, passed on bare.
    Passed(Slot),
}

impl Fixed {
    /// Hand `each` every type this names.
    pub fn each_type(&self, each: &mut dyn FnMut(KType)) {
        if let Fixed::Content(content) = self {
            content.each_type(each);
        }
    }
}

/// What the [trace](../build/README.md#the-trace) names at one call site of a keyed body.
#[derive(Clone, Copy, Debug)]
pub enum CallTrace<'a> {
    /// Every binding the site passes: per binding, the content at each of the callee's keys, in
    /// key order, sorted.
    Named {
        callee: Segment,
        bindings: &'a [&'a [Content]],
    },
    Untraced(Untraced),
}

/// Why the trace names nothing at a call site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Untraced {
    /// The load resolves the site to no one callee.
    Unresolved,
    /// A keyed slot is passed nothing the trace follows.
    Unfollowed,
    /// The site lies in a quote's code, which composed code can re-make with run content.
    Composed,
    /// A keyed slot passes on a slot of the site's own body, whose bindings are open.
    Caller(Open),
}

/// Why a body's bindings are open: the trace names none of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Open {
    /// Its name escapes, so the load cannot list its callers.
    Escapes,
    /// A call into it is untraced, or passes on a slot of an open caller.
    Fed,
    /// It has more bindings than the limit.
    Capped,
}

impl Typed<'_> {
    /// A body's facts where a unit fixed none.
    pub const EMPTY: Self = Typed {
        declared: Keyed::EMPTY,
        registered: Keyed::EMPTY,
        expressions: Keyed::EMPTY,
        parts: Keyed::EMPTY,
        statements: Keyed::EMPTY,
        binders: Keyed::EMPTY,
        settled: Keyed::EMPTY,
        instances: Keyed::EMPTY,
        uses: Keyed::EMPTY,
        selections: Keyed::EMPTY,
        traced: Keyed::EMPTY,
        by_name: Keyed::EMPTY,
        routes: Keyed::EMPTY,
        arms: Keyed::EMPTY,
        retypes: Keyed::EMPTY,
        trees: Keyed::EMPTY,
        picks: Keyed::EMPTY,
        contents: Keyed::EMPTY,
        holding: Keyed::EMPTY,
        minting: Keyed::EMPTY,
        signature: None,
        exits: false,
        closed: 0,
        folded: 0,
    };
}

impl UnitFacts<'_> {
    /// A unit's record where it fixed nothing.
    pub const EMPTY: Self = UnitFacts {
        typed: Keyed::EMPTY,
        born: Keyed::EMPTY,
        paths: Keyed::EMPTY,
        sites: &[],
        named_variables: &[],
        seeds: Keyed::EMPTY,
    };
}

/// A callable or a quote's code, as the unit that births it typed it. Its registration is its
/// callable type's, and the encloser's slot holds its entry.
#[derive(Clone, Copy, Debug)]
pub struct Born<'a> {
    pub callable: StaticCallable<'a>,
    /// Its own `FOR ALL` group as its body reads it, by group order.
    pub levels: &'a [Parametric],
    /// The solution its `FOR ALL` is born instantiated at, where the walk solved one.
    pub instance: Option<StaticInstance<'a>>,
    /// Its parameters whose arguments digest at a constant cost, sorted: a type binder, or one
    /// declared closed under `Module` or as a type value. The keys and caching folds read it.
    pub digestible: &'a [Slot],
}

/// What the walk computes for an owning body once its units and deferred bodies are typed.
#[derive(Clone, Copy, Debug)]
pub struct Closing<'a> {
    pub class: Option<BodyClass>,
    /// A callable's summary: `Some(None)` where it has none, a recursive one among them.
    pub summary: Option<Option<Tree<'a>>>,
    /// Whether the callable lies in a recursive component: it never unfolds.
    pub recursive: bool,
    /// A quote's code's last statement's static type as a traced `EVAL` runs it unfilled.
    pub ran: Option<Interval<Parametric>>,
    /// Why a quote's code does not type, reported when an `EVAL` runs it.
    pub refusal: Option<&'a ShapeError<'a>>,
    /// The first registration of a quote's code that overlaps a builtin overload, reported when an
    /// `EVAL` runs it. Apart from `refusal`, since `USING` keeps code that does not type.
    pub overlap: Option<&'a ShapeError<'a>>,
    /// What a callable's frame re-stamps its arguments and its value at.
    pub retypes: Retypes<'a>,
}

/// What a callable's frame re-stamps each argument and its value at, where the walk knew how: the
/// restamp to each value parameter's type, by name, and to its return.
#[derive(Clone, Copy, Debug)]
pub struct Retypes<'a> {
    pub parameters: &'a [(Symbol, Restamp)],
    pub returns: Option<Restamp>,
}

impl Retypes<'_> {
    /// A body whose frame re-stamps by no record.
    pub const NONE: Self = Retypes {
        parameters: &[],
        returns: None,
    };

    /// The retype to the parameter `name`'s type, where it is closed.
    pub fn parameter(&self, types: &Chain<'_, '_, '_, '_>, name: Symbol) -> Option<Retype> {
        (self.parameters.iter())
            .find(|(each, _)| *each == name)
            .and_then(|(_, restamp)| restamp.retype(types))
    }

    /// The retype to the return type, where it is closed.
    pub fn returned(&self, types: &Chain<'_, '_, '_, '_>) -> Option<Retype> {
        self.returns.and_then(|restamp| restamp.retype(types))
    }
}

/// How a retype to `at` re-stamps, laid where the walk knew how — `at` closed, or the witness's
/// answer over a rigid `at` — so a binding's table closes `at` as it closes any type a record
/// names, and the retype is read once it is closed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Restamp {
    pub how: Restamping,
    pub at: Parametric,
}

impl Restamp {
    /// The retype this restamp is, where `at` is closed.
    pub fn retype(&self, types: &Chain<'_, '_, '_, '_>) -> Option<Retype> {
        types.concrete(self.at).map(|at| self.how.at(at))
    }
}

/// What the load folds per body after the walk.
#[derive(Clone, Copy, Debug)]
pub struct Folds<'a> {
    pub caching: Option<&'a Caching<'a>>,
    /// The parameter slots a callable's settlement keys on: `None` where a call seeks nothing.
    pub keys: Option<&'a [Slot]>,
    /// A callable's or a block's parameters, or a quote's code's captures, that some unit beneath
    /// it waits on with its entry as its seam, keyable or not: what its cell's entry binds in its
    /// activation where nothing settled them.
    pub waits: &'a [Address],
    /// A body's slots bound at a statement that some unit's frontier holds: where the cell running
    /// it decides again once the statement binds them.
    pub seams: &'a [Slot],
    /// The seams some path the load left open is rooted at, sorted: decided at their statement
    /// even where the load built their content, which a path reads off what the run binds.
    pub rooting: &'a [Slot],
    /// Which of the body's type captures its content composes, parallel to its layout.
    pub contract: &'a [bool],
    /// What the trace names at each call site of a keyed body in this shape, by site.
    pub traces: Keyed<'a, (Site, CallTrace<'a>)>,
    /// On an owning body — a callable, or the program — each call site it owns that covers its
    /// callee's keys, sorted by shape and site.
    pub slot_maps: &'a [SlotMap<'a>],
}

/// One call site of a keyed callee whose record covers every key of the callee, outside a quote's
/// code: what a layer of the site's owning body lays the site's request from.
#[derive(Clone, Copy, Debug)]
pub struct SlotMap<'a> {
    /// The shape the site lies in — the owning body or an inline body it runs — and the site.
    pub at: Segment,
    pub site: Site,
    /// Where the call is written.
    pub call: SourceRef,
    pub callee: Segment,
    /// What the site passes at each of the callee's keys, in key order.
    pub passed: &'a [(Slot, Fixed)],
}

impl SlotMap<'_> {
    /// Whether it passes a slot of its owning body at some key.
    pub fn passes_on(&self) -> bool {
        (self.passed.iter()).any(|(_, fixed)| matches!(fixed, Fixed::Passed(_)))
    }
}

/// What a site's content depends on: nothing that varies per hop born under one park, the frame
/// body's parameters at these slots, or something that varies regardless.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Deps<'a> {
    Closed,
    On(&'a [Slot]),
    Varying,
}

/// The dependencies of content as they gather in scratch while it is walked, laid once at the end:
/// the union of every [`Deps`] added.
pub struct DepsGathering<'x> {
    varying: bool,
    /// Whether the content reads anything open: a capture or a parameter.
    open: bool,
    slots: BumpVec<'x, Slot>,
}

impl<'x> DepsGathering<'x> {
    /// Nothing gathered: closed.
    pub fn new(scratch: BumpAllocator<'x>) -> Self {
        DepsGathering {
            varying: false,
            open: false,
            slots: BumpVec::new_in(scratch),
        }
    }

    /// Gather `deps`.
    pub fn add(&mut self, deps: Deps<'_>) {
        match deps {
            Deps::Varying => self.varying = true,
            Deps::Closed => {}
            Deps::On(slots) => {
                self.open = true;
                self.slots.extend_from_slice(slots);
            }
        }
    }

    /// What is gathered, laid through `lay`: the slots sorted and deduplicated.
    pub fn laid<'a>(&mut self, lay: impl Lays<'a>) -> Deps<'a> {
        if self.varying {
            return Deps::Varying;
        }
        if !self.open {
            return Deps::Closed;
        }
        self.slots.sort_unstable();
        self.slots.dedup();
        Deps::On(collect(lay, self.slots.iter().copied()))
    }
}

/// Gather what the content `tree`, read at `holder` whose frame body is `frame`, depends on: each
/// leaf bound to a parameter of the frame body, no parameter for a leaf captured from a body
/// enclosing the frame, and anything for a leaf no binder names, one bound per run of a body, or
/// one bound by a non-parameter of the frame or of a block or module body it runs. A type value
/// written over names depends on the binder of each run-bound name it is written over, and on
/// anything where it holds another variable.
pub fn tree_deps(
    (shapes, types): (&Shapes<'_>, &Chain<'_, '_, '_, '_>),
    (holder, frame): (Segment, Segment),
    tree: &Tree<'_>,
    into: &mut DepsGathering<'_>,
    scratch: BumpAllocator<'_>,
) {
    let mut stack = BumpVec::new_in(scratch);
    stack.push(*tree);
    while let Some(tree) = stack.pop() {
        match tree {
            Tree::Digest(..) => {}
            Tree::Body { captures, .. } => stack.extend(captures.iter().map(|(_, at)| *at)),
            Tree::Knot { members, .. } => {
                for member in members {
                    stack.extend(
                        member
                            .captures
                            .iter()
                            .filter_map(|(_, capture)| match capture {
                                NodeCapture::Tree(tree) => Some(*tree),
                                NodeCapture::Fellow(_) => None,
                            }),
                    );
                }
            }
            Tree::View {
                source, underlying, ..
            } => stack.extend([*source, *underlying]),
            Tree::Retyped { source, .. } => stack.push(*source),
            Tree::TypeValue { named, .. } => {
                type_value_deps((shapes, types), (holder, frame), named, into, scratch);
            }
            Tree::Leaf(Leaf::Unnamed(_) | Leaf::Call { .. })
            | Tree::Leaf(Leaf::Bound {
                within: Some(_), ..
            }) => return into.add(Deps::Varying),
            Tree::Leaf(Leaf::Bound { binder, .. }) => {
                binder_deps(shapes, (holder, frame), (binder.body, binder.slot), into);
            }
        }
    }
}

/// Gather what the type value `named`, read at `holder` whose frame body is `frame`, depends on:
/// the binder each lexical variable in it is homed at, the first body outward from `holder`
/// declaring its level; anything for any other variable, or a level no body there declares.
fn type_value_deps(
    (shapes, types): (&Shapes<'_>, &Chain<'_, '_, '_, '_>),
    (holder, frame): (Segment, Segment),
    named: Parametric,
    into: &mut DepsGathering<'_>,
    scratch: BumpAllocator<'_>,
) {
    let mut levels = BumpVec::new_in(scratch);
    let mut varying = false;
    crate::type_lattice::read_through(
        types,
        scratch,
        named,
        crate::type_lattice::Side::Above,
        &mut |variable| {
            match variable {
                crate::type_lattice::Variable::Lexical { level, .. } => levels.push(level),
                _ => varying = true,
            }
            None
        },
    );
    if varying {
        return into.add(Deps::Varying);
    }
    for level in levels {
        let mut at = Some(holder);
        let declared = loop {
            let Some(segment) = at else {
                break None;
            };
            if let Some(Target::Local(slot)) = shapes.home(segment, level) {
                break Some((segment, slot));
            }
            at = shapes.parent(segment).map(|(parent, _)| parent);
        };
        match declared {
            Some(binder) => binder_deps(shapes, (holder, frame), binder, into),
            None => return into.add(Deps::Varying),
        }
    }
}

/// Gather what the binder at `slot` of `body`, read at `holder` whose frame body is `frame`,
/// depends on: a parameter of the frame body is itself; a capture from a body enclosing the frame
/// is no parameter — open, but the same at every frame of one closure; anything else varies.
fn binder_deps(
    shapes: &Shapes<'_>,
    (holder, frame): (Segment, Segment),
    (body, slot): (Segment, Slot),
    into: &mut DepsGathering<'_>,
) {
    // The chain from the holder up: the bodies the frame runs, the frame, then those around it.
    let mut at = Some(holder);
    let mut past_frame = false;
    while let Some(segment) = at {
        if segment == body {
            let parameter = shapes.shape_at(body).position_of(slot) == Position::PARAMETER;
            let own = [slot];
            return into.add(match (past_frame, segment == frame) {
                (true, _) => Deps::On(&[]),
                (false, true) if parameter => Deps::On(&own),
                _ => Deps::Varying,
            });
        }
        past_frame |= segment == frame;
        at = shapes.parent(segment).map(|(parent, _)| parent);
    }
    into.add(Deps::Varying)
}

/// One minting or composite site the caching fold classes: where it sits, the frame body its
/// dependencies name parameters of, what it depends on, and the content tree of each path class
/// it reads.
#[derive(Clone, Copy, Debug)]
pub struct SiteDeps<'a> {
    pub holder: Segment,
    pub frame: Segment,
    pub site: Site,
    pub minting: bool,
    pub deps: Deps<'a>,
    pub pending: &'a [Tree<'a>],
}

/// A lexical variable at `level` a contribution or a solution names, read from the body `reading`:
/// every callable or `MODULE` body between composes it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct NamedVariable {
    pub reading: Segment,
    pub level: usize,
}

/// The fact readers over a source of records: [`LoadFacts`] over one load's, [`Facts`] over a
/// cell's layers and the load they rest on. Every reader takes the body's shape and finds the fact
/// by its segment, so a reader may hand in a shape borrowed for any lifetime; one whose fact names
/// a type takes the reading frame's chain before it.
#[derive(Clone, Copy)]
pub struct FactsOver<S>(S);

/// Every fact one load decided, by body, beside the shape table they are facts of.
pub type LoadFacts<'graph> = FactsOver<LoadRecords<'graph>>;

/// What a step reads facts through, at the step's own lifetime `'step`: the load its cell's chain
/// rests on, beneath the cell's own layers. Built per step from the cell's depot reference, so
/// keeping one past the step does not compile:
///
/// ```compile_fail
/// use koan::memory::{Writer, resident};
/// use koan::scope::Facts;
///
/// fn keep<'here>(writer: Writer<'here, '_>, facts: Facts<'_>) -> &'here Facts<'here> {
///     resident(writer, facts)
/// }
/// ```
pub type Facts<'step> = FactsOver<Layers<'step>>;

/// One load's records: its base layer, beside the shape table they are records of and each body's
/// folds.
#[derive(Clone, Copy)]
pub struct LoadRecords<'graph> {
    shapes: &'graph Shapes<'graph>,
    /// The load's base layer: binding nothing in production.
    layer: &'graph BuildLayer<'graph>,
    /// Each body, by segment, beside its type captures as its birth reads them and its folds: none
    /// before the load folds, when a read falls to the base.
    bodies: &'graph [Laid<'graph>],
    /// A test's resumption laid as a load over the load: every read tries its own layer first.
    base: Option<&'graph LoadFacts<'graph>>,
    /// Each layer the load laid for a traced call, in lay order, a parent before its children:
    /// never evicted. A layer's ordinal is its index.
    layers: &'graph [LoadLayer<'graph>],
    /// `layers` by parent and key, each beside its ordinal, sorted.
    index: &'graph [(Option<u32>, LayerKey, u32)],
}

/// A layer the load laid: the layer it lies beneath — by ordinal in the load's table, or the root
/// — its key, the body it was laid for, and the layer.
#[derive(Clone, Copy)]
pub struct LoadLayer<'a> {
    pub parent: Option<u32>,
    pub key: LayerKey,
    pub body: Segment,
    pub layer: &'a BuildLayer<'a>,
}

/// A cell's layers over the base its chain rests on — the load, or a composition: the layers its
/// frame's activation decided, innermost first, then the depot layer it runs under, whose parents
/// lie below it. A read tries each layer from the innermost, then the base, and a unit an inner
/// layer re-ran replaces every outer record of it whole.
#[derive(Clone, Copy)]
pub struct Layers<'step> {
    base: LoadRecords<'step>,
    composition: Option<&'step Composition<'step>>,
    depot: Option<&'step BuildLayer<'step>>,
    decided: Option<&'step BuildLayer<'step>>,
}

impl<'step> Facts<'step> {
    /// The facts a cell under the load reads: the load's alone.
    pub fn of(load: &LoadFacts<'step>) -> Self {
        Facts::under(load, None)
    }

    /// The facts a cell under the depot layer `top` reads: its layers over the load.
    pub fn under(load: &LoadFacts<'step>, top: Option<&'step BuildLayer<'step>>) -> Self {
        FactsOver(Layers {
            base: load.0,
            composition: None,
            depot: top,
            decided: None,
        })
    }

    /// The facts a cell whose chain rests on `composition` reads: its layers over the
    /// composition's, under the depot layer `top`.
    pub fn composed(
        composition: &'step Composition<'step>,
        top: Option<&'step BuildLayer<'step>>,
    ) -> Self {
        FactsOver(Layers {
            base: composition.facts().0,
            composition: Some(composition),
            depot: top,
            decided: None,
        })
    }

    /// The composition the cell's chain rests on, if not the load.
    pub fn composition(&self) -> Option<&'step Composition<'step>> {
        self.0.composition
    }

    /// These facts beneath `decided`, the layers a frame's activation decided, in place of any
    /// these read.
    pub fn activated(self, decided: Option<&'step BuildLayer<'step>>) -> Self {
        FactsOver(Layers { decided, ..self.0 })
    }

    /// These facts as an event's witnessing walk reads them: each layer of `top`, the
    /// activation's decided layers — its cell's earlier events' and its enclosers' — that
    /// witnessed or was walked closed, re-parented through `lay` over the next kept below it and,
    /// at the bottom, over none. A binding's layer over a witnessed one is dropped, so the names
    /// its event witnessed stay rigid. These facts as they are where none is dropped.
    pub fn spine(
        self,
        lay: impl Lays<'step>,
        scratch: BumpAllocator<'_>,
        top: Option<&'step BuildLayer<'step>>,
    ) -> Self {
        let chain = || std::iter::successors(top, |layer| layer.parent());
        if !chain().any(BuildLayer::over_witnessed) {
            return self;
        }
        let mut kept: BumpVec<'_, &BuildLayer<'step>> = BumpVec::new_in(scratch);
        kept.extend(chain().filter(|layer| !layer.over_witnessed()));
        let spine =
            (kept.iter().rev()).fold(None, |below, layer| Some(resident(lay, layer.over(below))));
        self.activated(spine)
    }

    /// These facts without the depot's layers: what a quote's code's entry decides over, its frame
    /// reading through no settlement around it — its own decided layers read before them.
    pub fn undeposited(self) -> Self {
        FactsOver(Layers {
            depot: None,
            ..self.0
        })
    }

    /// The innermost depot layer these facts read, if any.
    pub fn top(&self) -> Option<&'step BuildLayer<'step>> {
        self.0.depot
    }

    /// The innermost layer these facts read: the activation's decided top, else the depot's.
    pub fn innermost(&self) -> Option<&'step BuildLayer<'step>> {
        self.0.decided.or(self.0.depot)
    }

    /// These facts with `layer`, whose parent is their innermost layer or the bottom of an
    /// activation's decided chain, read before it: a witnessed layer a closing walk reads over.
    pub fn beneath(self, layer: &'step BuildLayer<'step>) -> Self {
        let below = layer.parent();
        let decided = self.0.decided.is_some() || (below.is_none() && self.0.depot.is_some());
        debug_assert!(
            match decided {
                true =>
                    below
                        .zip(self.0.decided)
                        .is_none_or(|(below, top)| { std::ptr::addr_eq(below, top) })
                        && below.is_some() == self.0.decided.is_some(),
                false => match (below, self.0.depot) {
                    (Some(below), Some(top)) => std::ptr::addr_eq(below, top),
                    (None, None) => true,
                    _ => false,
                },
            },
            "a layer read beneath facts lies over their innermost"
        );
        match decided {
            true => self.activated(Some(layer)),
            false => FactsOver(Layers {
                depot: Some(layer),
                ..self.0
            }),
        }
    }

    /// The request of the call site `site` of `shape`: the depot layer's this cell runs under,
    /// else the base's. `None` where neither carries one.
    pub fn request(&self, shape: &BodyShape<'_>, site: Site) -> Option<&'step SiteRequest<'step>> {
        let at = shape.segment();
        (self.0.depot.and_then(|layer| layer.request(at, site)))
            .or_else(|| self.0.base.layer.request(at, site))
    }

    /// Hand `each` every term a frame reading these facts mints so every type they name resolves
    /// in its chain, in the order a term's leaves resolve: the composition's, then each
    /// settlement's from the outermost. None over the load.
    pub fn each_term(&self, each: &mut dyn FnMut(&'step Term<'step>)) {
        #[cfg(test)]
        let each = &mut |term| {
            TERM_PROBES.with(|probes| probes.set(probes.get() + 1));
            each(term)
        };
        (self.0.base.layer.terms().entries().iter()).for_each(|(_, term)| each(term));
        let mut settlements: smallvec::SmallVec<[&BuildLayer<'step>; 4]> =
            std::iter::successors(self.0.depot, |layer| layer.parent()).collect();
        settlements.reverse();
        (settlements.iter())
            .flat_map(|layer| layer.terms().entries())
            .for_each(|(_, term)| each(term));
    }

    /// These facts' layers, for a walk to rest on: every term they hold is minted into `types`
    /// first, so each type their records name resolves in the walk's chain.
    pub(crate) fn whole(&self, types: &Chain<'_, '_, '_, '_>) -> Layers<'step> {
        self.each_term(&mut |term| {
            types.mint(term);
        });
        self.0
    }

    /// Whether any layer binds `unknown`.
    pub fn binds(&self, unknown: Unknown) -> bool {
        self.0.bound(unknown).is_some()
    }

    /// The refusal the innermost settlement's walk reached, else the refusal of the composed code
    /// the chain rests on, if either reached one, its types resolving in `types`.
    pub fn refusal(&self, types: &Chain<'_, '_, '_, '_>) -> Option<&'step ShapeError<'step>> {
        let refusal = match self.0.depot {
            Some(top) => top.refusal(),
            None => self.0.base.layer.refusal(),
        };
        // A walk's own refusal is worded under the binding it walked: nothing to close.
        self.minted(types, refusal)
    }
}

impl<'step> Layers<'step> {
    /// Each layer, innermost first: the activation's decided chain, then the depot chain.
    fn layers(&self) -> impl Iterator<Item = &'step BuildLayer<'step>> {
        (std::iter::successors(self.decided, |layer| layer.parent()))
            .chain(std::iter::successors(self.depot, |layer| layer.parent()))
    }

    /// Whether a layer inner to `outer` — every layer where `outer` is `None` — re-ran `unit`.
    fn rerun_within(&self, outer: Option<&BuildLayer<'_>>, unit: UnitId) -> bool {
        (self.layers())
            .take_while(|layer| outer.is_none_or(|outer| !std::ptr::addr_eq(*layer, outer)))
            .any(|layer| layer.unit(unit).is_some())
    }
}

/// Where the fact readers find a record. A load's own records lie over its base's, and a re-run
/// unit's record replaces the base's whole.
pub(crate) trait RecordSource<'a>: Copy {
    fn shapes(&self) -> &'a Shapes<'a>;

    /// What `unknown` is bound to, by any layer: each layer binds its own names.
    fn bound(&self, unknown: Unknown) -> Option<Bound<'a>>;

    /// Hand `each` every unknown any layer binds, beside what it binds it to, the innermost
    /// layer's first.
    fn each_bound(&self, each: &mut dyn FnMut(Unknown, Bound<'a>));

    /// The newest record of `unit`, beside its frontier.
    fn unit(&self, unit: UnitId) -> Option<(&'a UnitFacts<'a>, Frontier<'a>)>;

    /// The newest closing record of the owning body `body`, beside its frontier.
    fn closing(&self, body: Segment) -> Option<(&'a Closing<'a>, Frontier<'a>)>;

    /// The record holding `held`, beside its unit.
    fn holding(&self, held: Held) -> Option<(UnitId, &'a UnitFacts<'a>)>;

    /// Hand `each` every holder in `segment` a current record says holds a module of `identity`'s
    /// tree, beside its unit, perhaps more than once.
    fn holders(&self, segment: Segment, identity: Identity, each: &mut dyn FnMut(UnitId, Holder));

    /// `body` as its load laid it.
    fn laid(&self, body: Segment) -> &'a Laid<'a>;

    /// The folds of `body`.
    fn folds(&self, body: Segment) -> Option<Folds<'a>>;

    /// The derived name `id`, which the innermost layer that laid it holds.
    fn derived_name(&self, id: crate::type_lattice::DerivedId) -> Option<DerivedName<'a>>;

    /// The closed tree whose identity is `digest`.
    fn tree_of(&self, digest: ContentDigest) -> Option<&'a Tree<'a>>;

    /// Whether these records rest on a composition, whose facts may name a type only a term
    /// makes in a reading frame's chain.
    fn composed(&self) -> bool {
        false
    }

    /// The term a layer laid for the type whose digest is `digest`: the composition's, then each
    /// settlement's. None over the load.
    fn term(&self, _digest: TypeDigest) -> Option<&'a Term<'a>> {
        None
    }

    /// Whether some layer holds a table of closed types: else no fact needs closing.
    fn closes(&self) -> bool {
        false
    }

    /// What the innermost layer whose table holds `ty` closes it to, whole or in part.
    fn closed(&self, ty: DeclaredType<Parametric>) -> Option<Closed> {
        let _ = ty;
        None
    }

    /// The tree the innermost layer whose table holds the tree of `identity` gives it.
    fn closed_tree(&self, identity: Identity) -> Option<&'a Tree<'a>> {
        let _ = identity;
        None
    }
}

impl<'graph> RecordSource<'graph> for LoadRecords<'graph> {
    fn shapes(&self) -> &'graph Shapes<'graph> {
        self.shapes
    }

    fn bound(&self, unknown: Unknown) -> Option<Bound<'graph>> {
        (self.layer.binding.bound(unknown)).or_else(|| self.base?.0.bound(unknown))
    }

    fn each_bound(&self, each: &mut dyn FnMut(Unknown, Bound<'graph>)) {
        for (unknown, bound) in self.layer.binding.entries() {
            each(*unknown, *bound);
        }
        if let Some(base) = self.base {
            base.0.each_bound(&mut |unknown, bound| {
                if self.layer.binding.bound(unknown).is_none() {
                    each(unknown, bound);
                }
            });
        }
    }

    fn unit(&self, unit: UnitId) -> Option<(&'graph UnitFacts<'graph>, Frontier<'graph>)> {
        match self.layer.unit(unit) {
            Some(record) => record.typed(),
            None => self.base?.0.unit(unit),
        }
    }

    fn closing(&self, body: Segment) -> Option<(&'graph Closing<'graph>, Frontier<'graph>)> {
        (self.layer.closing(body)).or_else(|| self.base?.0.closing(body))
    }

    /// The record holding `held`, beside its unit: this load's, else its base's where this load did
    /// not re-run the unit, since a re-run record replaces the base's whole.
    fn holding(&self, held: Held) -> Option<(UnitId, &'graph UnitFacts<'graph>)> {
        if let Some(found) = self.layer.holding(held) {
            return Some(found);
        }
        let (unit, facts) = self.base?.0.holding(held)?;
        self.layer.unit(unit).is_none().then_some((unit, facts))
    }

    fn holders(&self, segment: Segment, identity: Identity, each: &mut dyn FnMut(UnitId, Holder)) {
        self.layer.holders(segment, identity, each);
        if let Some(base) = self.base {
            base.0.holders(segment, identity, &mut |unit, holder| {
                if self.layer.unit(unit).is_none() {
                    each(unit, holder);
                }
            });
        }
    }

    /// `body` as this load laid it, or its base where this load has not folded yet.
    fn laid(&self, body: Segment) -> &'graph Laid<'graph> {
        match self.bodies.get(body.index() as usize) {
            Some(laid) => laid,
            None => (self.base)
                .expect("a load folds its records before a body is read")
                .0
                .laid(body),
        }
    }

    /// The folds of `body`: this load's, else its base's.
    fn folds(&self, body: Segment) -> Option<Folds<'graph>> {
        match (self.bodies.get(body.index() as usize)).and_then(|laid| laid.folds) {
            Some(folds) => Some(folds),
            None => self.base?.0.folds(body),
        }
    }

    fn derived_name(&self, id: crate::type_lattice::DerivedId) -> Option<DerivedName<'graph>> {
        match self.layer.derived().get(id) {
            Some(name) => Some(*name),
            None => self.base?.0.derived_name(id),
        }
    }

    /// Whether this load's layer or its base's holds a table of closed types: a test's resumption
    /// laid as a load closes what it absorbed as a layer does.
    fn closes(&self) -> bool {
        self.layer.closes() || self.base.is_some_and(|base| base.0.closes())
    }

    fn closed(&self, ty: DeclaredType<Parametric>) -> Option<Closed> {
        (self.layer.closed().probe(ty)).or_else(|| self.base?.0.closed(ty))
    }

    fn closed_tree(&self, identity: Identity) -> Option<&'graph Tree<'graph>> {
        (self.layer.closed().tree(identity)).or_else(|| self.base?.0.closed_tree(identity))
    }

    fn tree_of(&self, digest: ContentDigest) -> Option<&'graph Tree<'graph>> {
        (self.layer.tree_of(digest)).or_else(|| self.base?.0.tree_of(digest))
    }
}

impl<'step> RecordSource<'step> for Layers<'step> {
    fn composed(&self) -> bool {
        self.composition.is_some()
    }

    fn closes(&self) -> bool {
        self.decided.is_some_and(BuildLayer::closes) || self.depot.is_some_and(BuildLayer::closes)
    }

    fn closed(&self, ty: DeclaredType<Parametric>) -> Option<Closed> {
        self.layers().find_map(|layer| layer.closed().probe(ty))
    }

    fn closed_tree(&self, identity: Identity) -> Option<&'step Tree<'step>> {
        self.layers()
            .find_map(|layer| layer.closed().tree(identity))
    }

    fn term(&self, digest: TypeDigest) -> Option<&'step Term<'step>> {
        self.composition?;
        let term = self.base.layer.terms().get(digest).or_else(|| {
            std::iter::successors(self.depot, |layer| layer.parent())
                .find_map(|layer| layer.terms().get(digest))
        });
        term.map(|(_, term)| term)
    }

    fn shapes(&self) -> &'step Shapes<'step> {
        self.base.shapes()
    }

    fn bound(&self, unknown: Unknown) -> Option<Bound<'step>> {
        (self.layers())
            .find_map(|layer| layer.binding.bound(unknown))
            .or_else(|| self.base.bound(unknown))
    }

    fn each_bound(&self, each: &mut dyn FnMut(Unknown, Bound<'step>)) {
        let shadowed = |unknown, inner: &BuildLayer<'_>| {
            (self.layers())
                .take_while(|layer| !std::ptr::addr_eq(*layer, inner))
                .any(|layer| layer.binding.bound(unknown).is_some())
        };
        for layer in self.layers() {
            for (unknown, bound) in layer.binding.entries() {
                if !shadowed(*unknown, layer) {
                    each(*unknown, *bound);
                }
            }
        }
        self.base.each_bound(&mut |unknown, bound| {
            if self
                .layers()
                .all(|layer| layer.binding.bound(unknown).is_none())
            {
                each(unknown, bound);
            }
        });
    }

    fn unit(&self, unit: UnitId) -> Option<(&'step UnitFacts<'step>, Frontier<'step>)> {
        match self.layers().find_map(|layer| layer.unit(unit)) {
            Some(record) => record.typed(),
            None => self.base.unit(unit),
        }
    }

    fn closing(&self, body: Segment) -> Option<(&'step Closing<'step>, Frontier<'step>)> {
        (self.layers())
            .find_map(|layer| layer.closing(body))
            .or_else(|| self.base.closing(body))
    }

    fn holding(&self, held: Held) -> Option<(UnitId, &'step UnitFacts<'step>)> {
        for layer in self.layers() {
            if let Some((unit, facts)) = layer.holding(held) {
                return (!self.rerun_within(Some(layer), unit)).then_some((unit, facts));
            }
        }
        let (unit, facts) = self.base.holding(held)?;
        (!self.rerun_within(None, unit)).then_some((unit, facts))
    }

    fn holders(&self, segment: Segment, identity: Identity, each: &mut dyn FnMut(UnitId, Holder)) {
        for layer in self.layers() {
            layer.holders(segment, identity, &mut |unit, holder| {
                if !self.rerun_within(Some(layer), unit) {
                    each(unit, holder);
                }
            });
        }
        self.base.holders(segment, identity, &mut |unit, holder| {
            if !self.rerun_within(None, unit) {
                each(unit, holder);
            }
        });
    }

    fn laid(&self, body: Segment) -> &'step Laid<'step> {
        self.base.laid(body)
    }

    fn folds(&self, body: Segment) -> Option<Folds<'step>> {
        self.base.folds(body)
    }

    fn derived_name(&self, id: crate::type_lattice::DerivedId) -> Option<DerivedName<'step>> {
        (self.layers())
            .find_map(|layer| layer.derived().get(id).copied())
            .or_else(|| self.base.derived_name(id))
    }

    fn tree_of(&self, digest: ContentDigest) -> Option<&'step Tree<'step>> {
        (self.layers())
            .find_map(|layer| layer.tree_of(digest))
            .or_else(|| self.base.tree_of(digest))
    }
}

// A resumption's base is the load facts it extends. Every type the load's records name lies in the
// root or the agent tier, so a raw read of them needs no mint; a step's [`Facts`] hands a walk its
// layers only through [`Facts::whole`].
impl<'a> RecordSource<'a> for LoadFacts<'a> {
    fn shapes(&self) -> &'a Shapes<'a> {
        self.0.shapes()
    }

    fn bound(&self, unknown: Unknown) -> Option<Bound<'a>> {
        self.0.bound(unknown)
    }

    fn each_bound(&self, each: &mut dyn FnMut(Unknown, Bound<'a>)) {
        self.0.each_bound(each);
    }

    fn unit(&self, unit: UnitId) -> Option<(&'a UnitFacts<'a>, Frontier<'a>)> {
        self.0.unit(unit)
    }

    fn closing(&self, body: Segment) -> Option<(&'a Closing<'a>, Frontier<'a>)> {
        self.0.closing(body)
    }

    fn holding(&self, held: Held) -> Option<(UnitId, &'a UnitFacts<'a>)> {
        self.0.holding(held)
    }

    fn holders(&self, segment: Segment, identity: Identity, each: &mut dyn FnMut(UnitId, Holder)) {
        self.0.holders(segment, identity, each);
    }

    fn laid(&self, body: Segment) -> &'a Laid<'a> {
        self.0.laid(body)
    }

    fn folds(&self, body: Segment) -> Option<Folds<'a>> {
        self.0.folds(body)
    }

    fn derived_name(&self, id: crate::type_lattice::DerivedId) -> Option<DerivedName<'a>> {
        self.0.derived_name(id)
    }

    fn tree_of(&self, digest: ContentDigest) -> Option<&'a Tree<'a>> {
        self.0.tree_of(digest)
    }

    fn closed_tree(&self, identity: Identity) -> Option<&'a Tree<'a>> {
        self.0.closed_tree(identity)
    }
}

/// The code a value is born of, read at the program's own lifetime: the program's written code,
/// or, for a cell whose chain rests on a composition, the code composed at run time, under its root
/// code value, whose syntax lives at `'root`. What a value born of it keeps is its body's record or
/// a quote's syntax. The load is never evicted, so a value may point into it; a composition may be,
/// so a value born of composed code keeps its own copy of its body's record beside the root that
/// finds or rebuilds the composition, and its quote's syntax inside the root's. Every fact a step
/// decides by is read through [`Facts`] instead.
#[derive(Clone, Copy)]
pub struct Written<'graph, 'root, 'step> {
    load: LoadRecords<'graph>,
    composed: Option<(ComposedRoot<'root>, &'step Composition<'step>)>,
}

/// A body's record as a value born of it holds it: the load's, by pointer, or one of composed code
/// laid at `'a`, beside its root at `'root`.
#[derive(Clone, Copy)]
pub enum BodyAt<'graph, 'root, 'a> {
    Written(&'graph BodyRef<'graph, 'graph>),
    Composed(&'a BodyRef<'root, 'a>),
}

impl<'graph: 'root, 'root: 'a, 'a> BodyAt<'graph, 'root, 'a> {
    /// The record itself.
    pub fn get(self) -> &'a BodyRef<'root, 'a> {
        match self {
            BodyAt::Written(body) => body,
            BodyAt::Composed(body) => body,
        }
    }

    /// This record as a value copied through `writer` holds it: the load's pointer kept, composed
    /// code's record copied beside a copy of its root.
    pub fn copied<'to>(self, writer: Writer<'to, '_>) -> BodyAt<'graph, 'to, 'to>
    where
        'graph: 'to,
    {
        match self {
            BodyAt::Written(body) => BodyAt::Written(body),
            BodyAt::Composed(body) => {
                BodyAt::Composed(body.laid(writer, body.root.map(|root| root.copied(writer))))
            }
        }
    }
}

/// Where a code value's syntax is and what shapes it.
#[derive(Clone, Copy, Debug)]
pub enum CodeSyntax<'graph, 'cell> {
    /// A quote the program writes: its body in program storage, its shape the load's, by segment.
    Written {
        body: &'graph KExpression<'graph>,
        segment: Segment,
    },
    /// Code composed at run time, its syntax laid in a region: `root` finds or rebuilds its
    /// composition, and `body` is the root's syntax or a quote nested in it, whose shape `segment`
    /// names among its composition's shapes — `None` for the root itself.
    Composed {
        body: &'cell KExpression<'cell>,
        root: ComposedRoot<'cell>,
        segment: Option<Segment>,
    },
}

impl<'graph: 'cell, 'cell> CodeSyntax<'graph, 'cell> {
    /// The syntax itself.
    pub fn body(self) -> &'cell KExpression<'cell> {
        match self {
            CodeSyntax::Written { body, .. } | CodeSyntax::Composed { body, .. } => body,
        }
    }

    /// The root of the composed code this is, if it is not written.
    pub fn root(self) -> Option<ComposedRoot<'cell>> {
        match self {
            CodeSyntax::Written { .. } => None,
            CodeSyntax::Composed { root, .. } => Some(root),
        }
    }

    /// What laying this syntax again writes: nothing for written code, which a copy points at;
    /// composed code's body, and its root beside it where the body is a quote nested in it.
    pub fn weight(self) -> Weight {
        match self {
            CodeSyntax::Written { .. } => Weight::ZERO,
            CodeSyntax::Composed {
                root,
                segment: None,
                ..
            } => root.weight(),
            CodeSyntax::Composed { body, root, .. } => Weight::syntax(body).plus(root.weight()),
        }
    }

    /// This syntax as a copy through `writer` holds it: written code's pointer kept, composed
    /// code laid again, its root beside its body where the two differ.
    pub fn copied<'to>(self, writer: Writer<'to, '_>) -> CodeSyntax<'graph, 'to>
    where
        'graph: 'to,
    {
        match self {
            CodeSyntax::Written { body, segment } => CodeSyntax::Written { body, segment },
            CodeSyntax::Composed {
                root,
                segment: None,
                ..
            } => {
                let root = root.copied(writer);
                CodeSyntax::Composed {
                    body: root.syntax,
                    root,
                    segment: None,
                }
            }
            CodeSyntax::Composed {
                body,
                root,
                segment,
            } => CodeSyntax::Composed {
                body: copied(writer, body),
                root: root.copied(writer),
                segment,
            },
        }
    }
}

impl<'graph, 'root, 'step> Written<'graph, 'root, 'step> {
    /// The written code `load` holds.
    pub fn of(load: &LoadFacts<'graph>) -> Self {
        Written {
            load: load.0,
            composed: None,
        }
    }

    /// The code composed at run time whose root is `root`, which `composition` holds.
    pub fn composed(
        self,
        root: ComposedRoot<'root>,
        composition: &'step Composition<'step>,
    ) -> Self {
        Written {
            composed: Some((root, composition)),
            ..self
        }
    }

    /// The root of the composed code this is, if it is not the program's.
    pub fn root(&self) -> Option<ComposedRoot<'root>> {
        self.composed.map(|(root, _)| root)
    }

    /// The body at `segment`, as a value born of it holds it, a composed record laid through `lay`
    /// beside its root.
    pub fn body<'a>(&self, lay: impl Lays<'a>, segment: Segment) -> BodyAt<'graph, 'root, 'a>
    where
        'graph: 'a,
    {
        self.laid(lay, segment, self.root())
    }

    /// [`body`](Self::body) with no root on a composed record: what scratch shorter-lived than the
    /// root holds, the root kept beside it.
    pub fn unrooted<'a>(&self, lay: impl Lays<'a>, segment: Segment) -> BodyAt<'graph, 'static, 'a>
    where
        'graph: 'a,
    {
        self.laid(lay, segment, None)
    }

    /// The body at `segment`, a composed record laid through `lay` beside `root`.
    fn laid<'held, 'a>(
        &self,
        lay: impl Lays<'a>,
        segment: Segment,
        root: Option<ComposedRoot<'held>>,
    ) -> BodyAt<'graph, 'held, 'a>
    where
        'graph: 'a,
    {
        match self.composed {
            None => BodyAt::Written(&self.load.laid(segment).body),
            Some((_, composition)) => {
                BodyAt::Composed(composition.facts().0.laid(segment).body.laid(lay, root))
            }
        }
    }

    /// `part`, of the body at `segment`, where the load holds it, which outlives every region;
    /// `None` for composed code, which a value born of it may outlive.
    pub fn part(
        &self,
        segment: Segment,
        part: &ExpressionPart<'_>,
    ) -> Option<&'graph ExpressionPart<'graph>> {
        if self.composed.is_some() {
            return None;
        }
        let shape = self.load.shapes.shape_at(segment);
        match shape.located(shape.site(part)) {
            Located::Part(part) => Some(part),
            Located::Node(_) => unreachable!("a part's site locates a part"),
        }
    }

    /// The syntax of the quote at `site` of the body at `segment`, as a code value born of it
    /// holds it.
    pub fn quote(&self, segment: Segment, site: Site) -> CodeSyntax<'graph, 'root>
    where
        'graph: 'root,
    {
        let Some((root, composition)) = self.composed else {
            let shape = self.load.shapes.shape_at(segment);
            return match shape.located(site) {
                Located::Part(ExpressionPart::QuotedExpression(body)) => CodeSyntax::Written {
                    body,
                    segment: (shape.nested(site))
                        .expect("a quote's site holds its code shape")
                        .segment(),
                },
                _ => unreachable!("a quote's site locates its quoted expression"),
            };
        };
        let shapes = composition.facts().shapes();
        let nested = (shapes.shape_at(segment).nested(site))
            .expect("a quote's site holds its code shape")
            .segment();
        CodeSyntax::Composed {
            body: composition.quote_in(root.syntax, nested),
            root,
            segment: Some(nested),
        }
    }
}

/// A body as a value born of it holds it: the base whose shape table holds it, by digest, its
/// segment there, its code digest, and which of its closure slots its content composes — its
/// builder captures but those reading the program's top level, then the type captures its contract
/// names. A value's identity, digest and equality read those alone. Beside them, what a caller
/// needs of a body it did not shape: the slots a call's settlement keys on, by name, and the root
/// of the composed code it lies in, at `'root`, which finds or rebuilds its composition. One
/// pointer, so a value holds it in place of its shape's.
#[derive(Clone, Copy)]
pub struct BodyRef<'root, 'a> {
    pub base: ContentDigest,
    pub segment: Segment,
    pub code: ContentDigest,
    pub composes: &'a [bool],
    /// The parameter slots a call's settlement keys on, beside their names: `None` where a call
    /// seeks nothing.
    pub keys: Option<&'a [(Slot, BinderSymbol)]>,
    /// `None` for written code.
    pub root: Option<ComposedRoot<'root>>,
    /// Whether the body can birth a callable, which a call seeking a settlement of it lays a level
    /// for.
    pub births: bool,
    /// Whether a key of the body is a parameter its own tail rebinds: a hop of it seeks rather than
    /// borrowing its frame's layer.
    pub rebinds: bool,
}

/// The root of code composed at run time: its syntax, laid where the code value holding it lives,
/// its composition's digest, which finds the composition, and the parameters the composition
/// binds — a callable's, in its shape's channel order, where the code is the untraced body of one,
/// and none for code an `EVAL` or a `BUILD` runs, whose digest is its syntax digest.
#[derive(Clone, Copy, Debug)]
pub struct ComposedRoot<'a> {
    pub syntax: &'a KExpression<'a>,
    pub digest: ContentDigest,
    pub parameters: &'a [BinderSymbol],
}

impl ComposedRoot<'_> {
    /// This root with its syntax and parameters copied through `writer`.
    pub fn copied<'to>(self, writer: Writer<'to, '_>) -> ComposedRoot<'to> {
        ComposedRoot {
            syntax: copied(writer, self.syntax),
            digest: self.digest,
            parameters: collect(writer, self.parameters.iter().copied()),
        }
    }

    /// What laying a copy of this root writes: its syntax and its parameters.
    pub fn weight(&self) -> Weight {
        Weight::syntax(self.syntax).plus(Weight::run::<BinderSymbol>(self.parameters.len()))
    }
}

impl BodyRef<'_, '_> {
    /// Whether the closure slot `at` composes: one past the slots recorded does.
    pub fn composed(&self, at: usize) -> bool {
        self.composes.get(at).copied().unwrap_or(true)
    }

    /// This record copied through `lay` beside `root`.
    pub fn laid<'root, 'to>(
        &self,
        lay: impl Lays<'to>,
        root: Option<ComposedRoot<'root>>,
    ) -> &'to BodyRef<'root, 'to> {
        resident(
            lay,
            BodyRef {
                base: self.base,
                segment: self.segment,
                code: self.code,
                composes: collect(lay, self.composes.iter().copied()),
                keys: self.keys.map(|keys| collect(lay, keys.iter().copied())),
                root,
                births: self.births,
                rebinds: self.rebinds,
            },
        )
    }

    /// What laying a copy of this record costs a rebuild, its root's syntax included: nothing for
    /// written code, whose record the load holds.
    pub fn weight(&self) -> Weight {
        match self.root {
            None => Weight::ZERO,
            Some(root) => Weight::flat::<BodyRef<'_, '_>>()
                .plus(Weight::run::<bool>(self.composes.len()))
                .plus(Weight::run::<(Slot, BinderSymbol)>(
                    self.keys.map_or(0, <[_]>::len),
                ))
                .plus(root.weight()),
        }
    }
}

/// Which closure slots of `shape` its content composes: each builder capture reading no top-level
/// slot, then each type capture as `contract` says.
fn composes<'graph>(
    lay: impl Lays<'graph>,
    shape: &BodyShape<'_>,
    contract: impl Fn(usize) -> bool,
    types: usize,
) -> &'graph [bool] {
    let builder = shape.captures().len();
    lay.fill(builder + types, |at| match at.checked_sub(builder) {
        None => shape.composes(super::shape::CaptureSlot(at as u32)),
        Some(index) => contract(index),
    })
}

/// One body as the load lays it.
#[derive(Clone, Copy)]
pub(crate) struct Laid<'graph> {
    body: BodyRef<'graph, 'graph>,
    captures: &'graph [Coordinate],
    folds: Option<Folds<'graph>>,
}

/// A fact a unit's record holds that the shape table cannot place, by what it is found by. A slot's,
/// a statement's and a birth's unit is the shape's to name: [`Shapes::slot_unit`],
/// [`Shapes::running`] and [`Shapes::birth_unit`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Held {
    Site(Segment, Site),
    /// A path entry, by its chain root and name.
    Path(Segment, PathName),
    /// A class a path entry resolved to, by its chain root and the class: where a reader finds
    /// what a class reads. Of the units resolving a path to one class, a layer keeps the first.
    Class(Segment, PathClass),
    /// A module binder or a view written in place of a body holding a module of a tree of this
    /// identity, by the body, the identity and the holder: where a reader finds the roots a class
    /// is read through.
    Holder(Segment, Identity, Holder),
}

/// What holds a module of a tree in a body: a module binder, by its slot, or a view written in
/// place, by the site of the `:|` building it. Binders order first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Holder {
    Slot(Slot),
    Site(Site),
}

impl<'graph> LoadFacts<'graph> {
    /// The facts of a load that built no shape: what a graph that runs no program reads.
    pub const EMPTY: Self = FactsOver(LoadRecords {
        shapes: &Shapes::EMPTY,
        layer: &BuildLayer::EMPTY,
        bodies: &[],
        base: None,
        layers: &[],
        index: &[],
    });

    /// The bytes of this load's records index.
    #[cfg(test)]
    pub fn index_bytes(&self) -> usize {
        std::mem::size_of_val(self.0.layer.index.entries())
    }

    /// The bytes of the facts this load's records index keys: each own record's paths, and each
    /// body's site-keyed and through tables in it.
    #[cfg(test)]
    pub fn keyed_fact_bytes(&self) -> usize {
        use std::mem::size_of_val;
        let typed = |typed: &Typed<'_>| {
            size_of_val(typed.expressions.entries())
                + size_of_val(typed.parts.entries())
                + size_of_val(typed.settled.entries())
                + size_of_val(typed.instances.entries())
                + size_of_val(typed.uses.entries())
                + size_of_val(typed.selections.entries())
                + size_of_val(typed.by_name.entries())
                + size_of_val(typed.routes.entries())
                + size_of_val(typed.arms.entries())
                + size_of_val(typed.retypes.entries())
                + size_of_val(typed.trees.entries())
                + size_of_val(typed.picks.entries())
                + size_of_val(typed.minting.entries())
                + size_of_val(typed.holding.entries())
        };
        (self.own_units())
            .map(|(_, (facts, _))| {
                size_of_val(facts.paths.entries())
                    + facts.typed.iter().map(|(_, at)| typed(at)).sum::<usize>()
            })
            .sum()
    }

    /// The facts of a load whose walk ran nothing over `shapes`: every fact unknown, and every
    /// capture composing.
    #[cfg(test)]
    pub fn unwalked(lay: impl Lays<'graph>, shapes: &'graph Shapes<'graph>) -> Self {
        FactsOver(LoadRecords {
            shapes,
            layer: &BuildLayer::EMPTY,
            bodies: &[],
            base: None,
            layers: &[],
            index: &[],
        })
        .folded(lay, &[])
    }

    /// One walk's records over `shapes`, laid through `lay`, over `base` where the walk resumed
    /// one under `binding`: `units` and `closings` sorted by id, not yet folded.
    #[allow(clippy::too_many_arguments)]
    pub fn of_records(
        lay: impl Lays<'graph>,
        scratch: BumpAllocator<'_>,
        shapes: &'graph Shapes<'graph>,
        base: Option<&'graph LoadFacts<'graph>>,
        derived: Derived<'graph>,
        binding: Binding<'graph>,
        units: &[(UnitId, UnitRecord<'graph>)],
        closings: &[(Segment, (&'graph Closing<'graph>, Frontier<'graph>))],
    ) -> Self {
        if cfg!(debug_assertions)
            && let Some(base) = base
        {
            base.each_bound(&mut |unknown, below| {
                let bound = binding.bound(unknown);
                assert!(bound.is_some_and(|bound| bound.same_as(&below)));
            });
        }
        debug_assert!(base.is_none_or(|base| std::ptr::eq(base.0.shapes, shapes)));
        let layer = BuildLayer::laid(lay, scratch, shapes, None, binding, units, closings)
            .deriving(derived);
        FactsOver(LoadRecords {
            shapes,
            layer: resident(lay, layer),
            bodies: &[],
            base,
            layers: &[],
            index: &[],
        })
    }

    /// These records over `layer` in place of their base layer: a composition's, once its terms
    /// and refusal are known.
    pub fn rebased(&self, layer: &'graph BuildLayer<'graph>) -> Self {
        FactsOver(LoadRecords { layer, ..self.0 })
    }

    /// The load's base layer.
    pub fn base_layer(&self) -> &'graph BuildLayer<'graph> {
        self.0.layer
    }

    /// These records beside `layers`, the layers the load laid for its traced calls in lay order,
    /// their index by parent and key staged in `scratch` and laid through `lay`.
    pub fn laying(
        &self,
        lay: impl Lays<'graph>,
        scratch: BumpAllocator<'_>,
        layers: &'graph [LoadLayer<'graph>],
    ) -> Self {
        debug_assert!(
            (layers.iter().enumerate())
                .all(|(at, laid)| laid.parent.is_none_or(|parent| (parent as usize) < at)),
            "a load layer's parent precedes it"
        );
        let mut index = BumpVec::with_capacity_in(layers.len(), scratch);
        index.extend(
            (layers.iter().enumerate()).map(|(at, laid)| (laid.parent, laid.key, at as u32)),
        );
        index.sort_unstable();
        debug_assert!(
            index
                .windows(2)
                .all(|pair| (pair[0].0, pair[0].1) != (pair[1].0, pair[1].1)),
            "the load lays one layer per parent and key"
        );
        let index = collect(lay, index.iter().copied());
        FactsOver(LoadRecords {
            layers,
            index,
            ..self.0
        })
    }

    /// The layer the load laid beneath `parent` — a load layer's ordinal, or the root — under
    /// `key`, beside its ordinal in the load's table, if any.
    pub fn layer(
        &self,
        parent: Option<u32>,
        key: LayerKey,
    ) -> Option<(u32, &'graph BuildLayer<'graph>)> {
        let index = self.0.index;
        let at = index
            .binary_search_by_key(&(parent, key), |(parent, key, _)| (*parent, *key))
            .ok()?;
        let ordinal = index[at].2;
        Some((ordinal, self.0.layers[ordinal as usize].layer))
    }

    /// The layer at `ordinal` in the load's table.
    pub fn load_layer(&self, ordinal: u32) -> &'graph BuildLayer<'graph> {
        self.0.layers[ordinal as usize].layer
    }

    /// The layers the load laid for its traced calls, in lay order.
    pub fn layers(&self) -> &'graph [LoadLayer<'graph>] {
        self.0.layers
    }

    /// The witnessed layer under each binding's layer the load laid for `body` beneath `top` — a
    /// load layer, or the root where `None` — once per binding: what a settlement of another
    /// binding beneath it may read over. A miss scans the table, as it walks a body.
    pub fn witnessed(
        &self,
        top: Option<&BuildLayer<'_>>,
        body: Segment,
    ) -> impl Iterator<Item = &'graph BuildLayer<'graph>> + use<'graph> {
        let layers = self.0.layers;
        let top = top.map(|top| std::ptr::from_ref(top).addr());
        let beneath = move |laid: &LoadLayer<'_>| {
            let parent = laid.parent.map(|parent| layers[parent as usize].layer);
            parent.map(|parent| std::ptr::from_ref(parent).addr()) == top
        };
        (layers.iter())
            .filter(move |laid| laid.body == body && beneath(laid))
            .filter_map(|laid| {
                laid.layer
                    .parent()
                    .filter(|below| below.witnessing().is_some())
            })
    }

    /// These records beside `folds`, by segment: every capture composing where it is empty.
    pub fn folded(&self, lay: impl Lays<'graph>, folds: &[Folds<'graph>]) -> Self {
        let shapes = self.0.shapes;
        debug_assert!(folds.is_empty() || folds.len() == shapes.bodies().len());
        let bodies = shapes.bodies();
        let base = shapes.root().code_digest();
        let bodies = lay.fill(bodies.len(), |at| {
            let shape = bodies[at];
            let layout = shapes.layout(shape.segment());
            let folds = folds.get(at).copied();
            let contract = |index: usize| folds.is_none_or(|folds| folds.contract[index]);
            let keys = (folds.and_then(|folds| folds.keys)).filter(|_| !shape.deferred());
            let rebound = shapes.rebound(shape.segment());
            Laid {
                body: BodyRef {
                    base,
                    segment: shape.segment(),
                    code: shape.code_digest(),
                    composes: composes(lay, shape, contract, layout.len()),
                    // An untraced body's call runs under its composition, keyed on nothing.
                    keys: keys.map(|keys| {
                        collect(lay, keys.iter().map(|slot| (*slot, shape.slot_name(*slot))))
                    }),
                    root: None,
                    births: shape.births_callable(),
                    rebinds: keys.is_some_and(|keys| {
                        keys.iter().any(|slot| rebound.binary_search(slot).is_ok())
                    }),
                },
                captures: collect(lay, layout.iter().map(|(_, coordinate)| *coordinate)),
                folds,
            }
        });
        FactsOver(LoadRecords { bodies, ..self.0 })
    }

    /// The newest record of `unit` beside its frontier, its base's where this load did not re-run
    /// it.
    pub fn unit(&self, unit: UnitId) -> Option<(&'graph UnitFacts<'graph>, Frontier<'graph>)> {
        self.0.unit(unit)
    }

    /// The newest closing record of the owning body `body`, beside its frontier.
    pub fn closing(&self, body: Segment) -> Option<(&'graph Closing<'graph>, Frontier<'graph>)> {
        self.0.closing(body)
    }

    /// Hand `each` every type these records name.
    pub fn each_type(&self, each: &mut dyn FnMut(DeclaredType<Parametric>)) {
        self.0.layer.each_own_type(each);
    }

    /// The request the base carries for the call site `site` of `shape`, if any.
    pub fn request(
        &self,
        shape: &BodyShape<'_>,
        site: Site,
    ) -> Option<&'graph SiteRequest<'graph>> {
        self.0.layer.request(shape.segment(), site)
    }

    /// The units this load marked unreached, sorted.
    #[cfg(test)]
    pub fn unreached_units(&self) -> impl Iterator<Item = UnitId> + 'graph {
        self.0.layer.unreached_units()
    }

    /// The units this load ran itself, each beside its record and frontier, sorted.
    pub fn own_units(
        &self,
    ) -> impl Iterator<Item = (UnitId, (&'graph UnitFacts<'graph>, Frontier<'graph>))> + 'graph
    {
        (self.0.layer.units.entries().iter())
            .filter_map(|(unit, record)| Some((*unit, record.typed()?)))
    }
}

#[expect(
    private_bounds,
    reason = "a source of records is the crate's own; outside it only the two aliases are named"
)]
impl<'a, S: RecordSource<'a>> FactsOver<S> {
    /// `fact`, read off these records, with every type it names resolving in `types`, the reading
    /// frame's chain: each the chain lacks is minted from its term ([`RecordSource::term`]). A frame
    /// mints only what it reads, once per tier the chain interns in. Over the load and the
    /// settlements under the root, every type a fact names lies in the root or the agent tier
    /// already. Every reader of a fact naming a type reads through it, so none can skip it.
    fn minted<T: Named>(&self, types: &Chain<'_, '_, '_, '_>, fact: T) -> T {
        if !self.0.composed() {
            return fact;
        }
        fact.each_named(&mut |kt| {
            if types.resolves(kt) {
                return;
            }
            #[cfg(test)]
            TERM_PROBES.with(|probes| probes.set(probes.get() + 1));
            match self.0.term(kt.digest()) {
                Some(term) => types.mint(term),
                None => debug_assert!(false, "a type a fact names has a term"),
            }
        });
        fact
    }

    /// `fact`, read off these records, closed and minted ([`minted`](Self::minted)): each rigid
    /// type it names that a table of closed types its chain's layers hold gives a closed type takes
    /// it ([`ClosedTypes`](super::ClosedTypes)), and then every type it names resolves. A probe
    /// reads a type's digest alone, so a table's value is minted as any other type is. Nothing
    /// interns, and a fact naming no such type is handed back as read.
    fn resolved<T: Named + Close>(&self, types: &Chain<'_, '_, '_, '_>, fact: T) -> T {
        let fact = match self.0.closes() {
            false => fact,
            true => fact.closed(&mut Probing::<S, BumpAllocator<'_>> {
                source: &self.0,
                lay: None,
            }),
        };
        self.minted(types, fact)
    }

    /// [`resolved`](Self::resolved) for a fact holding a slice of types, which closing copies into
    /// `scratch` where some element changes.
    fn resolved_laying<'l, T: Named + CloseLaying<'l>>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'l>,
        fact: T,
    ) -> T {
        let fact = match self.0.closes() {
            false => fact,
            true => fact.closed_laying(&mut Probing {
                source: &self.0,
                lay: Some(scratch),
            }),
        };
        self.minted(types, fact)
    }

    /// `fact`, a fact of `shape` read at `site` where the cell runs, as a reader handed it back
    /// with its types resolving in `types`: closed, refused at every run, or left open to the run's
    /// own door. A rigid fact never reaches the run — every variable the
    /// load left open is bound in a layer the cell reads — save one over a path the load read off a
    /// signature lacking its member, which every run refuses; any other is a bug, refused as the
    /// load refuses a name with no binding.
    pub fn at_run<C, R>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        shape: &BodyShape<'_>,
        site: Site,
        fact: Static<'a, C, R>,
    ) -> AtRun<C> {
        match fact {
            Static::Closed(value) => AtRun::Closed(value),
            Static::Unknown => AtRun::Open,
            Static::Rigid { variables, .. } => {
                AtRun::Refused(self.unbound(types, shape, site, variables))
            }
        }
    }

    /// The type the type expression at `site` of `shape` denotes where the cell runs, or why every
    /// run refuses it. The load, or a layer the cell reads, fixed it: one left open is a bug.
    pub fn denoted(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        shape: &BodyShape<'_>,
        site: Site,
    ) -> Result<KType, Elaboration> {
        match self.at_run(
            types,
            shape,
            site,
            self.typed_expression(types, shape, site),
        ) {
            AtRun::Closed(closed) => Ok(closed),
            AtRun::Refused(refused) => Err(refused),
            AtRun::Open => {
                debug_assert!(
                    false,
                    "the type at {site:?} of body {:?} is open where it runs",
                    shape.segment()
                );
                Err(Elaboration::Unknown { site })
            }
        }
    }

    /// Why a rigid fact of `shape` over `variables`, read at `site`, refuses.
    fn unbound(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        shape: &BodyShape<'_>,
        site: Site,
        variables: &[Variable],
    ) -> Elaboration {
        let shapes = self.shapes();
        let path = |root: Segment, name| self.path(types, shapes.shape_at(root), name);
        for variable in variables {
            if let Variable::Path { root, entry, .. } = *variable
                && let Some((entry, resolution)) = path(root, entry)
                && let Some(owner) = resolution.lacks
            {
                return Elaboration::NoSuchMember {
                    owner,
                    name: entry.label.symbol(),
                    site,
                };
            }
        }
        #[cfg(test)]
        rigid_at_run();
        debug_assert!(
            false,
            "a rigid fact of body {:?} at {site:?} reached the run",
            shape.segment()
        );
        let name = variables.iter().find_map(|variable| match *variable {
            Variable::Type { at, .. } => match shapes.follow(shape.segment(), at)? {
                Lands::Binder { at, slot } => Some(shapes.shape_at(at).slot_name(slot).symbol()),
                _ => None,
            },
            Variable::Path { root, entry, .. } => Some(path(root, entry)?.0.label.symbol()),
        });
        match name {
            Some(name) => Elaboration::Unbound { name, site },
            None => Elaboration::Unknown { site },
        }
    }

    /// The shape table these are facts of.
    pub fn shapes(&self) -> &'a Shapes<'a> {
        self.0.shapes()
    }

    /// The term of the derived name `id`, which the innermost layer that laid it holds.
    pub fn term_of(&self, id: crate::type_lattice::DerivedId) -> Option<DerivedTerm<'a>> {
        self.0.derived_name(id).map(|name| name.term)
    }

    /// The derived name `id`, which the innermost layer that laid it holds.
    pub fn derived_name(&self, id: crate::type_lattice::DerivedId) -> Option<DerivedName<'a>> {
        self.0.derived_name(id)
    }

    /// What any layer binds `unknown` to.
    pub fn bound(&self, unknown: Unknown) -> Option<Bound<'a>> {
        self.0.bound(unknown)
    }

    /// Hand `each` every unknown any layer binds, beside what it binds it to, the innermost
    /// layer's first.
    pub fn each_bound(&self, each: &mut dyn FnMut(Unknown, Bound<'a>)) {
        self.0.each_bound(each);
    }

    /// The closed tree whose identity is `digest`, among those these records computed.
    pub fn tree_of(&self, digest: ContentDigest) -> Option<&'a Tree<'a>> {
        self.0.tree_of(digest)
    }

    /// `segment`'s facts in the record holding `held`.
    fn typed(&self, segment: Segment, held: Held) -> Option<&'a Typed<'a>> {
        let (_, facts) = self.0.holding(held)?;
        facts.typed.get(segment).map(|(_, typed)| typed)
    }

    /// `shape` as this load's table holds it, found by its segment: so a reader's shape may be
    /// borrowed for any lifetime.
    fn at(&self, shape: &BodyShape<'_>) -> &'a BodyShape<'a> {
        let at = self.0.shapes().shape_at(shape.segment());
        debug_assert!(std::ptr::addr_eq(at, shape), "a shape of this load");
        at
    }

    /// The entry at `key` of the `table` of the facts the unit holding `held` fixed for `shape`.
    fn fact<K: Ord + Copy + 'a, V: Copy + 'a>(
        &self,
        shape: &BodyShape<'_>,
        held: Held,
        table: impl FnOnce(&Typed<'a>) -> Keyed<'a, (K, V)>,
        key: K,
    ) -> Option<V> {
        let typed = self.typed(shape.segment(), held)?;
        table(typed).get(key).map(|(_, value)| *value)
    }

    /// The entry at `site` of the site-keyed `table` of `shape`'s facts.
    fn at_site<V: Copy + 'a>(
        &self,
        shape: &BodyShape<'_>,
        site: Site,
        table: impl FnOnce(&Typed<'a>) -> Keyed<'a, (Site, V)>,
    ) -> Option<V> {
        self.fact(shape, Held::Site(shape.segment(), site), table, site)
    }

    /// `shape`'s facts in the newest record of `unit`.
    fn typed_in(&self, shape: &BodyShape<'_>, unit: UnitId) -> Option<&'a Typed<'a>> {
        let (facts, _) = self.0.unit(unit)?;
        facts.typed.get(shape.segment()).map(|(_, typed)| typed)
    }

    /// The entry at `slot` of the slot-keyed `table` of `shape`'s facts.
    fn at_slot<V: Copy + 'a>(
        &self,
        shape: &BodyShape<'_>,
        slot: Slot,
        table: impl FnOnce(&Typed<'a>) -> Keyed<'a, (Slot, V)>,
    ) -> Option<V> {
        let typed = self.typed_in(shape, self.0.shapes().slot_unit(shape.segment(), slot))?;
        table(typed).get(slot).map(|(_, value)| *value)
    }

    /// The birth facts of `body`, a callable or a quote's code, in its birth unit's newest record.
    fn born(&self, body: Segment) -> Option<&'a Born<'a>> {
        let (facts, _) = self.0.unit(self.0.shapes().birth_unit(body)?)?;
        facts.born.get(body).map(|(_, born)| born)
    }

    /// Whether the site-listing `table` of `shape`'s facts lists `site`.
    fn lists_site(
        &self,
        shape: &BodyShape<'_>,
        site: Site,
        table: impl FnOnce(&Typed<'a>) -> Keyed<'a, Site>,
    ) -> bool {
        (self.typed(shape.segment(), Held::Site(shape.segment(), site)))
            .is_some_and(|typed| table(typed).get(site).is_some())
    }

    /// What the load fixed for the type expression at `site` of `shape`: `Unknown` where it records
    /// none there.
    pub fn typed_expression(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        shape: &BodyShape<'_>,
        site: Site,
    ) -> StaticType<'a> {
        let typed = self.at_site(shape, site, |typed| typed.expressions);
        self.resolved(types, typed.unwrap_or(Static::Unknown))
    }

    /// What the load fixed for the type binder at `slot` of `shape`.
    pub fn declared_type(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        shape: &BodyShape<'_>,
        slot: Slot,
    ) -> StaticType<'a> {
        let declared = self.at_slot(shape, slot, |typed| typed.declared);
        self.resolved(types, declared.unwrap_or(Static::Unknown))
    }

    /// What the load fixed for the registration at `slot` of `shape`.
    pub fn registered_type(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        shape: &BodyShape<'_>,
        slot: Slot,
    ) -> StaticRegistered<'a> {
        let registered = self.at_slot(shape, slot, |typed| typed.registered);
        self.resolved(types, registered.unwrap_or(Static::Unknown))
    }

    /// Whether the callable body `body` is born with `name` among its `FOR ALL` names: where a
    /// call's solution, not an argument, binds it.
    pub fn quantifies(&self, body: Segment, name: BinderSymbol) -> bool {
        let BinderSymbol::Type(name) = name else {
            return false;
        };
        match self.born(body).map(|born| born.callable) {
            Some(Static::Closed(callable)) => callable.quantifier_map.get(name).is_some(),
            Some(Static::Rigid { value, .. }) => value.quantifier_map.get(name).is_some(),
            _ => false,
        }
    }

    /// What the load fixed for the callable body `shape`'s type.
    pub fn callable_type(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        shape: &BodyShape<'_>,
    ) -> StaticCallable<'a> {
        let callable = (self.born(shape.segment())).map_or(Static::Unknown, |born| born.callable);
        self.resolved(types, callable)
    }

    /// The callable body `shape`'s own `FOR ALL` group as its body reads it, by group order.
    pub fn group_levels(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        shape: &BodyShape<'_>,
    ) -> &'a [Parametric] {
        self.minted(
            types,
            self.born(shape.segment()).map_or(&[], |born| born.levels),
        )
    }

    /// The solution the callable body `shape`'s `FOR ALL` is born instantiated at, where the load
    /// solved one.
    pub fn born_instance<'l>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'l>,
        shape: &BodyShape<'_>,
    ) -> Option<StaticInstance<'l>>
    where
        'a: 'l,
    {
        let instance: Option<StaticInstance<'l>> = self.born(shape.segment())?.instance;
        self.resolved_laying(types, scratch, instance)
    }

    /// Each lexical variable `shape` declares, by level, beside where its activation holds it.
    pub fn homes(&self, shape: &BodyShape<'_>) -> &'a [(usize, Target)] {
        self.0.shapes().homes(shape.segment())
    }

    /// Each type capture of `shape` past its builder captures, as the coordinate its birth reads.
    pub fn type_captures(&self, shape: &BodyShape<'_>) -> &'a [Coordinate] {
        self.0.laid(shape.segment()).captures
    }

    /// How many closure slots a birth of `shape` fills: its builder captures, then its type
    /// captures.
    pub fn capture_count(&self, shape: &BodyShape<'_>) -> usize {
        shape.captures().len() + self.type_captures(shape).len()
    }

    /// Which of `shape`'s type captures its content composes, parallel to
    /// [`type_captures`](Self::type_captures).
    pub fn contract(&self, shape: &BodyShape<'_>) -> &'a [bool] {
        &self.0.laid(shape.segment()).body.composes[shape.captures().len()..]
    }

    /// The path `shape` spells at `site`, where it spells one its chain numbers: the segment of
    /// the body rooting its chain, beside its level there. A text fact, read off the shape table.
    pub fn path_read(&self, shape: &BodyShape<'_>, site: Site) -> Option<(Segment, usize)> {
        let index = shape.spelled_at(site)?;
        let level = self.0.shapes().path_level(shape.segment(), index)?;
        Some((self.0.shapes().chain_root(shape.segment()), level))
    }

    /// The path entry `name` of the chain the body `root` roots, beside what the load resolved it
    /// to.
    pub fn path(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        root: &BodyShape<'_>,
        name: PathName,
    ) -> Option<(PathEntry<'a>, Resolution<'a>)> {
        let (_, facts) = self.0.holding(Held::Path(root.segment(), name))?;
        let path = facts
            .paths
            .get((root.segment(), name))
            .map(|(_, path)| *path);
        self.resolved(types, path)
    }

    /// The first registration of the quote's code `shape` that overlaps a builtin overload,
    /// reported when an `EVAL` runs it.
    pub fn overlap(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        shape: &BodyShape<'_>,
    ) -> Option<&'a ShapeError<'a>> {
        let overlap = (self.0.closing(shape.segment())).and_then(|(closing, _)| closing.overlap);
        // An overlap names a builtin overload's closed type: nothing to close.
        self.minted(types, overlap)
    }

    /// Why the code shape `shape`'s code cannot be built or does not type, reported when an `EVAL`
    /// runs it, worded as a load with its bindings declared words it: closed through the chain's
    /// tables, as a re-run under them reads it, then each capture of the code through `captured`,
    /// the closed type the `EVAL`'s code binds it to, where it binds one. Laid in `scratch` where
    /// some type closes.
    pub fn typing_refusal<'l>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'l>,
        shape: &BodyShape<'_>,
        captured: &mut dyn FnMut(CaptureSlot) -> Option<KType>,
    ) -> Option<&'l ShapeError<'l>>
    where
        'a: 'l,
    {
        let refusal: Option<&'l ShapeError<'l>> = self.refused(shape);
        let refusal = self.resolved_laying(types, scratch, refusal)?;
        let (shapes, segment) = (self.0.shapes(), shape.segment());
        let captured = std::cell::RefCell::new(captured);
        Some(refusal.closed_laying(&mut Capturing {
            types,
            lay: scratch,
            bound: &|level| match shapes.home(segment, level)? {
                Target::Capture(capture) => (captured.borrow_mut())(capture),
                Target::Local(_) => None,
            },
        }))
    }

    /// Whether the code shape `shape`'s code cannot be built or does not type: a binding closes a
    /// refusal's wording, never whether there is one.
    pub fn refuses(&self, shape: &BodyShape<'_>) -> bool {
        self.refused(shape).is_some()
    }

    /// The refusal of the code shape `shape`, as laid.
    fn refused(&self, shape: &BodyShape<'_>) -> Option<&'a ShapeError<'a>> {
        // A code shape the builder could not build holds its refusal itself.
        (self.0.closing(shape.segment()))
            .and_then(|(closing, _)| closing.refusal)
            .or_else(|| self.at(shape).refusal())
    }

    /// The static type of the part at `site` of `shape`, where the load typed one.
    pub fn value_type(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        shape: &BodyShape<'_>,
        site: Site,
    ) -> Option<Interval> {
        self.resolved(types, self.at_site(shape, site, |typed| typed.parts))
    }

    /// The static type of statement `index` of `shape`'s body.
    pub fn statement_type(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        shape: &BodyShape<'_>,
        index: usize,
    ) -> Option<Interval> {
        let index = index as u32;
        let typed = self.typed_in(shape, self.0.shapes().running(shape.segment(), index))?;
        self.resolved(types, typed.statements.get(index).map(|(_, typed)| *typed))
    }

    /// The static type of the binder at `slot` of `shape`.
    pub fn binder_type(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        shape: &BodyShape<'_>,
        slot: Slot,
    ) -> Option<DeclaredType<Interval>> {
        self.resolved(types, self.at_slot(shape, slot, |typed| typed.binders))
    }

    /// What the load fixed about the keyworded use whose node sits at `site` of `shape`.
    pub fn use_statics<'l>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'l>,
        shape: &BodyShape<'_>,
        site: Site,
    ) -> UseStatics<'l>
    where
        'a: 'l,
    {
        let statics: UseStatics<'l> =
            (self.at_site(shape, site, |typed| typed.uses)).unwrap_or(UseStatics::UNFIXED);
        self.resolved_laying(types, scratch, statics)
    }

    /// The route the load fixed for the tail call whose node sits at `site` of `shape`, where it
    /// knows the callee.
    pub fn route(&self, shape: &BodyShape<'_>, site: Site) -> Option<Route<'a>> {
        self.at_site(shape, site, |typed| typed.routes)
    }

    /// What the load fixed about the arms of the `MATCH` whose node sits at `site` of `shape`.
    pub fn arms(&self, shape: &BodyShape<'_>, site: Site) -> Option<Arms<'a>> {
        self.at_site(shape, site, |typed| typed.arms)
    }

    /// What the load recorded a retype at `site` of `shape` re-stamps at, where it knew how and
    /// the reading frame's binding closes the type it re-stamps at.
    pub fn retype(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        shape: &BodyShape<'_>,
        site: Site,
    ) -> Option<Retype> {
        let restamp = self.resolved(types, self.at_site(shape, site, |typed| typed.retypes))?;
        restamp.retype(types)
    }

    /// What a frame of the callable `shape` re-stamps its arguments and its value at, where the
    /// load closed their declared types.
    pub fn retypes<'l>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'l>,
        shape: &BodyShape<'_>,
    ) -> Retypes<'l>
    where
        'a: 'l,
    {
        let retypes = self.0.closing(shape.segment());
        self.resolved_laying(
            types,
            scratch,
            retypes.map_or(Retypes::NONE, |(closing, _)| closing.retypes),
        )
    }

    /// The class the load fixed for the frame body `shape`.
    pub fn class(&self, shape: &BodyShape<'_>) -> Option<BodyClass> {
        self.0.closing(shape.segment())?.0.class
    }

    /// Whether the frame body `shape` caches.
    pub fn caches(&self, shape: &BodyShape<'_>) -> bool {
        self.caching(shape).is_some_and(|caching| caching.caches)
    }

    /// Where `shape`'s types intern at run.
    pub fn caching(&self, shape: &BodyShape<'_>) -> Option<&'a Caching<'a>> {
        self.0.folds(shape.segment())?.caching
    }

    /// What the trace names at each call site of a keyed body in `shape`, by site.
    pub fn traces(&self, shape: &BodyShape<'_>) -> Keyed<'a, (Site, CallTrace<'a>)> {
        (self.0.folds(shape.segment())).map_or(Keyed::EMPTY, |folds| folds.traces)
    }

    /// Each call site the owning body `owner` owns that covers its callee's keys, sorted by shape
    /// and site.
    pub fn slot_maps(&self, owner: Segment) -> &'a [SlotMap<'a>] {
        (self.0.folds(owner)).map_or(&[], |folds| folds.slot_maps)
    }

    /// What the trace names at the call site `site` of `shape`, where it calls a keyed body.
    pub fn trace(&self, shape: &BodyShape<'_>, site: Site) -> Option<CallTrace<'a>> {
        self.traces(shape).get(site).map(|(_, trace)| *trace)
    }

    /// How the load classed the minting site at `site`, one of `shape`'s own.
    pub fn mint_at(&self, shape: &BodyShape<'_>, site: Site) -> Option<Mint> {
        let caching = self.caching(shape)?;
        caching.mints.get(site).map(|(_, mint)| *mint)
    }

    /// Whether the load recorded `site` as one of `shape`'s own minting sites.
    pub fn mints_at(&self, shape: &BodyShape<'_>, site: Site) -> bool {
        self.lists_site(shape, site, |typed| typed.minting)
    }

    /// The content tree of the view the `:|` at `site` of `shape` builds, where its signature is
    /// closed.
    pub fn view_tree(&self, shape: &BodyShape<'_>, site: Site) -> Option<&'a Tree<'a>> {
        let tree = self.at_site(shape, site, |typed| typed.trees)?;
        Some(self.substitution(*tree).unwrap_or(tree))
    }

    /// What the view the `:|` at `site` of `shape` builds carries for each keyworded member, where
    /// the load sees all of the body-born module it views.
    pub fn picks(&self, shape: &BodyShape<'_>, site: Site) -> Option<&'a [Pick<'a>]> {
        self.at_site(shape, site, |typed| typed.picks)
    }

    /// Whether the load settled the ascription at `site` of `shape`.
    pub fn settled(&self, shape: &BodyShape<'_>, site: Site) -> bool {
        self.lists_site(shape, site, |typed| typed.settled)
    }

    /// What the load fixed where `shape` reads a quantified function at `site`.
    pub fn instance_at<'l>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'l>,
        shape: &BodyShape<'_>,
        site: Site,
    ) -> Option<InstanceRead<'l>>
    where
        'a: 'l,
    {
        let read: Option<InstanceRead<'l>> = self.at_site(shape, site, |typed| typed.instances);
        self.resolved_laying(types, scratch, read)
    }

    /// The point each variable of the group of the candidate the keyworded use at `site` of
    /// `shape` selects solves to, by group index, where the load selected one.
    pub fn selection<'l>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'l>,
        shape: &BodyShape<'_>,
        site: Site,
    ) -> Option<Points<'l>>
    where
        'a: 'l,
    {
        let points: Option<Points<'l>> = self.at_site(shape, site, |typed| typed.selections);
        self.resolved_laying(types, scratch, points)
    }

    /// The content tree of the module the binder at `slot` of `shape` holds, if the load read it.
    pub fn content(&self, shape: &BodyShape<'_>, slot: Slot) -> Option<Tree<'a>> {
        let tree = self.at_slot(shape, slot, |typed| typed.contents)?;
        Some(self.substitution(tree).copied().unwrap_or(tree))
    }

    /// The tree a table of its chain's layers gives `tree` under their binding, where `tree` is
    /// open and one does: the tree a fresh load reads under it.
    fn substitution(&self, tree: Tree<'a>) -> Option<&'a Tree<'a>> {
        match tree.identity() {
            identity @ Identity::Open(_) if self.0.closes() => self.0.closed_tree(identity),
            _ => None,
        }
    }

    /// What each parameter of the call by name whose argument sits at `site` of `shape` is solved
    /// from.
    pub fn named_contributions<'l>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'l>,
        shape: &BodyShape<'_>,
        site: Site,
    ) -> &'l [StaticType<'l>]
    where
        'a: 'l,
    {
        let named: &'l [StaticType<'l>] =
            (self.at_site(shape, site, |typed| typed.by_name)).unwrap_or(&[]);
        self.resolved_laying(types, scratch, named)
    }

    /// The slots `shape` binds at a statement that some unit's frontier holds.
    pub fn rooting_seams(&self, shape: &BodyShape<'_>) -> &'a [Slot] {
        self.0
            .folds(shape.segment())
            .map_or(&[], |folds| folds.rooting)
    }

    pub fn statement_seams(&self, shape: &BodyShape<'_>) -> &'a [Slot] {
        self.0
            .folds(shape.segment())
            .map_or(&[], |folds| folds.seams)
    }

    /// The parameters of the callable or block `shape`, or the captures of the quote's code
    /// `shape`, some unit beneath it waits on with its entry as its seam.
    pub fn entry_waits(&self, shape: &BodyShape<'_>) -> &'a [Address] {
        self.0
            .folds(shape.segment())
            .map_or(&[], |folds| folds.waits)
    }

    /// How many of `shape`'s slots keep a digest memo.
    pub fn digested_count(&self, shape: &BodyShape<'_>) -> usize {
        self.0.shapes().digested_count(shape.segment())
    }

    /// Where `slot`'s digest memo sits among `shape`'s, if it keeps one.
    pub fn digested_index(&self, shape: &BodyShape<'_>, slot: Slot) -> Option<usize> {
        self.0.shapes().digested_index(shape.segment(), slot)
    }

    /// Where the captures of the module the `:|` at `site` of `shape` views keep their memos.
    pub fn view_slots(&self, shape: &BodyShape<'_>, site: Site) -> Option<ViewSlots<'a>> {
        self.0.shapes().view_slots(shape.segment(), site)
    }

    /// The digested slots the call keyed by `site` of `shape` passes bare.
    pub fn handoffs(&self, shape: &BodyShape<'_>, site: Site) -> &'a [Handoff] {
        self.0.shapes().handoffs(shape.segment(), site)
    }
}

impl UnitFacts<'_> {
    /// Each fact this record holds that the shape cannot place, by what it is found by.
    pub(super) fn held(&self, each: &mut impl FnMut(Held)) {
        for (segment, typed) in self.typed.iter() {
            typed.held(*segment, each);
        }
        for ((root, name), (_, resolution)) in self.paths.iter() {
            each(Held::Path(*root, *name));
            if let Resolved::Class(class) = resolution.resolved {
                each(Held::Class(*root, class));
            }
        }
    }
}

impl Typed<'_> {
    /// Each site and through fact of `segment` these tables hold, by what it is found by.
    pub(crate) fn held(&self, segment: Segment, each: &mut impl FnMut(Held)) {
        let sites = (self.expressions.iter().map(|(site, _)| *site))
            .chain(self.parts.iter().map(|(site, _)| *site))
            .chain(self.settled.iter().copied())
            .chain(self.instances.iter().map(|(site, _)| *site))
            .chain(self.uses.iter().map(|(site, _)| *site))
            .chain(self.selections.iter().map(|(site, _)| *site))
            .chain(self.by_name.iter().map(|(site, _)| *site))
            .chain(self.routes.iter().map(|(site, _)| *site))
            .chain(self.arms.iter().map(|(site, _)| *site))
            .chain(self.retypes.iter().map(|(site, _)| *site))
            .chain(self.trees.iter().map(|(site, _)| *site))
            .chain(self.picks.iter().map(|(site, _)| *site))
            .chain(self.minting.iter().copied());
        sites.for_each(|site| each(Held::Site(segment, site)));
        for (slot, held) in self.holding.iter() {
            held.iter()
                .for_each(|identity| each(Held::Holder(segment, *identity, Holder::Slot(*slot))));
        }
        // A view written in place holds a module of its own tree, where a binder could name it.
        for (site, tree) in self.trees.iter() {
            if tree.identity() != Identity::Unnamed {
                each(Held::Holder(segment, tree.identity(), Holder::Site(*site)));
            }
        }
    }
}

#[cfg(test)]
thread_local! {
    /// How many rigid facts and points reached the run's door: what the door law holds at nothing.
    /// Tests reset it.
    pub(crate) static RIGID_AT_RUN: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Count a rigid fact or point the run's door met.
#[cfg(test)]
pub(crate) fn rigid_at_run() {
    RIGID_AT_RUN.with(|count| count.set(count.get() + 1));
}

#[cfg(test)]
thread_local! {
    /// How many terms the run probed: each a frame's read found its chain lacking, and each a
    /// walk minted whole before it ran. Tests reset it.
    pub(crate) static TERM_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A closer writing in the closed type `bound` gives a level: a quote's code's captures, as its
/// `EVAL` binds them.
struct Capturing<'q, 'tier, 'pair, 'reg, 'door, 'l> {
    types: &'q Chain<'tier, 'pair, 'reg, 'door>,
    lay: BumpAllocator<'l>,
    bound: &'q dyn Fn(usize) -> Option<KType>,
}

impl Closer for Capturing<'_, '_, '_, '_, '_, '_> {
    fn close(&mut self, ty: DeclaredType<Parametric>) -> Closed {
        let binds = super::derived::LevelsOnly(self.bound);
        super::question::rebound(self.types, self.lay, ty, &binds)
    }
}

impl<'l> Laying<'l> for Capturing<'_, '_, '_, '_, '_, 'l> {
    type Lay = BumpAllocator<'l>;

    fn lay(&self) -> BumpAllocator<'l> {
        self.lay
    }
}

/// The run's closer: a probe of the tables of closed types the chain's layers hold, laying what it
/// copies in `lay`. It never interns.
struct Probing<'q, S, L> {
    source: &'q S,
    lay: Option<L>,
}

impl<'a, S: RecordSource<'a>, L> Closer for Probing<'_, S, L> {
    fn close(&mut self, ty: DeclaredType<Parametric>) -> Closed {
        self.source.closed(ty).unwrap_or(Closed::Same)
    }
}

impl<'a, 'l, S: RecordSource<'a>> Laying<'l> for Probing<'_, S, BumpAllocator<'l>> {
    type Lay = BumpAllocator<'l>;

    fn lay(&self) -> BumpAllocator<'l> {
        self.lay.expect("a laying read takes a scratch")
    }
}
